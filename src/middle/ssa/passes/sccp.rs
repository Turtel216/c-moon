//! Sparse conditional constant propagation (Wegman and Zadeck).
//!
//! One pass that does the work of three.  It propagates constants, folds the
//! arithmetic they make constant, and decides which branches can be taken --
//! and it does all three *together*, which is what makes it stronger than any
//! order of running them separately.  A branch whose condition is known is
//! never taken on one side, so the values arriving from that side are not
//! merged into the phi at the join; which can make the phi constant; which can
//! make another branch known.
//!
//! ```text
//! x = 1                  a plain constant propagation pass gives up at the
//! if (x) y = 2; else ...  join, because `y` is 2 on one edge and something
//! return y                else on the other. SCCP never looks at the other
//!                         edge, because it proves it is never taken.
//! ```
//!
//! # The lattice
//!
//! Each value is *unknown* (nothing has been proved yet), a *constant*, or
//! *varying*.  Analysis is optimistic: everything starts unknown and only ever
//! moves down, so a value in a loop is assumed constant until an argument
//! proves otherwise.  That is what lets a loop-carried value be recognised as
//! constant; a pessimistic pass starting at "varying" never can.
//!
//! An undefined value counts as the constant zero, which is what leaving SSA
//! materialises it as.  The two have to agree: the program is reading a
//! variable it never wrote, and the compiler is entitled to pick, but it must
//! pick the same thing twice.
//!
//! # What it does not do
//!
//! Nothing is deleted here beyond blocks that turn out unreachable.  An
//! instruction whose result is now a literal everywhere is left in place for
//! dead-code elimination to remove, so that each pass has one job.

use std::collections::{HashMap, HashSet};

use crate::middle::ir::{Sign, Width};
use crate::middle::ssa::defuse::{DefUse, Use, UsePosition};
use crate::middle::ssa::{BinOp, BlockId, Function, InstId, Op, Operand, Terminator, ValueId};

/// What is known about a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Known {
    /// Nothing yet: no executable definition of it has been seen.
    Nothing,
    /// This literal, on every path that reaches it.
    Constant(i64),
    /// Not the same on every path, or not computable at compile time.
    Anything,
}

impl Known {
    /// What is known about a value given two things known about it.
    ///
    /// This is the lattice's meet: agreement survives, disagreement does not.
    fn meet(self, other: Known) -> Known {
        match (self, other) {
            (Known::Nothing, known) | (known, Known::Nothing) => known,
            (Known::Constant(left), Known::Constant(right)) if left == right => {
                Known::Constant(left)
            }
            _ => Known::Anything,
        }
    }
}

/// Propagate constants, fold what becomes constant, and delete the blocks that
/// turn out to be unreachable.
///
/// # Returns
///
/// Whether anything changed.  Running it twice over the same function changes
/// nothing the second time.
pub fn run(function: &mut Function) -> bool {
    let solution = Solver::solve(function);
    apply(function, &solution)
}

/// What the analysis concluded.
struct Solution {
    known: HashMap<ValueId, Known>,
    executable_edges: HashSet<(BlockId, BlockId)>,
}

/// The analysis: two worklists run to a fixed point.
struct Solver<'a> {
    function: &'a Function,
    chains: DefUse,
    known: HashMap<ValueId, Known>,
    /// Blocks proved reachable so far.
    executable: Vec<bool>,
    /// Edges proved traversable so far.
    edges: HashSet<(BlockId, BlockId)>,
    /// Edges found traversable but not yet followed.
    flow: Vec<(BlockId, BlockId)>,
    /// Values whose knowledge changed, whose readers must be revisited.
    ssa: Vec<ValueId>,
}

impl<'a> Solver<'a> {
    fn solve(function: &'a Function) -> Solution {
        let mut solver = Self {
            function,
            chains: DefUse::compute(function),
            known: HashMap::new(),
            executable: vec![false; function.block_count()],
            edges: HashSet::new(),
            flow: Vec::new(),
            ssa: Vec::new(),
        };

        solver.enter(function.entry());
        // Both worklists feed each other -- a newly reachable block can make a
        // value known, and a newly known value can make a block reachable --
        // so neither is done until both are.
        while !solver.flow.is_empty() || !solver.ssa.is_empty() {
            while let Some((from, to)) = solver.flow.pop() {
                if !solver.edges.insert((from, to)) {
                    continue;
                }
                // A new way into the block means its phis merge one argument
                // more than they did before.
                solver.visit_phis(to);
                if !solver.executable[to.index()] {
                    solver.enter(to);
                }
            }

            while let Some(value) = solver.ssa.pop() {
                for site in solver.chains.uses_of(value).to_vec() {
                    if solver.executable[site.block.index()] {
                        solver.visit(site);
                    }
                }
            }
        }

        Solution {
            known: solver.known,
            executable_edges: solver.edges,
        }
    }

    /// Reach a block for the first time: everything in it now runs.
    fn enter(&mut self, block: BlockId) {
        self.executable[block.index()] = true;
        self.visit_phis(block);
        for &inst in &self.function.block(block).insts {
            self.visit_inst(inst);
        }
        self.visit_terminator(block);
    }

    /// Revisit whatever reads a value that has just changed.
    fn visit(&mut self, site: Use) {
        match site.position {
            UsePosition::Phi { phi, .. } => self.visit_phi(site.block, phi),
            UsePosition::Inst { inst, .. } => self.visit_inst(inst),
            UsePosition::Terminator { .. } => self.visit_terminator(site.block),
        }
    }

    fn visit_phis(&mut self, block: BlockId) {
        for phi in 0..self.function.block(block).phis.len() {
            self.visit_phi(block, phi);
        }
    }

    /// A phi is the meet of the arguments that arrive along an edge that is
    /// actually traversable.  Ignoring the others is the whole of the
    /// "conditional" in this pass's name.
    fn visit_phi(&mut self, block: BlockId, phi: usize) {
        let node = &self.function.block(block).phis[phi];
        let mut result = Known::Nothing;

        for (argument, &operand) in node.args.iter().enumerate() {
            let predecessor = self.function.block(block).preds()[argument];
            if self.edges.contains(&(predecessor, block)) {
                result = result.meet(self.operand(operand));
            }
        }

        self.record(node.dest, result);
    }

    fn visit_inst(&mut self, inst: InstId) {
        let Some(dest) = self.function.inst(inst).dest else {
            return;
        };
        let known = self.evaluate(&self.function.inst(inst).op);
        self.record(dest, known);
    }

    /// What an operation produces, given what is known about its operands.
    fn evaluate(&self, op: &Op) -> Known {
        match op {
            Op::Binary(operator, width, lhs, rhs) => {
                match (self.operand(*lhs), self.operand(*rhs)) {
                    // One operand that could be anything is enough.
                    (Known::Anything, _) | (_, Known::Anything) => Known::Anything,
                    (Known::Constant(left), Known::Constant(right)) => {
                        fold(*operator, *width, left, right)
                            .map_or(Known::Anything, Known::Constant)
                    }
                    // Something is still unknown, so this is too -- for now.
                    _ => Known::Nothing,
                }
            }
            Op::Copy(source) => self.operand(*source),

            // A conversion of a known value is known: the constant is held
            // sign-extended, so narrowing it is all there is to do. It is
            // narrowed to the source width first, so that a widening
            // conversion sign-extends from the bit the source ends at rather
            // than from wherever the constant happens to.
            Op::Convert {
                from,
                sign,
                to,
                value,
            } => match self.operand(*value) {
                Known::Constant(constant) => Known::Constant(to.narrow(from.read(*sign, constant))),
                other => other,
            },

            // The undefined value is materialised as zero when SSA is left, so
            // that is what it is worth here.
            Op::Undef => Known::Constant(0),

            // Everything else produces a value this pass cannot see into: what
            // a caller returns, what an argument arrives as, what is in
            // memory.
            Op::Call { .. }
            | Op::GetParam(_)
            | Op::SlotLoad { .. }
            | Op::ArrayLoad { .. }
            | Op::ArrayAddr { .. }
            | Op::Load { .. }
            | Op::AddrOf { .. } => Known::Anything,

            // The stores define nothing.
            Op::SlotStore { .. } | Op::ArrayStore { .. } | Op::Store { .. } => Known::Nothing,
        }
    }

    /// Follow a block's transfer, marking the edges it can take.
    fn visit_terminator(&mut self, block: BlockId) {
        match self.function.block(block).terminator() {
            Terminator::Jump(target) => self.flow.push((block, *target)),

            Terminator::Branch {
                cond,
                then_block,
                else_block,
                ..
            } => match self.operand(*cond) {
                // Nothing is known about the condition yet, so neither side
                // has been proved reachable. Optimism: assume neither until
                // something says otherwise.
                Known::Nothing => {}
                Known::Constant(0) => self.flow.push((block, *else_block)),
                Known::Constant(_) => self.flow.push((block, *then_block)),
                Known::Anything => {
                    self.flow.push((block, *then_block));
                    self.flow.push((block, *else_block));
                }
            },

            Terminator::Return(_) => {}
        }
    }

    /// What is known about an operand.
    fn operand(&self, operand: Operand) -> Known {
        match operand {
            Operand::Imm(constant) => Known::Constant(constant),
            Operand::Value(value) => self.known.get(&value).copied().unwrap_or(Known::Nothing),
        }
    }

    /// Record what is now known about a value, and wake its readers if that
    /// changed.
    fn record(&mut self, value: ValueId, known: Known) {
        let before = self.known.get(&value).copied().unwrap_or(Known::Nothing);
        // Meeting with what was known keeps the walk going downwards even if a
        // recomputation would have produced something higher, which is what
        // guarantees it terminates.
        let after = before.meet(known);

        if after != before {
            self.known.insert(value, after);
            self.ssa.push(value);
        }
    }
}

/// Rewrite the function according to what was proved.
fn apply(function: &mut Function, solution: &Solution) -> bool {
    let constants: HashMap<ValueId, Operand> = solution
        .known
        .iter()
        .filter_map(|(&value, &known)| match known {
            Known::Constant(constant) => Some((value, Operand::Imm(constant))),
            _ => None,
        })
        .collect();

    let mut changed = function.substitute_operands(&constants);

    // A branch with one traversable edge is a jump. Replacing the terminator
    // drops the other edge, and with it the phi arguments that arrived along
    // it.
    for block in function.block_ids() {
        let Terminator::Branch {
            then_block,
            else_block,
            ..
        } = *function.block(block).terminator()
        else {
            continue;
        };

        let taken = match (
            solution.executable_edges.contains(&(block, then_block)),
            solution.executable_edges.contains(&(block, else_block)),
        ) {
            (true, false) => then_block,
            (false, true) => else_block,
            // Both traversable, or the block itself unreachable and neither
            // proved: leave it, and let the sweep below deal with the block.
            _ => continue,
        };

        function.set_terminator(block, Terminator::Jump(taken));
        changed = true;
    }

    // Blocks that only the branches just folded could reach are now
    // unreachable, and dominance is undefined for those.
    changed |= function.retain_reachable();
    changed
}

/// Evaluate a binary operation on two constants.
///
/// # Returns
///
/// `None` when the operation has no compile-time answer, which is division by
/// zero.  The program is free to do that; the compiler is not free to decide
/// what it produces.
fn fold(operator: BinOp, width: Width, lhs: i64, rhs: i64) -> Option<i64> {
    // Wrapping arithmetic throughout: overflow is undefined in C, and the
    // target wraps, so folding must agree with what the hardware would have
    // done rather than crash the compiler.
    //
    // It has to wrap where the hardware would, too: narrowing the result to
    // the width the instruction computes at is what makes a folded `int`
    // overflow come out as the machine's 32-bit answer rather than the one an
    // `i64` would have given.
    //
    // An unsigned operation reads the same two bit patterns as unsigned
    // numbers, which is what `Width::unsigned` hands it; the answer is stored
    // back the one way constants are stored.
    Some(match operator {
        BinOp::Add => width.narrow(lhs.wrapping_add(rhs)),
        BinOp::Sub => width.narrow(lhs.wrapping_sub(rhs)),
        BinOp::Mul => width.narrow(lhs.wrapping_mul(rhs)),
        BinOp::Div(Sign::Signed) => {
            if rhs == 0 {
                return None;
            }
            width.narrow(lhs.wrapping_div(rhs))
        }
        BinOp::Div(Sign::Unsigned) => {
            let divisor = width.unsigned(rhs);
            if divisor == 0 {
                return None;
            }
            width.narrow((width.unsigned(lhs) / divisor) as i64)
        }
        // The bitwise operations work a bit at a time, so they never carry
        // anything into the bits above the width; narrowing is what keeps the
        // answer in the one form a constant is stored in.
        BinOp::And => width.narrow(lhs & rhs),
        BinOp::Or => width.narrow(lhs | rhs),
        BinOp::Xor => width.narrow(lhs ^ rhs),
        // A shift by more than the width is undefined in C, so folding is free
        // to answer with whatever the machine would have: `Width::shift_count`
        // is that rule, and using it here is what keeps the optimised and the
        // unoptimised build of the same program in agreement.
        BinOp::Shl => width.narrow(lhs.wrapping_shl(width.shift_count(rhs))),
        // The left operand's signedness decides what fills the vacated top
        // bits: reading it signed shifts its sign down, reading it unsigned
        // shifts zeroes in.
        BinOp::Shr(Sign::Signed) => width.narrow(lhs.wrapping_shr(width.shift_count(rhs))),
        BinOp::Shr(Sign::Unsigned) => {
            width.narrow((width.unsigned(lhs) >> width.shift_count(rhs)) as i64)
        }
        // Equality asks whether the bits are the same, which they either are
        // or are not however they read.
        BinOp::Eq => i64::from(lhs == rhs),
        BinOp::Neq => i64::from(lhs != rhs),
        BinOp::Lt(Sign::Signed) => i64::from(lhs < rhs),
        BinOp::Lte(Sign::Signed) => i64::from(lhs <= rhs),
        BinOp::Gt(Sign::Signed) => i64::from(lhs > rhs),
        BinOp::Gte(Sign::Signed) => i64::from(lhs >= rhs),
        BinOp::Lt(Sign::Unsigned) => i64::from(width.unsigned(lhs) < width.unsigned(rhs)),
        BinOp::Lte(Sign::Unsigned) => i64::from(width.unsigned(lhs) <= width.unsigned(rhs)),
        BinOp::Gt(Sign::Unsigned) => i64::from(width.unsigned(lhs) > width.unsigned(rhs)),
        BinOp::Gte(Sign::Unsigned) => i64::from(width.unsigned(lhs) >= width.unsigned(rhs)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::middle::ssa::verify::verify_ssa;
    use crate::middle::ssa::{SlotOrigin, Terminator};

    fn function() -> Function {
        Function::new("f".to_string(), "entry".to_string(), "exit".to_string())
    }

    /// The block whose label is `label`, which must still exist.
    fn block_named(function: &Function, label: &str) -> Option<BlockId> {
        function
            .block_ids()
            .find(|&block| function.block(block).label == label)
    }

    #[test]
    fn a_folded_shift_agrees_with_the_one_the_hardware_would_have_run() {
        // Arrange: the bits an `int` holds -1 in.
        let all_ones = -1;

        // Act / Assert: a right shift is where the two readings part company.
        // Signed, the sign is shifted down and -1 stays -1 ...
        assert_eq!(
            fold(BinOp::Shr(Sign::Signed), Width::Bits32, all_ones, 1),
            Some(-1)
        );
        // ... unsigned, zeroes come in at the top of the 32-bit value.
        assert_eq!(
            fold(BinOp::Shr(Sign::Unsigned), Width::Bits32, all_ones, 1),
            Some(2147483647)
        );

        // A left shift is one operation: what leaves the top of the width is
        // gone whichever way the operand reads.
        assert_eq!(
            fold(BinOp::Shl, Width::Bits32, 1, 31),
            Some(i64::from(i32::MIN))
        );
        assert_eq!(fold(BinOp::Shl, Width::Bits64, 1, 31), Some(2147483648));

        // A count past the width is undefined in C, and the answer here is
        // the one the machine gives: only the low five bits of it are read.
        assert_eq!(fold(BinOp::Shl, Width::Bits32, 1, 33), Some(2));
        assert_eq!(fold(BinOp::Shl, Width::Bits64, 1, 33), Some(8589934592));
    }

    #[test]
    fn folding_a_bitwise_operation_leaves_a_constant_of_its_own_width() {
        // Arrange / Act / Assert: the answer is stored the way every constant
        // is -- the low bits, sign-extended -- so an `int` whose top bit the
        // operation set comes out negative.
        assert_eq!(fold(BinOp::And, Width::Bits32, 12, 10), Some(8));
        assert_eq!(fold(BinOp::Or, Width::Bits32, 12, 10), Some(14));
        assert_eq!(fold(BinOp::Xor, Width::Bits32, 12, 10), Some(6));
        assert_eq!(fold(BinOp::Xor, Width::Bits32, 0, -1), Some(-1));
        assert_eq!(
            fold(BinOp::Or, Width::Bits32, 0, i64::from(i32::MIN)),
            Some(i64::from(i32::MIN))
        );
    }

    #[test]
    fn folding_reads_the_operands_the_way_the_operation_does() {
        // Arrange: the bits an `int` holds -1 in are the bits an `unsigned
        // int` holds its largest value in.
        let all_ones = -1;

        // Act / Assert: one pair of constants, two answers per operation.
        assert_eq!(
            fold(BinOp::Div(Sign::Signed), Width::Bits32, all_ones, 2),
            Some(0)
        );
        assert_eq!(
            fold(BinOp::Div(Sign::Unsigned), Width::Bits32, all_ones, 2),
            Some(2147483647)
        );
        assert_eq!(
            fold(BinOp::Lt(Sign::Signed), Width::Bits32, all_ones, 2),
            Some(1)
        );
        assert_eq!(
            fold(BinOp::Lt(Sign::Unsigned), Width::Bits32, all_ones, 2),
            Some(0)
        );
        // An equality asks only whether the bits are the same, so it is one
        // operation rather than two.
        assert_eq!(fold(BinOp::Eq, Width::Bits32, all_ones, all_ones), Some(1));
        // Dividing by zero is the program's business either way.
        assert_eq!(fold(BinOp::Div(Sign::Unsigned), Width::Bits32, 1, 0), None);
    }

    #[test]
    fn arithmetic_on_constants_is_folded_through_a_chain() {
        // Arrange: `a = 2 + 3; b = a * 4; return b`.
        let mut function = function();
        let entry = function.entry();
        let sum = function
            .emit(
                entry,
                Op::Binary(BinOp::Add, Width::Bits64, Operand::Imm(2), Operand::Imm(3)),
            )
            .expect("an addition defines a value");
        let product = function
            .emit(
                entry,
                Op::Binary(
                    BinOp::Mul,
                    Width::Bits64,
                    Operand::Value(sum),
                    Operand::Imm(4),
                ),
            )
            .expect("a multiplication defines a value");
        function.set_terminator(entry, Terminator::Return(Some(Operand::Value(product))));

        // Act
        assert!(run(&mut function));

        // Assert: the arithmetic is still there for dead-code elimination to
        // remove, but nothing reads it any more.
        assert_eq!(verify_ssa(&function), Ok(()));
        assert_eq!(
            *function.block(entry).terminator(),
            Terminator::Return(Some(Operand::Imm(20)))
        );
    }

    #[test]
    fn a_branch_on_a_known_condition_loses_its_other_arm() {
        // Arrange: `if (1) return 1; else return 2;`
        let mut function = function();
        let entry = function.entry();
        let taken = function.add_block("taken".to_string());
        let untaken = function.add_block("untaken".to_string());
        function.set_terminator(taken, Terminator::Return(Some(Operand::Imm(1))));
        function.set_terminator(untaken, Terminator::Return(Some(Operand::Imm(2))));
        function.set_terminator(
            entry,
            Terminator::Branch {
                cond: Operand::Imm(1),
                then_block: taken,
                else_block: untaken,
                width: Width::Bits64,
            },
        );

        // Act
        assert!(run(&mut function));

        // Assert
        assert_eq!(verify_ssa(&function), Ok(()));
        assert!(block_named(&function, "untaken").is_none());
        assert_eq!(function.block_count(), 2);
    }

    #[test]
    fn a_merge_is_constant_when_only_one_of_its_edges_can_be_taken() {
        // Arrange: the case that gives this pass its name. Plain constant
        // propagation stops at the join, because the value differs between the
        // two arms; SCCP never merges the arm it proves unreachable.
        //
        //   if (1) x = 5; else x = 7;
        //   return x;
        let mut function = function();
        let entry = function.entry();
        let left = function.add_block("left".to_string());
        let right = function.add_block("right".to_string());
        let join = function.add_block("join".to_string());

        function.set_terminator(left, Terminator::Jump(join));
        function.set_terminator(right, Terminator::Jump(join));
        function.set_terminator(
            entry,
            Terminator::Branch {
                cond: Operand::Imm(1),
                then_block: left,
                else_block: right,
                width: Width::Bits64,
            },
        );

        let slot = function.slot_for(SlotOrigin::Variable(0));
        let merged = function.add_phi(join, slot);
        function.block_mut(join).phis[0].args = vec![Operand::Imm(5), Operand::Imm(7)];
        function.set_terminator(join, Terminator::Return(Some(Operand::Value(merged))));

        // Act
        assert!(run(&mut function));

        // Assert: the value is known, and the arm that would have contradicted
        // it is gone.
        assert_eq!(verify_ssa(&function), Ok(()));
        assert!(block_named(&function, "right").is_none());
        let join = block_named(&function, "join").expect("the join survives");
        assert_eq!(
            *function.block(join).terminator(),
            Terminator::Return(Some(Operand::Imm(5)))
        );
    }

    #[test]
    fn a_value_that_really_varies_is_left_alone() {
        // Arrange: a counter, which is 0 on the way in and something else on
        // the way round, so nothing is known about it.
        //
        //   i = 0; while (i < 10) i = i + 1; return i;
        let mut function = function();
        let entry = function.entry();
        let header = function.add_block("header".to_string());
        let latch = function.add_block("latch".to_string());
        let done = function.add_block("done".to_string());

        function.set_terminator(entry, Terminator::Jump(header));
        function.set_terminator(latch, Terminator::Jump(header));

        let slot = function.slot_for(SlotOrigin::Variable(0));
        let counter = function.add_phi(header, slot);
        let stepped = function
            .emit(
                latch,
                Op::Binary(
                    BinOp::Add,
                    Width::Bits64,
                    Operand::Value(counter),
                    Operand::Imm(1),
                ),
            )
            .expect("an addition defines a value");
        function.block_mut(header).phis[0].args = vec![Operand::Imm(0), Operand::Value(stepped)];

        let test = function
            .emit(
                header,
                Op::Binary(
                    BinOp::Lt(Sign::Signed),
                    Width::Bits64,
                    Operand::Value(counter),
                    Operand::Imm(10),
                ),
            )
            .expect("a comparison defines a value");
        function.set_terminator(
            header,
            Terminator::Branch {
                cond: Operand::Value(test),
                then_block: latch,
                else_block: done,
                width: Width::Bits64,
            },
        );
        function.set_terminator(done, Terminator::Return(Some(Operand::Value(counter))));

        // Act: optimism first believes the counter is 0 and the loop always
        // entered; the back edge is what disproves both.
        run(&mut function);

        // Assert: nothing was folded, and the exit that optimism briefly
        // considered unreachable is still there.
        assert_eq!(verify_ssa(&function), Ok(()));
        assert_eq!(function.block_count(), 4);
        let done = block_named(&function, "done").expect("the exit survives");
        assert_eq!(
            *function.block(done).terminator(),
            Terminator::Return(Some(Operand::Value(counter)))
        );
    }

    #[test]
    fn division_by_zero_is_not_folded() {
        // Arrange: the program is free to divide by zero; the compiler is not
        // free to decide what that produces.
        let mut function = function();
        let entry = function.entry();
        let quotient = function
            .emit(
                entry,
                Op::Binary(
                    BinOp::Div(Sign::Signed),
                    Width::Bits64,
                    Operand::Imm(1),
                    Operand::Imm(0),
                ),
            )
            .expect("a division defines a value");
        function.set_terminator(entry, Terminator::Return(Some(Operand::Value(quotient))));

        // Act
        run(&mut function);

        // Assert: the division survives, and the return still reads it.
        assert_eq!(
            *function.block(entry).terminator(),
            Terminator::Return(Some(Operand::Value(quotient)))
        );
    }

    #[test]
    fn a_call_result_is_never_assumed_constant() {
        // Arrange: what a callee returns is not visible here.
        let mut function = function();
        let entry = function.entry();
        let result = function
            .emit(
                entry,
                Op::Call {
                    callee: "g".to_string(),
                    args: Vec::new(),
                },
            )
            .expect("a call defines a value");
        function.set_terminator(entry, Terminator::Return(Some(Operand::Value(result))));

        // Act / Assert
        assert!(!run(&mut function));
        assert_eq!(
            *function.block(entry).terminator(),
            Terminator::Return(Some(Operand::Value(result)))
        );
    }

    #[test]
    fn running_it_twice_changes_nothing_the_second_time() {
        // Arrange: the pass pipeline's fixed point depends on this.
        let mut function = function();
        let entry = function.entry();
        let sum = function
            .emit(
                entry,
                Op::Binary(BinOp::Add, Width::Bits64, Operand::Imm(2), Operand::Imm(3)),
            )
            .expect("an addition defines a value");
        function.set_terminator(entry, Terminator::Return(Some(Operand::Value(sum))));

        // Act / Assert
        assert!(run(&mut function));
        assert!(!run(&mut function));
    }
}
