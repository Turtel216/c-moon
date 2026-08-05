//! Turning memory into values: where phi nodes go, and what each name means.
//!
//! Construction leaves every variable in memory, so this works on loads and
//! stores rather than on the source program.  A slot that
//! [`promote`](super::promote) cleared
//! is replaced by SSA values: its stores become definitions, its loads become
//! uses of whichever definition reaches them, and the joins where two
//! definitions meet get a phi node.
//!
//! # Semi-pruned placement
//!
//! Phi nodes go at the iterated dominance frontier of the blocks that define a
//! variable, which is the classic Cytron placement.  On its own that is
//! *minimal* SSA, and it places phis for variables that are dead at the join
//! anyway -- every temporary the lowering invents is written and read inside
//! one block, and would still collect a phi at every join that follows it.
//!
//! Restricting placement to the *non-local* names removes those: a variable is
//! non-local if some block reads it before writing it, which is a cheap
//! approximation of "live across a block boundary" and needs no liveness
//! analysis.  A variable that is not non-local is only ever read in a block
//! that has already written it, so no phi of it could ever be read.  That is
//! semi-pruned SSA.
//!
//! Fully pruned SSA needs real liveness and removes a few more; it can come
//! later if the phi count turns out to matter.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use super::{BlockId, Function, Op, SlotId};

/// How a function uses the slots that may be promoted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SlotUsage {
    /// The blocks that store to each slot.
    pub definitions: BTreeMap<SlotId, BTreeSet<BlockId>>,
    /// Slots some block reads before writing, i.e. the ones whose value has to
    /// survive an edge.
    pub non_local: BTreeSet<SlotId>,
}

impl SlotUsage {
    /// Find where `promotable` slots are written, and which of them are read
    /// in a block that has not already written them.
    pub fn compute(function: &Function, promotable: &BTreeSet<SlotId>) -> Self {
        let mut usage = Self::default();

        for block in function.block_ids() {
            // Reset per block: "before being defined" is a question about this
            // block alone.
            let mut written_here: HashSet<SlotId> = HashSet::new();

            for &inst in &function.block(block).insts {
                match &function.inst(inst).op {
                    Op::SlotLoad { slot } if promotable.contains(slot) => {
                        if !written_here.contains(slot) {
                            usage.non_local.insert(*slot);
                        }
                    }
                    Op::SlotStore { slot, .. } if promotable.contains(slot) => {
                        written_here.insert(*slot);
                        usage.definitions.entry(*slot).or_default().insert(block);
                    }
                    _ => {}
                }
            }
        }

        usage
    }
}

/// Where each slot needs a phi node.
///
/// # Arguments
///
/// * `frontiers` - the dominance frontier of every block, by block index
/// * `usage` - where the slots are written, and which of them are non-local
///
/// # Returns
///
/// The blocks needing a phi for each slot, in block order, so that placement
/// -- and the order phis end up in within a block -- is reproducible.
pub fn phi_placement(
    frontiers: &[Vec<BlockId>],
    usage: &SlotUsage,
) -> BTreeMap<SlotId, BTreeSet<BlockId>> {
    let mut placement = BTreeMap::new();

    for (&slot, defined_in) in &usage.definitions {
        // A name no block reads before writing cannot be read through a phi,
        // so it needs none. This is what makes the placement semi-pruned.
        if !usage.non_local.contains(&slot) {
            continue;
        }

        let mut needs_phi: BTreeSet<BlockId> = BTreeSet::new();
        let mut worklist: Vec<BlockId> = defined_in.iter().copied().collect();

        while let Some(block) = worklist.pop() {
            for &frontier in &frontiers[block.index()] {
                if !needs_phi.insert(frontier) {
                    continue;
                }
                // The phi is itself a definition, so the frontier of the block
                // it lands in may need one too -- which is what makes this the
                // *iterated* dominance frontier.
                if !defined_in.contains(&frontier) {
                    worklist.push(frontier);
                }
            }
        }

        if !needs_phi.is_empty() {
            placement.insert(slot, needs_phi);
        }
    }

    placement
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::middle::ssa::dom::{DomTree, Graph};
    use crate::middle::ssa::{Operand, SlotOrigin, Terminator};

    fn block(index: usize) -> BlockId {
        BlockId::from_index(index)
    }

    /// A hand-built graph, owning the adjacency lists the dominance code
    /// borrows.
    struct Edges {
        predecessors: Vec<Vec<BlockId>>,
        successors: Vec<Vec<BlockId>>,
    }

    impl Edges {
        fn new(count: usize, edges: &[(usize, usize)]) -> Self {
            let mut this = Self {
                predecessors: vec![Vec::new(); count],
                successors: vec![Vec::new(); count],
            };
            for &(from, to) in edges {
                this.successors[from].push(block(to));
                this.predecessors[to].push(block(from));
            }
            this
        }

        /// The dominance frontier of every block.
        fn frontiers(&self) -> Vec<Vec<BlockId>> {
            let graph = Graph::new(block(0), &self.predecessors, &self.successors);
            DomTree::build(graph).frontiers(graph)
        }
    }

    /// Usage in which `slot` is non-local and written in `blocks`.
    fn written_in(slot: usize, blocks: &[usize]) -> SlotUsage {
        let slot = SlotId::from_index(slot);
        SlotUsage {
            definitions: BTreeMap::from([(slot, blocks.iter().map(|&b| block(b)).collect())]),
            non_local: BTreeSet::from([slot]),
        }
    }

    /// The blocks placed for `slot`, by index.
    fn placed(placement: &BTreeMap<SlotId, BTreeSet<BlockId>>, slot: usize) -> Vec<usize> {
        placement
            .get(&SlotId::from_index(slot))
            .map(|blocks| blocks.iter().map(|&b| b.index()).collect())
            .unwrap_or_default()
    }

    #[test]
    fn a_variable_written_in_both_arms_gets_a_phi_at_the_join() {
        // Arrange: a diamond, written in each arm.
        let edges = Edges::new(4, &[(0, 1), (0, 2), (1, 3), (2, 3)]);
        let usage = written_in(0, &[1, 2]);

        // Act
        let placement = phi_placement(&edges.frontiers(), &usage);

        // Assert
        assert_eq!(placed(&placement, 0), vec![3]);
    }

    #[test]
    fn a_variable_written_in_one_arm_still_gets_a_phi_at_the_join() {
        // Arrange: the other arm carries the value from before the branch, so
        // two definitions still meet.
        let edges = Edges::new(4, &[(0, 1), (0, 2), (1, 3), (2, 3)]);
        let usage = written_in(0, &[0, 1]);

        // Act / Assert
        assert_eq!(
            placed(&phi_placement(&edges.frontiers(), &usage), 0),
            vec![3]
        );
    }

    #[test]
    fn a_variable_written_in_a_loop_gets_a_phi_at_the_header() {
        // Arrange: `0 -> 1`, `1 -> 2 -> 1`, `1 -> 3`, written before the loop
        // and again inside it.
        let edges = Edges::new(4, &[(0, 1), (1, 2), (2, 1), (1, 3)]);
        let usage = written_in(0, &[0, 2]);

        // Act / Assert: the header is where the initial value and the one from
        // the previous iteration meet.
        assert_eq!(
            placed(&phi_placement(&edges.frontiers(), &usage), 0),
            vec![1]
        );
    }

    #[test]
    fn a_phi_can_force_another_phi_further_on() {
        // Arrange: a diamond nested in the left arm of another one, with the
        // variable written only in the inner left arm:
        //
        //   0 -> {1 -> {3, 4} -> 5 -> 6, 2 -> 6}
        //
        // The phi the inner join needs is itself a definition, and it does not
        // dominate the outer join, so that needs one too -- the iterated
        // dominance frontier rather than the frontier.
        let edges = Edges::new(
            7,
            &[
                (0, 1),
                (0, 2),
                (1, 3),
                (1, 4),
                (3, 5),
                (4, 5),
                (5, 6),
                (2, 6),
            ],
        );
        let usage = written_in(0, &[3]);

        // Act / Assert
        assert_eq!(
            placed(&phi_placement(&edges.frontiers(), &usage), 0),
            vec![5, 6]
        );
    }

    #[test]
    fn nested_loops_get_a_phi_at_each_header() {
        // Arrange: an inner loop `2 -> 3 -> 2` inside an outer one
        // `1 -> 2 -> 4 -> 1`, written in the inner body.
        let edges = Edges::new(6, &[(0, 1), (1, 2), (2, 3), (3, 2), (2, 4), (4, 1), (1, 5)]);
        let usage = written_in(0, &[3]);

        // Act / Assert
        assert_eq!(
            placed(&phi_placement(&edges.frontiers(), &usage), 0),
            vec![1, 2]
        );
    }

    #[test]
    fn a_variable_that_never_crosses_a_block_boundary_gets_no_phi() {
        // Arrange: the same diamond, but the variable is read only in blocks
        // that have already written it -- every temporary the lowering
        // invents looks like this.
        let edges = Edges::new(4, &[(0, 1), (0, 2), (1, 3), (2, 3)]);
        let usage = SlotUsage {
            definitions: BTreeMap::from([(
                SlotId::from_index(0),
                BTreeSet::from([block(1), block(2)]),
            )]),
            non_local: BTreeSet::new(),
        };

        // Act / Assert: minimal SSA would place one at the join and leave it
        // dead; semi-pruned placement does not.
        assert!(phi_placement(&edges.frontiers(), &usage).is_empty());
    }

    #[test]
    fn a_slot_read_before_it_is_written_in_its_block_is_non_local() {
        // Arrange: block 0 writes `s` then reads it; block 1 reads it first.
        let mut function = Function::new("f".to_string(), "entry".to_string(), "exit".to_string());
        let entry = function.entry();
        let second = function.add_block("second".to_string());
        function.set_terminator(entry, Terminator::Jump(second));

        let local = function.slot_for(SlotOrigin::Temporary("t1".to_string()));
        let crossing = function.slot_for(SlotOrigin::Variable(0));

        function.emit(
            entry,
            Op::SlotStore {
                slot: local,
                value: Operand::Imm(1),
            },
        );
        function.emit(entry, Op::SlotLoad { slot: local });
        function.emit(
            entry,
            Op::SlotStore {
                slot: crossing,
                value: Operand::Imm(2),
            },
        );
        function.emit(second, Op::SlotLoad { slot: crossing });

        // Act
        let promotable = BTreeSet::from([local, crossing]);
        let usage = SlotUsage::compute(&function, &promotable);

        // Assert: both are written, but only the one read in a block that did
        // not write it needs its value carried across an edge.
        assert_eq!(
            usage.definitions.keys().copied().collect::<Vec<_>>(),
            vec![local, crossing]
        );
        assert_eq!(usage.non_local, BTreeSet::from([crossing]));
    }

    #[test]
    fn slots_that_may_not_be_promoted_are_ignored_entirely() {
        // Arrange: a slot that failed the promotion gate is memory, and stays
        // memory -- it must not appear in either set.
        let mut function = Function::new("f".to_string(), "entry".to_string(), "exit".to_string());
        let entry = function.entry();
        let pinned = function.slot_for(SlotOrigin::Variable(0));
        function.emit(
            entry,
            Op::SlotStore {
                slot: pinned,
                value: Operand::Imm(1),
            },
        );
        function.emit(entry, Op::SlotLoad { slot: pinned });

        // Act
        let usage = SlotUsage::compute(&function, &BTreeSet::new());

        // Assert
        assert_eq!(usage, SlotUsage::default());
    }
}
