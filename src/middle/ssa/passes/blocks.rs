//! Block merging: one block where the control flow only ever went one way.
//!
//! A block that jumps to a block nothing else jumps to is two blocks
//! describing one straight line of code.  Joining them costs nothing and saves
//! the jump, and it gives the later passes longer blocks to work within.
//!
//! Lowering leaves a great many of these -- every `if` and every loop is built
//! out of blocks that exist to be branch targets -- and constant propagation
//! makes more of them by turning branches into jumps.
//!
//! A block still carrying phi nodes is left alone even when it has one way in.
//! A phi with a single argument is a copy, and [`copyprop`](super::copyprop)
//! removes it; the pipeline runs that first, so the merge happens on the next
//! round rather than being duplicated here.

use crate::middle::ssa::{BlockId, Function, Terminator};

/// Join every pair of blocks that only ever run one after the other.
///
/// # Returns
///
/// Whether anything was merged.  Running it twice over the same function
/// merges nothing the second time.
pub fn run(function: &mut Function) -> bool {
    let mut merged = false;

    // One merge can enable another -- the absorbing block inherits a jump that
    // may itself be mergeable -- so this looks again after each one. Block ids
    // stay valid throughout, because nothing is deleted until the end.
    while let Some((first, second)) = next_merge(function) {
        function.merge_blocks(first, second);
        merged = true;
    }

    if merged {
        // The absorbed blocks are now unreachable.
        function.retain_reachable();
    }
    merged
}

/// A block and the successor it can absorb, if there is such a pair.
fn next_merge(function: &Function) -> Option<(BlockId, BlockId)> {
    function.block_ids().find_map(|block| {
        let Terminator::Jump(successor) = *function.block(block).terminator() else {
            return None;
        };

        // A block that jumps to itself is a loop, not a straight line.
        let mergeable = successor != block
            && function.block(successor).preds() == [block]
            && function.block(successor).phis.is_empty();

        mergeable.then_some((block, successor))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::middle::ir::Width;

    use crate::middle::ssa::verify::verify_ssa;
    use crate::middle::ssa::{Op, Operand, SlotOrigin};

    fn function() -> Function {
        Function::new("f".to_string(), "entry".to_string(), "exit".to_string())
    }

    fn labels(function: &Function) -> Vec<String> {
        function
            .block_ids()
            .map(|block| function.block(block).label.clone())
            .collect()
    }

    #[test]
    fn a_block_entered_one_way_joins_the_block_that_enters_it() {
        // Arrange: `entry -> middle -> last`, each entered only from the one
        // before it.
        let mut function = function();
        let entry = function.entry();
        let middle = function.add_block("middle".to_string());
        let last = function.add_block("last".to_string());

        function.emit(entry, Op::Copy(Operand::Imm(1)));
        function.emit(middle, Op::Copy(Operand::Imm(2)));
        let result = function
            .emit(last, Op::Copy(Operand::Imm(3)))
            .expect("a copy defines a value");

        function.set_terminator(entry, Terminator::Jump(middle));
        function.set_terminator(middle, Terminator::Jump(last));
        function.set_terminator(last, Terminator::Return(Some(Operand::Value(result))));

        // Act
        assert!(run(&mut function));

        // Assert: one block, holding all three instructions in order.
        assert_eq!(verify_ssa(&function), Ok(()));
        assert_eq!(labels(&function), vec!["entry".to_string()]);
        assert_eq!(function.block(function.entry()).insts.len(), 3);
    }

    #[test]
    fn a_block_entered_from_two_places_is_left_alone() {
        // Arrange: a join, which is exactly what must not be absorbed.
        let mut function = function();
        let entry = function.entry();
        let left = function.add_block("left".to_string());
        let join = function.add_block("join".to_string());

        function.set_terminator(left, Terminator::Jump(join));
        function.set_terminator(
            entry,
            Terminator::Branch {
                cond: Operand::Imm(1),
                then_block: left,
                else_block: join,
                width: Width::Bits64,
            },
        );
        function.set_terminator(join, Terminator::Return(None));

        // Act / Assert: `left` cannot absorb `join`, and `entry` branches, so
        // it can absorb nothing either.
        assert!(!run(&mut function));
        assert_eq!(function.block_count(), 3);
    }

    #[test]
    fn a_block_that_jumps_to_itself_is_left_alone() {
        // Arrange: an endless loop, whose single predecessor is itself.
        let mut function = function();
        let entry = function.entry();
        let spin = function.add_block("spin".to_string());
        function.set_terminator(entry, Terminator::Jump(spin));
        function.set_terminator(spin, Terminator::Jump(spin));

        // Act / Assert: `entry` cannot absorb `spin`, which is entered twice,
        // and `spin` must not absorb itself.
        assert!(!run(&mut function));
        assert_eq!(function.block_count(), 2);
    }

    #[test]
    fn a_block_with_phi_nodes_left_is_not_merged_yet() {
        // Arrange: a phi in a block with one way in is a copy, and copy
        // propagation is what removes it.
        let mut function = function();
        let entry = function.entry();
        let second = function.add_block("second".to_string());
        function.set_terminator(entry, Terminator::Jump(second));
        function.set_terminator(second, Terminator::Return(None));

        let slot = function.slot_for(SlotOrigin::Variable(0));
        function.add_phi(second, slot);
        function.block_mut(second).phis[0].args = vec![Operand::Imm(1)];

        // Act / Assert
        assert!(!run(&mut function));
        assert_eq!(function.block_count(), 2);
    }

    #[test]
    fn merging_keeps_the_phi_arguments_of_the_blocks_it_leads_to() {
        // Arrange: `entry -> middle`, and `middle` branches into a loop whose
        // header carries a phi. Absorbing `middle` moves an edge, and the phi
        // argument belonging to it has to move with it.
        let mut function = function();
        let entry = function.entry();
        let middle = function.add_block("middle".to_string());
        let header = function.add_block("header".to_string());
        let latch = function.add_block("latch".to_string());
        let done = function.add_block("done".to_string());

        function.set_terminator(entry, Terminator::Jump(middle));
        function.set_terminator(middle, Terminator::Jump(header));
        function.set_terminator(latch, Terminator::Jump(header));
        function.set_terminator(
            header,
            Terminator::Branch {
                cond: Operand::Imm(1),
                then_block: latch,
                else_block: done,
                width: Width::Bits64,
            },
        );

        let slot = function.slot_for(SlotOrigin::Variable(0));
        let carried = function.add_phi(header, slot);
        function.block_mut(header).phis[0].args = vec![Operand::Imm(7), Operand::Value(carried)];
        function.set_terminator(done, Terminator::Return(Some(Operand::Value(carried))));

        // Act
        assert!(run(&mut function));

        // Assert: the argument that arrived from `middle` now arrives from
        // `entry`, at the same position.
        assert_eq!(verify_ssa(&function), Ok(()));
        let header = function
            .block_ids()
            .find(|&block| function.block(block).label == "header")
            .expect("the header survives");
        let position = function
            .block(header)
            .preds()
            .iter()
            .position(|&pred| function.block(pred).label == "entry")
            .expect("the entry now enters the header directly");
        assert_eq!(
            function.block(header).phis[0].args[position],
            Operand::Imm(7)
        );
    }

    #[test]
    fn running_it_twice_merges_nothing_the_second_time() {
        // Arrange: the pass pipeline's fixed point depends on this.
        let mut function = function();
        let entry = function.entry();
        let second = function.add_block("second".to_string());
        function.set_terminator(entry, Terminator::Jump(second));
        function.set_terminator(second, Terminator::Return(None));

        // Act / Assert
        assert!(run(&mut function));
        assert!(!run(&mut function));
    }
}
