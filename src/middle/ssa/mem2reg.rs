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

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::frontend::renamer::VarId;

use super::dom::{DomTree, Graph};
use super::{BlockId, DefSite, Function, Op, Operand, SlotId, SlotOrigin, SourceName, ValueId};

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
                    Op::SlotLoad { slot, .. } if promotable.contains(slot) => {
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

/// Replace every promotable slot with SSA values.
///
/// # Arguments
///
/// * `function` - a function in the all-in-memory form construction produces
/// * `promotable` - the slots the promotion gate cleared
///
/// After this, a promoted slot has no loads or stores left and no longer
/// appears anywhere; the values that used to flow through it are read
/// directly.  Slots that were not promoted are untouched.
pub fn promote_slots(function: &mut Function, promotable: &BTreeSet<SlotId>) {
    if promotable.is_empty() {
        return;
    }

    let usage = SlotUsage::compute(function, promotable);

    let tree = {
        // The adjacency lists borrow nothing from `function` once built, so
        // the tree outlives them and the function can be mutated below.
        let (predecessors, successors) = function.adjacency();
        let graph = Graph::new(function.entry(), &predecessors, &successors);
        let tree = DomTree::build(graph);
        let frontiers = tree.frontiers(graph);
        let placement = phi_placement(&frontiers, &usage);

        // Phi nodes go in before renaming, so that renaming finds them and
        // fills in their arguments as it passes each predecessor.
        for (&slot, blocks) in &placement {
            for &block in blocks {
                function.add_phi(block, slot);
            }
        }
        tree
    };

    Renaming::new(function, promotable).run(&tree);
}

/// The renaming walk: what each name means at each point of the function.
struct Renaming<'a> {
    function: &'a mut Function,
    promotable: &'a BTreeSet<SlotId>,
    /// The definition currently in scope for each slot, innermost last.
    stacks: HashMap<SlotId, Vec<Operand>>,
    /// What to read instead of the value a deleted load produced.
    substitutions: HashMap<ValueId, Operand>,
    /// The value standing for "never written on this path".
    undefined: Operand,
    undefined_used: bool,
    /// Next version number for each source variable, for readable dumps.
    versions: HashMap<VarId, u32>,
}

impl<'a> Renaming<'a> {
    fn new(function: &'a mut Function, promotable: &'a BTreeSet<SlotId>) -> Self {
        // Created up front rather than on demand: inserting into a block while
        // its instruction list is being rebuilt would lose the instruction.
        // It is removed again below if nothing turned out to need it.
        let entry = function.entry();
        let prologue = function
            .block(entry)
            .insts
            .iter()
            .take_while(|&&inst| matches!(function.inst(inst).op, Op::GetParam(_)))
            .count();
        let undefined = function
            .insert(entry, prologue, Op::Undef)
            .expect("undef defines a value");

        Self {
            function,
            promotable,
            stacks: HashMap::new(),
            substitutions: HashMap::new(),
            undefined: Operand::Value(undefined),
            undefined_used: false,
            versions: HashMap::new(),
        }
    }

    /// Walk the dominator tree, renaming as it goes.
    ///
    /// Preorder is what makes a stack the right structure: a block is visited
    /// after everything that dominates it, so whatever is on top of a slot's
    /// stack when the block is reached is exactly the definition that reaches
    /// it.  The pushes a block made are undone on the way back out.
    fn run(mut self, tree: &DomTree) {
        let children = tree.children();

        // An explicit stack rather than recursion, for the same reason the
        // rest of the module avoids it: a deep enough function would otherwise
        // overflow the compiler's own stack.
        let mut steps = vec![Step::Enter(self.function.entry())];
        let mut pushed: Vec<Vec<SlotId>> = vec![Vec::new(); self.function.block_count()];

        while let Some(step) = steps.pop() {
            match step {
                Step::Enter(block) => {
                    pushed[block.index()] = self.rename_block(block);
                    steps.push(Step::Leave(block));
                    // Reversed so the first child is dealt with first, which
                    // keeps version numbers in a readable order.
                    for &child in children[block.index()].iter().rev() {
                        steps.push(Step::Enter(child));
                    }
                }
                Step::Leave(block) => {
                    for slot in pushed[block.index()].drain(..) {
                        self.stacks
                            .get_mut(&slot)
                            .expect("Compiler Bug: popping a slot that was never pushed")
                            .pop();
                    }
                }
            }
        }

        if !self.undefined_used {
            let entry = self.function.entry();
            if let Operand::Value(value) = self.undefined
                && let DefSite::Inst(inst) = self.function.value_def(value).site
            {
                self.function.remove(entry, inst);
            }
        }
    }

    /// Rename one block, and fill in the phi arguments its outgoing edges owe.
    ///
    /// # Returns
    ///
    /// The slots this block pushed a definition for, to be popped on the way
    /// back out of the dominator tree.
    fn rename_block(&mut self, block: BlockId) -> Vec<SlotId> {
        let mut pushed = Vec::new();

        // A phi is a definition of its slot, in scope for the whole block.
        let phis: Vec<(SlotId, ValueId)> = self
            .function
            .block(block)
            .phis
            .iter()
            .map(|phi| (phi.slot, phi.dest))
            .collect();
        for (slot, dest) in phis {
            self.name(slot, dest);
            self.stacks
                .entry(slot)
                .or_default()
                .push(Operand::Value(dest));
            pushed.push(slot);
        }

        // Rust note: `mem::take` moves the instruction list out, so the loop
        // can mutate the instructions it names without holding a borrow of the
        // block itself.
        let insts = std::mem::take(&mut self.function.block_mut(block).insts);
        let mut kept = Vec::with_capacity(insts.len());

        for inst in insts {
            for operand in self.function.inst_mut(inst).op.operands_mut() {
                if let Operand::Value(value) = *operand
                    && let Some(&replacement) = self.substitutions.get(&value)
                {
                    *operand = replacement;
                }
            }

            let action = match &self.function.inst(inst).op {
                Op::SlotLoad { slot, .. } if self.promotable.contains(slot) => Action::Load(*slot),
                Op::SlotStore { slot, value, .. } if self.promotable.contains(slot) => {
                    Action::Store(*slot, *value)
                }
                _ => Action::Keep,
            };

            match action {
                // The load disappears: everything that read its result reads
                // the definition that reaches it instead.
                Action::Load(slot) => {
                    let current = self.current(slot);
                    let dest = self
                        .function
                        .inst(inst)
                        .dest
                        .expect("Compiler Bug: a load defines a value");
                    self.substitutions.insert(dest, current);
                }
                // The store disappears too: it defines the name from here on,
                // which is what the stack records.
                Action::Store(slot, value) => {
                    if let Operand::Value(value) = value {
                        self.name(slot, value);
                    }
                    self.stacks.entry(slot).or_default().push(value);
                    pushed.push(slot);
                }
                Action::Keep => kept.push(inst),
            }
        }

        self.function.block_mut(block).insts = kept;

        for operand in self.function.block_mut(block).terminator_operands_mut() {
            if let Operand::Value(value) = *operand
                && let Some(&replacement) = self.substitutions.get(&value)
            {
                *operand = replacement;
            }
        }

        self.fill_successor_phis(block);
        pushed
    }

    /// Give each phi of each successor the value arriving from this block.
    fn fill_successor_phis(&mut self, block: BlockId) {
        let successors: Vec<BlockId> = self.function.block(block).successors().collect();

        for successor in successors {
            // Every position, not just the first: a block can reach the same
            // successor along more than one edge, and each edge has its own
            // argument.
            let positions: Vec<usize> = self
                .function
                .block(successor)
                .preds()
                .iter()
                .enumerate()
                .filter(|&(_, &pred)| pred == block)
                .map(|(position, _)| position)
                .collect();

            for index in 0..self.function.block(successor).phis.len() {
                let slot = self.function.block(successor).phis[index].slot;
                let current = self.current(slot);
                for &position in &positions {
                    self.function.block_mut(successor).phis[index].args[position] = current;
                }
            }
        }
    }

    /// The definition of `slot` in scope here, or the undefined value when the
    /// program reads a variable it never wrote.
    fn current(&mut self, slot: SlotId) -> Operand {
        match self.stacks.get(&slot).and_then(|stack| stack.last()) {
            Some(&operand) => operand,
            None => {
                self.undefined_used = true;
                self.undefined
            }
        }
    }

    /// Record that `value` is the next version of the variable `slot` stands
    /// for, so dumps and diagnostics can name it.
    fn name(&mut self, slot: SlotId, value: ValueId) {
        let SlotOrigin::Variable(variable) = self.function.slot(slot).origin else {
            return;
        };
        if self.function.value_def(value).source.is_some() {
            return;
        }

        let version = self.versions.entry(variable).or_default();
        self.function.name_value(
            value,
            SourceName {
                variable,
                version: *version,
            },
        );
        *version += 1;
    }
}

/// What renaming does with one instruction.
enum Action {
    /// A load of a promoted slot, which disappears.
    Load(SlotId),
    /// A store to a promoted slot, which disappears.
    Store(SlotId, Operand),
    /// Anything else.
    Keep,
}

/// One step of the walk over the dominator tree.
enum Step {
    Enter(BlockId),
    Leave(BlockId),
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::middle::ir::Width;

    use crate::middle::ssa::Terminator;
    use crate::middle::ssa::dom::{DomTree, Graph};

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
                width: Width::Bits64,
            },
        );
        function.emit(
            entry,
            Op::SlotLoad {
                slot: local,
                width: Width::Bits64,
            },
        );
        function.emit(
            entry,
            Op::SlotStore {
                slot: crossing,
                value: Operand::Imm(2),
                width: Width::Bits64,
            },
        );
        function.emit(
            second,
            Op::SlotLoad {
                slot: crossing,
                width: Width::Bits64,
            },
        );

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
                width: Width::Bits64,
            },
        );
        function.emit(
            entry,
            Op::SlotLoad {
                slot: pinned,
                width: Width::Bits64,
            },
        );

        // Act
        let usage = SlotUsage::compute(&function, &BTreeSet::new());

        // Assert
        assert_eq!(usage, SlotUsage::default());
    }
}
