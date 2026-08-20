//! Dead-code elimination: mark and sweep over the def-use graph.
//!
//! In SSA an instruction is dead when nothing reads the value it produces, and
//! that is a question about one value rather than a dataflow analysis over the
//! whole function: a value is written once, so if nothing reads it anywhere,
//! nothing ever will.
//!
//! The pass marks what must be kept and sweeps away the rest.  What must be
//! kept is whatever the program can be observed doing -- a call, a write to
//! memory, the value a block branches on or returns -- and, transitively,
//! whatever those read.  Everything the mark does not reach computes something
//! nobody looks at.
//!
//! # Branches are kept unconditionally
//!
//! Every terminator is a root, so no branch is ever removed and no block is
//! ever emptied of its control flow.  A more aggressive pass would ask whether
//! a branch decides anything -- whether the blocks it chooses between reach the
//! same places anyway -- which needs the post-dominator tree and a notion of
//! control dependence.  That is deliberately not done here: the analysis is
//! not written, and pretending otherwise by deleting branches would be wrong
//! rather than merely conservative.

use std::collections::HashSet;

use crate::middle::ssa::{DefSite, Function, InstId, Operand, ValueId};

/// Remove every instruction and phi whose result nobody reads.
///
/// # Returns
///
/// Whether anything was removed.  Running it twice over the same function
/// removes nothing the second time.
pub fn run(function: &mut Function) -> bool {
    let live = mark(function);
    sweep(function, &live)
}

/// What must be kept.
struct Live {
    values: HashSet<ValueId>,
    insts: HashSet<InstId>,
}

/// Find everything that is observable, and everything it reads.
fn mark(function: &Function) -> Live {
    let mut live = Live {
        values: HashSet::new(),
        insts: HashSet::new(),
    };
    let mut worklist: Vec<Operand> = Vec::new();

    for block in function.block_ids() {
        for &inst in &function.block(block).insts {
            // An operation whose effect is not its result has to run whether
            // or not anything reads it.
            if !function.inst(inst).op.is_pure() {
                live.insts.insert(inst);
                worklist.extend(function.inst(inst).op.operands());
            }
        }

        // Every transfer is a root; see the note on branches above.
        worklist.extend(function.block(block).terminator().operands());
    }

    while let Some(operand) = worklist.pop() {
        // A literal reads nothing, so it keeps nothing alive.
        let Operand::Value(value) = operand else {
            continue;
        };
        if !live.values.insert(value) {
            continue;
        }

        match function.value_def(value).site {
            DefSite::Inst(inst) => {
                live.insts.insert(inst);
                worklist.extend(function.inst(inst).op.operands());
            }
            // A phi reads one value per incoming edge, and reaching the phi
            // means the program can arrive along any of them.
            DefSite::Phi(block, position) => {
                worklist.extend(function.block(block).phis[position].args.iter().copied());
            }
            // Reading a value whose definition is gone is a broken function,
            // which the verifier reports; there is nothing to keep alive.
            DefSite::Removed => {}
        }
    }

    live
}

/// Remove everything the mark did not reach.
fn sweep(function: &mut Function, live: &Live) -> bool {
    let mut removed = false;

    for block in function.block_ids() {
        let before = function.block(block).phis.len();
        function.retain_phis(block, |phi| live.values.contains(&phi.dest));
        removed |= function.block(block).phis.len() != before;

        let dead: Vec<InstId> = function
            .block(block)
            .insts
            .iter()
            .copied()
            .filter(|inst| !live.insts.contains(inst))
            .collect();
        removed |= !dead.is_empty();
        for inst in dead {
            function.remove(block, inst);
        }
    }

    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::middle::ssa::verify::verify_ssa;
    use crate::middle::ssa::{BinOp, BlockId, Op, SlotOrigin, Terminator};

    fn function() -> Function {
        Function::new("f".to_string(), "entry".to_string(), "exit".to_string())
    }

    fn add(function: &mut Function, block: BlockId, lhs: Operand, rhs: Operand) -> ValueId {
        function
            .emit(block, Op::Binary(BinOp::Add, lhs, rhs))
            .expect("an addition defines a value")
    }

    #[test]
    fn a_computation_nobody_reads_is_removed_with_everything_it_reads() {
        // Arrange: `a = 1 + 2; b = a + 3; return 0` -- both are dead, and `a`
        // only because `b` is.
        let mut function = function();
        let entry = function.entry();
        let first = add(&mut function, entry, Operand::Imm(1), Operand::Imm(2));
        add(&mut function, entry, Operand::Value(first), Operand::Imm(3));
        function.set_terminator(entry, Terminator::Return(Some(Operand::Imm(0))));

        // Act
        assert!(run(&mut function));

        // Assert
        assert_eq!(verify_ssa(&function), Ok(()));
        assert!(function.block(entry).insts.is_empty());
    }

    #[test]
    fn a_computation_something_returns_is_kept() {
        // Arrange
        let mut function = function();
        let entry = function.entry();
        let sum = add(&mut function, entry, Operand::Imm(1), Operand::Imm(2));
        function.set_terminator(entry, Terminator::Return(Some(Operand::Value(sum))));

        // Act / Assert
        assert!(!run(&mut function));
        assert_eq!(function.block(entry).insts.len(), 1);
    }

    #[test]
    fn a_call_is_kept_even_when_its_result_is_thrown_away() {
        // Arrange: the effect of a call is not its result.
        let mut function = function();
        let entry = function.entry();
        function.emit(
            entry,
            Op::Call {
                callee: "g".to_string(),
                args: vec![Operand::Imm(1)],
            },
        );
        function.set_terminator(entry, Terminator::Return(None));

        // Act / Assert
        assert!(!run(&mut function));
        assert_eq!(function.block(entry).insts.len(), 1);
    }

    #[test]
    fn a_write_to_memory_is_kept_along_with_the_value_it_writes() {
        // Arrange: `s = 1 + 2; *p = s` where nothing reads `s` afterwards.
        let mut function = function();
        let entry = function.entry();
        let slot = function.slot_for(SlotOrigin::Variable(0));
        let sum = add(&mut function, entry, Operand::Imm(1), Operand::Imm(2));
        function.emit(
            entry,
            Op::SlotStore {
                slot,
                value: Operand::Value(sum),
            },
        );
        function.set_terminator(entry, Terminator::Return(None));

        // Act / Assert: the store keeps the addition alive.
        assert!(!run(&mut function));
        assert_eq!(function.block(entry).insts.len(), 2);
    }

    #[test]
    fn a_phi_nothing_reads_is_removed() {
        // Arrange: a merge whose result is never used.
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
            },
        );
        function.set_terminator(join, Terminator::Return(None));

        let slot = function.slot_for(SlotOrigin::Variable(0));
        function.add_phi(join, slot);
        function.block_mut(join).phis[0].args = vec![Operand::Imm(1), Operand::Imm(2)];

        // Act
        assert!(run(&mut function));

        // Assert
        assert_eq!(verify_ssa(&function), Ok(()));
        assert!(function.block(join).phis.is_empty());
    }

    #[test]
    fn a_branch_is_kept_even_when_both_arms_do_the_same_thing() {
        // Arrange: deciding this one is dead needs control dependence, which
        // this pass deliberately does not compute.
        let mut function = function();
        let entry = function.entry();
        let left = function.add_block("left".to_string());
        let right = function.add_block("right".to_string());
        let join = function.add_block("join".to_string());
        function.set_terminator(left, Terminator::Jump(join));
        function.set_terminator(right, Terminator::Jump(join));
        function.set_terminator(join, Terminator::Return(None));

        let condition = add(&mut function, entry, Operand::Imm(1), Operand::Imm(1));
        function.set_terminator(
            entry,
            Terminator::Branch {
                cond: Operand::Value(condition),
                then_block: left,
                else_block: right,
            },
        );

        // Act / Assert: the condition is live because the branch reads it.
        assert!(!run(&mut function));
        assert_eq!(function.block(entry).insts.len(), 1);
    }

    #[test]
    fn running_it_twice_removes_nothing_the_second_time() {
        // Arrange: the pass pipeline's fixed point depends on this.
        let mut function = function();
        let entry = function.entry();
        add(&mut function, entry, Operand::Imm(1), Operand::Imm(2));
        function.set_terminator(entry, Terminator::Return(None));

        // Act / Assert
        assert!(run(&mut function));
        assert!(!run(&mut function));
    }
}
