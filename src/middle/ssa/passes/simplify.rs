//! Algebraic simplification: identities that hold whatever the value is.
//!
//! Constant folding is [`sccp`](super::sccp)'s job -- this is what is left
//! when only one operand is known.  `x * 1` is `x` without knowing anything
//! about `x`, and `x - x` is zero for every `x` there is.
//!
//! # Why the identities on equal operands are exact here
//!
//! `x - x`, `x / x` and `x == x` all rest on the two operands being the same
//! value.  On three-address code that is a guess: two mentions of a variable
//! are the same value only if nothing wrote to it in between, and the pass has
//! to know that. In SSA it is not a guess at all -- the same `ValueId` *is*
//! the same value, everywhere and always, because it is written once.

use crate::middle::ssa::{BinOp, Function, Op, Operand};

/// Apply the identities to every instruction they fit.
///
/// # Returns
///
/// Whether anything changed.  Running it twice over the same function changes
/// nothing the second time.
pub fn run(function: &mut Function) -> bool {
    let mut changed = false;

    for block in function.block_ids() {
        for inst in function.block(block).insts.clone() {
            if let Some(simpler) = simplify(&function.inst(inst).op) {
                function.inst_mut(inst).op = simpler;
                changed = true;
            }
        }
    }

    changed
}

/// The simpler operation this one is equivalent to, if there is one.
fn simplify(op: &Op) -> Option<Op> {
    let Op::Binary(operator, width, lhs, rhs) = *op else {
        return None;
    };
    let same = matches!((lhs, rhs), (Operand::Value(left), Operand::Value(right)) if left == right);

    match operator {
        BinOp::Add => match (lhs, rhs) {
            (Operand::Imm(0), other) | (other, Operand::Imm(0)) => Some(Op::Copy(other)),
            _ => None,
        },

        BinOp::Sub => match (lhs, rhs) {
            (_, Operand::Imm(0)) => Some(Op::Copy(lhs)),
            _ if same => Some(Op::Copy(Operand::Imm(0))),
            _ => None,
        },

        BinOp::Mul => match (lhs, rhs) {
            (Operand::Imm(0), _) | (_, Operand::Imm(0)) => Some(Op::Copy(Operand::Imm(0))),
            (Operand::Imm(1), other) | (other, Operand::Imm(1)) => Some(Op::Copy(other)),
            // Strength reduction: doubling is an addition, which every machine
            // this targets does faster than a multiplication.
            (Operand::Imm(2), other) | (other, Operand::Imm(2)) => {
                Some(Op::Binary(BinOp::Add, width, other, other))
            }
            _ => None,
        },

        // Dividing by one, or by itself, gives the same answer whichever
        // way the operands read.
        BinOp::Div(_) => match (lhs, rhs) {
            (_, Operand::Imm(1)) => Some(Op::Copy(lhs)),
            // Not for `0 / x`: the divisor may be zero, and the result of that
            // is the program's business rather than the compiler's.
            _ if same => Some(Op::Copy(Operand::Imm(1))),
            _ => None,
        },

        // A value equals itself, is not less than itself, and so on.
        BinOp::Eq | BinOp::Lte(_) | BinOp::Gte(_) if same => Some(Op::Copy(Operand::Imm(1))),
        BinOp::Neq | BinOp::Lt(_) | BinOp::Gt(_) if same => Some(Op::Copy(Operand::Imm(0))),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::middle::ir::{Sign, Width};

    use crate::middle::ssa::Terminator;
    use crate::middle::ssa::verify::verify_ssa;

    /// Simplify `lhs <operator> rhs`, where the operands are built from a
    /// value the pass cannot see into, so only the identities can apply.
    ///
    /// # Returns
    ///
    /// What the operation became, and the opaque operand, for comparison.
    fn simplified(
        operator: BinOp,
        operands: impl Fn(Operand) -> (Operand, Operand),
    ) -> (Op, Operand) {
        let mut function = Function::new("f".to_string(), "entry".to_string(), "exit".to_string());
        let entry = function.entry();
        let opaque = Operand::Value(
            function
                .emit(entry, Op::GetParam(0))
                .expect("a parameter read defines a value"),
        );

        let (lhs, rhs) = operands(opaque);
        let result = function
            .emit(entry, Op::Binary(operator, Width::Bits64, lhs, rhs))
            .expect("a binary operation defines a value");
        function.set_terminator(entry, Terminator::Return(Some(Operand::Value(result))));

        run(&mut function);
        assert_eq!(verify_ssa(&function), Ok(()));

        let inst = function.block(entry).insts[1];
        (function.inst(inst).op.clone(), opaque)
    }

    #[test]
    fn adding_or_subtracting_zero_is_the_value_itself() {
        for build in [|x| (Operand::Imm(0), x), |x| (x, Operand::Imm(0))] {
            let (op, opaque) = simplified(BinOp::Add, build);
            assert_eq!(op, Op::Copy(opaque));
        }

        let (op, opaque) = simplified(BinOp::Sub, |x| (x, Operand::Imm(0)));
        assert_eq!(op, Op::Copy(opaque));
    }

    #[test]
    fn multiplying_or_dividing_by_one_needs_no_multiplication() {
        let (op, opaque) = simplified(BinOp::Mul, |x| (x, Operand::Imm(1)));
        assert_eq!(op, Op::Copy(opaque));

        let (op, opaque) = simplified(BinOp::Mul, |x| (Operand::Imm(1), x));
        assert_eq!(op, Op::Copy(opaque));

        let (op, opaque) = simplified(BinOp::Div(Sign::Signed), |x| (x, Operand::Imm(1)));
        assert_eq!(op, Op::Copy(opaque));
    }

    #[test]
    fn multiplying_by_zero_is_zero_whatever_the_other_operand_is() {
        let (op, _) = simplified(BinOp::Mul, |x| (x, Operand::Imm(0)));
        assert_eq!(op, Op::Copy(Operand::Imm(0)));
    }

    #[test]
    fn doubling_becomes_an_addition() {
        let (op, opaque) = simplified(BinOp::Mul, |x| (x, Operand::Imm(2)));
        assert_eq!(op, Op::Binary(BinOp::Add, Width::Bits64, opaque, opaque));

        let (op, opaque) = simplified(BinOp::Mul, |x| (Operand::Imm(2), x));
        assert_eq!(op, Op::Binary(BinOp::Add, Width::Bits64, opaque, opaque));
    }

    #[test]
    fn an_operation_on_one_value_twice_is_decided_without_knowing_it() {
        for (operator, expected) in [
            (BinOp::Sub, 0),
            (BinOp::Div(Sign::Signed), 1),
            (BinOp::Eq, 1),
            (BinOp::Lte(Sign::Signed), 1),
            (BinOp::Gte(Sign::Signed), 1),
            (BinOp::Neq, 0),
            (BinOp::Lt(Sign::Signed), 0),
            (BinOp::Gt(Sign::Signed), 0),
        ] {
            let (op, _) = simplified(operator, |x| (x, x));
            assert_eq!(op, Op::Copy(Operand::Imm(expected)), "{operator:?}");
        }
    }

    #[test]
    fn dividing_zero_by_something_unknown_is_left_alone() {
        // The divisor may be zero, and what that produces is not the
        // compiler's to decide.
        let (op, opaque) = simplified(BinOp::Div(Sign::Signed), |x| (Operand::Imm(0), x));
        assert_eq!(
            op,
            Op::Binary(
                BinOp::Div(Sign::Signed),
                Width::Bits64,
                Operand::Imm(0),
                opaque
            )
        );
    }

    #[test]
    fn two_different_values_are_not_assumed_equal() {
        let mut function = Function::new("f".to_string(), "entry".to_string(), "exit".to_string());
        let entry = function.entry();
        let first = function
            .emit(entry, Op::GetParam(0))
            .expect("a parameter read defines a value");
        let second = function
            .emit(entry, Op::GetParam(1))
            .expect("a parameter read defines a value");
        let compared = function
            .emit(
                entry,
                Op::Binary(
                    BinOp::Eq,
                    Width::Bits64,
                    Operand::Value(first),
                    Operand::Value(second),
                ),
            )
            .expect("a comparison defines a value");
        function.set_terminator(entry, Terminator::Return(Some(Operand::Value(compared))));

        assert!(!run(&mut function));
    }

    #[test]
    fn running_it_twice_changes_nothing_the_second_time() {
        // Arrange: the pass pipeline's fixed point depends on this, and the
        // strength reduction is the rule that could most easily oscillate.
        let mut function = Function::new("f".to_string(), "entry".to_string(), "exit".to_string());
        let entry = function.entry();
        let opaque = function
            .emit(entry, Op::GetParam(0))
            .expect("a parameter read defines a value");
        let doubled = function
            .emit(
                entry,
                Op::Binary(
                    BinOp::Mul,
                    Width::Bits64,
                    Operand::Value(opaque),
                    Operand::Imm(2),
                ),
            )
            .expect("a multiplication defines a value");
        function.set_terminator(entry, Terminator::Return(Some(Operand::Value(doubled))));

        // Act / Assert
        assert!(run(&mut function));
        assert!(!run(&mut function));
    }
}
