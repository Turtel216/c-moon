//! Dominance: reverse postorder, the dominator tree and dominance frontiers.
//!
//! Block `a` *dominates* block `b` when every path from the entry to `b` runs
//! through `a`.  SSA construction is built on three consequences of that
//! relation: the order blocks are visited in (reverse postorder), the tree of
//! immediate dominators, and the *dominance frontier* of each block -- the
//! blocks where two different definitions can first meet, which is exactly
//! where phi nodes belong.
//!
//! The tree is built with the Cooper-Harvey-Kennedy iterative algorithm ("A
//! Simple, Fast Dominance Algorithm", 2001).  Lengauer-Tarjan is
//! asymptotically faster and much harder to get right; at the size of the
//! functions this compiler sees, the iterative version wins on clarity and, in
//! practice, on speed as well.
//!
//! # Unreachable blocks must be deleted first
//!
//! **Every block must be reachable from the entry before dominance is
//! computed.**  A block no path reaches has no immediate dominator -- there
//! are no paths to intersect -- so the fixed point below has no answer to give
//! for it.  Callers delete unreachable blocks first; [`DomTree::build`] panics
//! rather than inventing an answer.
//!
//! # Graphs, not IR
//!
//! Everything here works on plain adjacency lists rather than on any
//! particular CFG type, so one implementation serves the TAC CFG, the SSA
//! function, and -- with the two lists exchanged -- post-dominance.

use std::collections::BTreeSet;

use crate::middle::ssa::BlockId;

/// A read-only view of a control-flow graph as adjacency lists.
///
/// Both lists are indexed by [`BlockId::index`], so a graph of `n` blocks has
/// `n` entries in each.  The two must describe the same edge set: an edge
/// `a -> b` appears in `successors[a]` and in `predecessors[b]`.  Parallel
/// edges -- a conditional branch whose two targets are the same block -- are
/// repeated entries, and dominance treats them exactly as it treats a single
/// edge.
#[derive(Debug, Clone, Copy)]
pub struct Graph<'a> {
    entry: BlockId,
    predecessors: &'a [Vec<BlockId>],
    successors: &'a [Vec<BlockId>],
}

impl<'a> Graph<'a> {
    /// View a pair of adjacency lists as a control-flow graph.
    ///
    /// # Arguments
    ///
    /// * `entry` - the block execution starts at
    /// * `predecessors` - incoming edges of each block, by block index
    /// * `successors` - outgoing edges of each block, by block index
    ///
    /// # Panics
    ///
    /// Panics if the two lists disagree about how many blocks there are, or if
    /// `entry` is not one of them.
    pub fn new(
        entry: BlockId,
        predecessors: &'a [Vec<BlockId>],
        successors: &'a [Vec<BlockId>],
    ) -> Self {
        assert_eq!(
            predecessors.len(),
            successors.len(),
            "Compiler Bug: predecessor and successor lists describe different graphs"
        );
        assert!(
            entry.index() < successors.len(),
            "Compiler Bug: entry block {:?} is not in the graph",
            entry
        );
        Self {
            entry,
            predecessors,
            successors,
        }
    }

    /// The block execution starts at.
    pub fn entry(self) -> BlockId {
        self.entry
    }

    /// How many blocks the graph has.
    pub fn block_count(self) -> usize {
        self.successors.len()
    }

    /// The blocks `block` can transfer control to.
    pub fn successors(self, block: BlockId) -> &'a [BlockId] {
        &self.successors[block.index()]
    }

    /// The blocks that can transfer control to `block`.
    pub fn predecessors(self, block: BlockId) -> &'a [BlockId] {
        &self.predecessors[block.index()]
    }
}

/// The blocks reachable from the entry, in reverse postorder.
///
/// Reverse postorder is the order SSA construction and every forward dataflow
/// analysis want: a block appears after every predecessor that is not reached
/// through a back edge, so one pass over the list already propagates
/// information as far as an acyclic graph allows.
///
/// # Returns
///
/// Reachable blocks only.  A shorter result than the graph has blocks means
/// there is unreachable code, which is how [`DomTree::build`] detects it.
pub fn reverse_postorder(graph: Graph<'_>) -> Vec<BlockId> {
    let mut visited = vec![false; graph.block_count()];
    let mut order = Vec::with_capacity(graph.block_count());

    // An explicit stack rather than recursion: a deeply nested function would
    // otherwise be able to overflow the compiler's own stack.  Each entry is a
    // block together with how many of its successors have been dealt with.
    let mut stack = vec![(graph.entry(), 0usize)];
    visited[graph.entry().index()] = true;

    while !stack.is_empty() {
        let top = stack.len() - 1;
        let (block, next) = stack[top];

        match graph.successors(block).get(next) {
            Some(&successor) => {
                stack[top].1 = next + 1;
                if !visited[successor.index()] {
                    visited[successor.index()] = true;
                    stack.push((successor, 0));
                }
            }
            // Every successor has been visited, so the block is finished and
            // takes its place in postorder.
            None => {
                order.push(block);
                stack.pop();
            }
        }
    }

    order.reverse();
    order
}

/// The dominator tree of one control-flow graph, plus the numbering it was
/// built from.
///
/// # Invalidation
///
/// A `DomTree` is a snapshot of the graph it was built from.  **Adding or
/// removing a block or an edge invalidates it completely** -- the preorder
/// intervals [`DomTree::dominates`] answers from are numbered over the whole
/// tree, so a single new edge can renumber every block.  Rebuild it; never
/// patch it, and never hold one across a mutation of the graph.
#[derive(Debug, Clone)]
pub struct DomTree {
    /// Immediate dominator of each block, by block index.  The entry block is
    /// its own immediate dominator, which is what makes the fixed point below
    /// terminate without a special case.
    immediate: Vec<BlockId>,
    /// The blocks in reverse postorder.
    order: Vec<BlockId>,
    /// Position of each block within `order`, by block index.
    order_number: Vec<u32>,
    /// Preorder stamps over the dominator tree: `enter[a] <= enter[b]` and
    /// `exit[b] <= exit[a]` exactly when `a` dominates `b`.
    enter: Vec<u32>,
    exit: Vec<u32>,
}

impl DomTree {
    /// Build the dominator tree of `graph`.
    ///
    /// The algorithm is Cooper-Harvey-Kennedy: initialise every block's
    /// immediate dominator to "unknown", then repeatedly recompute each
    /// block's as the intersection of its predecessors' dominator chains,
    /// visiting blocks in reverse postorder, until nothing changes.  Because
    /// reverse postorder puts at least one predecessor of every reachable
    /// block before it, the first sweep already gets most of the answer and
    /// the fixed point converges in two or three passes -- including on
    /// irreducible graphs, which need no special handling here.
    ///
    /// # Panics
    ///
    /// Panics if any block is unreachable from the entry.  Unreachable blocks
    /// have no dominator to compute and must be deleted before this runs; see
    /// the module documentation.
    pub fn build(graph: Graph<'_>) -> Self {
        let count = graph.block_count();
        let order = reverse_postorder(graph);
        assert_eq!(
            order.len(),
            count,
            "Compiler Bug: dominance was computed over a graph with {} unreachable block(s); \
             delete them first",
            count - order.len()
        );

        let mut order_number = vec![0u32; count];
        for (position, &block) in order.iter().enumerate() {
            order_number[block.index()] = position as u32;
        }

        // `None` is CHK's "undefined": a block whose dominator has not been
        // computed yet contributes nothing to an intersection.
        let mut immediate: Vec<Option<BlockId>> = vec![None; count];
        immediate[graph.entry().index()] = Some(graph.entry());

        let mut changed = true;
        while changed {
            changed = false;

            // The entry is skipped: it dominates itself and nothing else can
            // improve on that, and folding its incoming back edges into the
            // intersection would only undo it.
            for &block in order.iter().skip(1) {
                let mut new_idom: Option<BlockId> = None;

                for &predecessor in graph.predecessors(block) {
                    if immediate[predecessor.index()].is_none() {
                        continue;
                    }
                    new_idom = Some(match new_idom {
                        None => predecessor,
                        Some(current) => intersect(predecessor, current, &immediate, &order_number),
                    });
                }

                if immediate[block.index()] != new_idom {
                    immediate[block.index()] = new_idom;
                    changed = true;
                }
            }
        }

        // Reverse postorder puts at least one predecessor of every reachable
        // block ahead of it, and reachability was checked above, so every
        // block came out of the fixed point with a dominator.
        let immediate: Vec<BlockId> = immediate
            .into_iter()
            .map(|idom| idom.expect("Compiler Bug: reachable block left without a dominator"))
            .collect();

        let (enter, exit) = stamp_preorder(&immediate, &order, graph.entry());

        Self {
            immediate,
            order,
            order_number,
            enter,
            exit,
        }
    }

    /// The immediate dominator of `block`: the last block every path to it has
    /// in common.
    ///
    /// # Returns
    ///
    /// `None` for the entry block, which has no dominator other than itself.
    pub fn immediate_dominator(&self, block: BlockId) -> Option<BlockId> {
        let idom = self.immediate[block.index()];
        (idom != block).then_some(idom)
    }

    /// The blocks in reverse postorder.
    pub fn reverse_postorder(&self) -> &[BlockId] {
        &self.order
    }

    /// This block's position in reverse postorder.
    pub fn order_number(&self, block: BlockId) -> u32 {
        self.order_number[block.index()]
    }

    /// Does `a` dominate `b`?
    ///
    /// Dominance is reflexive: a block dominates itself.  The query is O(1) --
    /// `a` dominates `b` exactly when `b` sits inside `a`'s subtree of the
    /// dominator tree, which the preorder stamps decide with two comparisons.
    /// See the type's documentation for when those stamps stop being valid.
    pub fn dominates(&self, a: BlockId, b: BlockId) -> bool {
        self.enter[a.index()] <= self.enter[b.index()]
            && self.exit[b.index()] <= self.exit[a.index()]
    }

    /// The dominance frontier of every block, by block index.
    ///
    /// The frontier of `a` is the set of blocks `a` does *not* dominate but
    /// has an edge into from a block it does dominate: the earliest points
    /// where a definition made in `a` can meet a definition made elsewhere.
    /// Phi nodes for a variable go at the iterated frontier of the blocks that
    /// define it.
    ///
    /// This is the Cooper-Harvey-Kennedy formulation: only a block with
    /// several predecessors can be in anyone's frontier, so for each such
    /// block the walk climbs from each predecessor up to its immediate
    /// dominator, adding it to the frontier of everything on the way.
    ///
    /// # Arguments
    ///
    /// * `graph` - the same graph this tree was built from
    ///
    /// # Returns
    ///
    /// One ascending, duplicate-free list per block, so phi placement is
    /// deterministic.
    pub fn frontiers(&self, graph: Graph<'_>) -> Vec<Vec<BlockId>> {
        let mut frontiers: Vec<BTreeSet<BlockId>> = vec![BTreeSet::new(); graph.block_count()];

        for block in (0..graph.block_count()).map(BlockId::from_index) {
            let predecessors = graph.predecessors(block);
            if predecessors.len() < 2 {
                continue;
            }

            let idom = self.immediate[block.index()];
            for &predecessor in predecessors {
                let mut runner = predecessor;
                // Everything from the predecessor up to (but excluding) the
                // block's immediate dominator can reach `block` without
                // dominating it.
                while runner != idom {
                    frontiers[runner.index()].insert(block);
                    runner = self.immediate[runner.index()];
                }
            }
        }

        frontiers
            .into_iter()
            .map(|frontier| frontier.into_iter().collect())
            .collect()
    }
}

/// The nearest block that dominates both `a` and `b`.
///
/// Walking up from whichever of the two is deeper in reverse postorder makes
/// the two chains meet: a block's immediate dominator always has a strictly
/// smaller reverse-postorder number, so each step shortens the distance.
fn intersect(
    mut a: BlockId,
    mut b: BlockId,
    immediate: &[Option<BlockId>],
    order_number: &[u32],
) -> BlockId {
    /// One step up the dominator chain of a block already computed.
    fn climb(block: BlockId, immediate: &[Option<BlockId>]) -> BlockId {
        immediate[block.index()].expect("Compiler Bug: climbed into an uncomputed dominator")
    }

    while a != b {
        while order_number[a.index()] > order_number[b.index()] {
            a = climb(a, immediate);
        }
        while order_number[b.index()] > order_number[a.index()] {
            b = climb(b, immediate);
        }
    }
    a
}

/// Number the dominator tree in preorder, recording when each subtree is
/// entered and left.
///
/// # Returns
///
/// The enter and exit stamp of every block, by block index.  A subtree
/// occupies one contiguous range of stamps, which is what turns a dominance
/// query into two integer comparisons.
fn stamp_preorder(
    immediate: &[BlockId],
    order: &[BlockId],
    entry: BlockId,
) -> (Vec<u32>, Vec<u32>) {
    let count = immediate.len();

    // Invert the immediate-dominator map into child lists.  Reverse postorder
    // gives the children of each block a stable order, so the stamps -- and
    // anything derived from them -- are reproducible.
    let mut children: Vec<Vec<BlockId>> = vec![Vec::new(); count];
    for &block in order {
        if block != entry {
            children[immediate[block.index()].index()].push(block);
        }
    }

    let mut enter = vec![0u32; count];
    let mut exit = vec![0u32; count];
    let mut clock = 0u32;

    let mut stack = vec![(entry, 0usize)];
    enter[entry.index()] = clock;
    clock += 1;

    while !stack.is_empty() {
        let top = stack.len() - 1;
        let (block, next) = stack[top];

        match children[block.index()].get(next) {
            Some(&child) => {
                stack[top].1 = next + 1;
                enter[child.index()] = clock;
                clock += 1;
                stack.push((child, 0));
            }
            None => {
                exit[block.index()] = clock;
                clock += 1;
                stack.pop();
            }
        }
    }

    (enter, exit)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hand-built control-flow graph, owning the adjacency lists that
    /// [`Graph`] borrows.
    struct Edges {
        predecessors: Vec<Vec<BlockId>>,
        successors: Vec<Vec<BlockId>>,
    }

    impl Edges {
        /// Build a graph of `count` blocks from a `(from, to)` edge list.
        /// Block 0 is the entry.
        fn new(count: usize, edges: &[(usize, usize)]) -> Self {
            let mut this = Self {
                predecessors: vec![Vec::new(); count],
                successors: vec![Vec::new(); count],
            };
            for &(from, to) in edges {
                this.successors[from].push(BlockId::from_index(to));
                this.predecessors[to].push(BlockId::from_index(from));
            }
            this
        }

        fn graph(&self) -> Graph<'_> {
            Graph::new(BlockId::from_index(0), &self.predecessors, &self.successors)
        }
    }

    fn block(index: usize) -> BlockId {
        BlockId::from_index(index)
    }

    /// The immediate dominator of every block, by index, with the entry's own
    /// entry left as `None`.
    fn immediate_dominators(tree: &DomTree, count: usize) -> Vec<Option<usize>> {
        (0..count)
            .map(|index| {
                tree.immediate_dominator(block(index))
                    .map(|idom| idom.index())
            })
            .collect()
    }

    /// The dominance frontier of every block, by index.
    fn frontiers(tree: &DomTree, graph: Graph<'_>) -> Vec<Vec<usize>> {
        tree.frontiers(graph)
            .into_iter()
            .map(|frontier| frontier.into_iter().map(BlockId::index).collect())
            .collect()
    }

    /// `0 -> {1, 2} -> 3`: the shape an `if`/`else` lowers to.
    fn diamond() -> Edges {
        Edges::new(4, &[(0, 1), (0, 2), (1, 3), (2, 3)])
    }

    /// `0 -> 1`, `1 -> 2 -> 1` and `1 -> 3`: one loop with a single back edge.
    fn natural_loop() -> Edges {
        Edges::new(4, &[(0, 1), (1, 2), (2, 1), (1, 3)])
    }

    /// A loop header reached by two different back edges, from 2 and from 3.
    fn multiple_back_edges() -> Edges {
        Edges::new(5, &[(0, 1), (1, 2), (1, 4), (2, 1), (2, 3), (3, 1)])
    }

    /// Two blocks branching into each other, neither dominating the other:
    /// the smallest irreducible graph, and the case a single-pass dominator
    /// computation gets wrong.
    fn irreducible() -> Edges {
        Edges::new(3, &[(0, 1), (0, 2), (1, 2), (2, 1)])
    }

    /// An inner loop (2 -> 3 -> 2) inside an outer one (1 -> ... -> 4 -> 1).
    fn nested_loops() -> Edges {
        Edges::new(6, &[(0, 1), (1, 2), (2, 3), (3, 2), (2, 4), (4, 1), (1, 5)])
    }

    #[test]
    fn reverse_postorder_puts_a_block_before_its_forward_successors() {
        // Arrange
        let edges = diamond();

        // Act
        let order = reverse_postorder(edges.graph());

        // Assert: the entry comes first and the join comes last, whichever arm
        // the depth-first walk happens to take first.
        let position = |target: usize| {
            order
                .iter()
                .position(|&b| b == block(target))
                .expect("every reachable block is in the order")
        };
        assert_eq!(order.len(), 4);
        assert_eq!(position(0), 0);
        assert!(position(0) < position(1) && position(1) < position(3));
        assert!(position(0) < position(2) && position(2) < position(3));
    }

    #[test]
    fn reverse_postorder_starts_at_the_header_of_a_loop_and_visits_it_once() {
        // Arrange: the back edge 2 -> 1 must not make the walk revisit 1.
        let edges = natural_loop();

        // Act
        let order = reverse_postorder(edges.graph());

        // Assert
        assert_eq!(order.len(), 4);
        assert_eq!(order[0], block(0));
        assert_eq!(order[1], block(1));
    }

    #[test]
    fn reverse_postorder_covers_only_reachable_blocks() {
        // Arrange: block 2 has an outgoing edge but nothing reaches it.
        let edges = Edges::new(3, &[(0, 1), (2, 1)]);

        // Act
        let order = reverse_postorder(edges.graph());

        // Assert
        assert_eq!(order, vec![block(0), block(1)]);
    }

    #[test]
    fn both_arms_of_a_diamond_are_dominated_by_the_block_that_branched() {
        // Arrange
        let edges = diamond();

        // Act
        let tree = DomTree::build(edges.graph());

        // Assert: the join is dominated by the branch, not by either arm --
        // it is reachable through both.
        assert_eq!(
            immediate_dominators(&tree, 4),
            vec![None, Some(0), Some(0), Some(0)]
        );
        assert!(tree.dominates(block(0), block(3)));
        assert!(!tree.dominates(block(1), block(3)));
        assert!(!tree.dominates(block(2), block(3)));
    }

    #[test]
    fn a_loop_header_dominates_its_body() {
        // Arrange
        let edges = natural_loop();

        // Act
        let tree = DomTree::build(edges.graph());

        // Assert: the back edge does not stop 1 from dominating 2 and 3.
        assert_eq!(
            immediate_dominators(&tree, 4),
            vec![None, Some(0), Some(1), Some(1)]
        );
        assert!(tree.dominates(block(1), block(2)));
        assert!(!tree.dominates(block(2), block(3)));
    }

    #[test]
    fn a_header_with_two_back_edges_still_dominates_the_whole_loop() {
        // Arrange
        let edges = multiple_back_edges();

        // Act
        let tree = DomTree::build(edges.graph());

        // Assert: 3 is only reachable through 2, so 2 dominates it even though
        // both 2 and 3 branch back to the header.
        assert_eq!(
            immediate_dominators(&tree, 5),
            vec![None, Some(0), Some(1), Some(2), Some(1)]
        );
        assert!(tree.dominates(block(1), block(3)));
        assert!(tree.dominates(block(2), block(3)));
    }

    #[test]
    fn neither_block_of_an_irreducible_loop_dominates_the_other() {
        // Arrange: 1 and 2 branch into each other and both are reachable
        // directly from the entry, so the loop has two entry points.
        let edges = irreducible();

        // Act
        let tree = DomTree::build(edges.graph());

        // Assert: the iterative fixed point settles on the entry for both.
        assert_eq!(immediate_dominators(&tree, 3), vec![None, Some(0), Some(0)]);
        assert!(!tree.dominates(block(1), block(2)));
        assert!(!tree.dominates(block(2), block(1)));
        assert!(tree.dominates(block(0), block(1)));
        assert!(tree.dominates(block(0), block(2)));
    }

    #[test]
    fn nested_loop_headers_dominate_outwards() {
        // Arrange
        let edges = nested_loops();

        // Act
        let tree = DomTree::build(edges.graph());

        // Assert: the outer header dominates the inner loop, and the block
        // after the loop is dominated by the outer header alone.
        assert_eq!(
            immediate_dominators(&tree, 6),
            vec![None, Some(0), Some(1), Some(2), Some(2), Some(1)]
        );
        assert!(tree.dominates(block(1), block(3)));
        assert!(tree.dominates(block(2), block(4)));
        assert!(!tree.dominates(block(3), block(4)));
        assert!(!tree.dominates(block(4), block(5)));
    }

    #[test]
    fn dominance_is_reflexive_and_the_entry_dominates_everything() {
        // Arrange
        let edges = nested_loops();

        // Act
        let tree = DomTree::build(edges.graph());

        // Assert
        for index in 0..6 {
            assert!(tree.dominates(block(index), block(index)));
            assert!(tree.dominates(block(0), block(index)));
        }
    }

    #[test]
    #[should_panic(expected = "unreachable block")]
    fn dominance_refuses_a_graph_with_unreachable_blocks() {
        // Arrange: block 2 is orphaned, as unreachable-code elimination can
        // leave behind. It has to be deleted before dominance is computed.
        let edges = Edges::new(3, &[(0, 1), (2, 1)]);

        // Act / Assert
        let _ = DomTree::build(edges.graph());
    }

    #[test]
    fn the_join_of_a_diamond_is_on_the_frontier_of_both_arms() {
        // Arrange
        let edges = diamond();
        let tree = DomTree::build(edges.graph());

        // Act / Assert: 3 is where the two arms' definitions meet, so a phi
        // for anything either arm assigns belongs there.
        assert_eq!(
            frontiers(&tree, edges.graph()),
            vec![vec![], vec![3], vec![3], vec![]]
        );
    }

    #[test]
    fn a_loop_header_is_on_its_own_frontier() {
        // Arrange
        let edges = natural_loop();
        let tree = DomTree::build(edges.graph());

        // Act / Assert: a definition in the body meets the one from before the
        // loop back at the header, which is why loops need a phi there.
        assert_eq!(
            frontiers(&tree, edges.graph()),
            vec![vec![], vec![1], vec![1], vec![]]
        );
    }

    #[test]
    fn every_block_of_a_multi_latch_loop_has_the_header_on_its_frontier() {
        // Arrange
        let edges = multiple_back_edges();
        let tree = DomTree::build(edges.graph());

        // Act / Assert
        assert_eq!(
            frontiers(&tree, edges.graph()),
            vec![vec![], vec![1], vec![1], vec![1], vec![]]
        );
    }

    #[test]
    fn nested_loops_put_both_headers_on_the_inner_bodys_frontier() {
        // Arrange
        let edges = nested_loops();
        let tree = DomTree::build(edges.graph());

        // Act / Assert: the inner header is on the frontier of both loops --
        // of the inner one because of its own back edge, and of the outer one
        // because the outer latch is reached through it.
        assert_eq!(
            frontiers(&tree, edges.graph()),
            vec![vec![], vec![1], vec![1, 2], vec![2], vec![1], vec![]]
        );
    }

    #[test]
    fn an_irreducible_loop_puts_each_block_on_the_others_frontier() {
        // Arrange
        let edges = irreducible();
        let tree = DomTree::build(edges.graph());

        // Act / Assert
        assert_eq!(
            frontiers(&tree, edges.graph()),
            vec![vec![], vec![2], vec![1]]
        );
    }

    #[test]
    fn parallel_edges_are_dominated_the_same_as_a_single_edge() {
        // Arrange: a conditional branch whose arms are the same block, which
        // the SSA edge API represents as two predecessors.
        let edges = Edges::new(2, &[(0, 1), (0, 1)]);

        // Act
        let tree = DomTree::build(edges.graph());

        // Assert: 1 has two predecessors but only one of them, so nothing
        // meets there and its frontier contribution is empty.
        assert_eq!(immediate_dominators(&tree, 2), vec![None, Some(0)]);
        assert!(tree.dominates(block(0), block(1)));
        assert_eq!(frontiers(&tree, edges.graph()), vec![vec![], vec![]]);
    }
}
