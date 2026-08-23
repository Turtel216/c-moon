//! Global value numbering: compute each thing once.
//!
//! Two instructions that perform the same operation on the same values produce
//! the same result, so the second one is redundant -- provided the first is
//! available wherever the second was.  In SSA "the same values" is a question
//! about value ids rather than about what has been assigned since, which is
//! what makes this cheap: an expression is keyed by its operator and its
//! operands, and equal keys mean equal results.
//!
//! The walk is over the dominator tree, with the table scoped to each
//! subtree.  That scoping *is* the availability rule: an expression computed
//! in a block can be reused exactly where that block dominates, because those
//! are the points control cannot reach without having computed it.  An
//! expression computed in a sibling block is not reused, however identical --
//! there is a path that misses it.
//!
//! # What is not numbered, and why
//!
//! **Nothing that reads memory.**  Two loads of the same address are the same
//! expression syntactically and need not produce the same value, because
//! anything in between may have written to it.  Knowing otherwise needs an
//! account of which stores can affect which loads -- memory SSA -- which this
//! compiler deliberately does not have, so loads, array reads and slot reads
//! are all left alone.  This is the assumption a naive value numbering gets
//! wrong, and getting it wrong produces a miscompile rather than slow code.
//!
//! Calls are not numbered either: two calls to one function may return
//! different things, and the second one may need to happen for its own sake.
//!
//! # Partial redundancy
//!
//! An expression computed on *some* paths to a point is not touched here.
//! Removing those needs partial redundancy elimination, which inserts
//! computations on the paths that lack them; a much larger algorithm, and one
//! for another day.

use std::collections::HashMap;

use crate::middle::ir::Width;
use crate::middle::ssa::dom::{DomTree, Graph};
use crate::middle::ssa::{BinOp, BlockId, Function, InstId, Op, Operand, SlotId, ValueId};

/// What makes two computations the same.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Key {
    /// An operator, the width it computes at and its operands, with the
    /// commutative ones ordered so that `a + b` and `b + a` are one
    /// expression. Two operations of different widths are two expressions:
    /// they can disagree wherever the narrower one wraps.
    Binary(BinOp, Width, Operand, Operand),
    /// The address of a slot, which never changes within a function.
    AddrOf(SlotId),
    /// The address of one element of a slot, which is as fixed as the slot's
    /// own address once the index is.
    ArrayAddr(SlotId, Operand),
    /// A merge, which is only ever congruent to another merge in the same
    /// block taking the same values along the same edges.
    Phi(BlockId, Vec<Operand>),
}

/// Replace every computation that repeats one already available.
///
/// # Returns
///
/// Whether anything was replaced.  Running it twice over the same function
/// replaces nothing the second time.
pub fn run(function: &mut Function) -> bool {
    let tree = {
        let (predecessors, successors) = function.adjacency();
        let graph = Graph::new(function.entry(), &predecessors, &successors);
        DomTree::build(graph)
    };

    let mut numbering = Numbering {
        function,
        available: HashMap::new(),
        replacement: HashMap::new(),
        redundant: Vec::new(),
    };
    numbering.walk(&tree);
    numbering.apply()
}

/// The walk, and what it found.
struct Numbering<'a> {
    function: &'a mut Function,
    /// The value each expression is already computed into, within the subtree
    /// currently being walked.
    available: HashMap<Key, ValueId>,
    /// Redundant value to the one that already holds its result.
    replacement: HashMap<ValueId, Operand>,
    /// The definitions to remove, once their readers have been redirected.
    redundant: Vec<(BlockId, Redundant)>,
}

/// A definition found to compute something already available.
enum Redundant {
    Inst(InstId),
    Phi(ValueId),
}

impl Numbering<'_> {
    /// Walk the dominator tree, numbering as it goes.
    ///
    /// Preorder with a scoped table: everything a block adds is in scope for
    /// the blocks it dominates, and goes out of scope on the way back up.
    fn walk(&mut self, tree: &DomTree) {
        let children = tree.children();
        let mut steps = vec![Step::Enter(self.function.entry())];
        let mut added: Vec<Vec<Key>> = vec![Vec::new(); self.function.block_count()];

        while let Some(step) = steps.pop() {
            match step {
                Step::Enter(block) => {
                    added[block.index()] = self.number_block(block);
                    steps.push(Step::Leave(block));
                    for &child in children[block.index()].iter().rev() {
                        steps.push(Step::Enter(child));
                    }
                }
                Step::Leave(block) => {
                    for key in added[block.index()].drain(..) {
                        self.available.remove(&key);
                    }
                }
            }
        }
    }

    /// Number one block's phis and instructions.
    ///
    /// # Returns
    ///
    /// The keys this block made available, to be taken out of scope again.
    fn number_block(&mut self, block: BlockId) -> Vec<Key> {
        let mut added = Vec::new();

        let phis: Vec<(ValueId, Vec<Operand>)> = self
            .function
            .block(block)
            .phis
            .iter()
            .map(|phi| (phi.dest, phi.args.clone()))
            .collect();
        for (dest, args) in phis {
            let arguments = args.into_iter().map(|arg| self.canonical(arg)).collect();
            self.number(
                Key::Phi(block, arguments),
                dest,
                block,
                Redundant::Phi(dest),
                &mut added,
            );
        }

        for inst in self.function.block(block).insts.clone() {
            let Some(dest) = self.function.inst(inst).dest else {
                continue;
            };
            let Some(key) = self.key(&self.function.inst(inst).op.clone()) else {
                continue;
            };
            self.number(key, dest, block, Redundant::Inst(inst), &mut added);
        }

        added
    }

    /// Record `dest` as computing `key`, or note that something already does.
    fn number(
        &mut self,
        key: Key,
        dest: ValueId,
        block: BlockId,
        definition: Redundant,
        added: &mut Vec<Key>,
    ) {
        match self.available.get(&key) {
            Some(&existing) => {
                self.replacement.insert(dest, Operand::Value(existing));
                self.redundant.push((block, definition));
            }
            None => {
                self.available.insert(key.clone(), dest);
                added.push(key);
            }
        }
    }

    /// What makes this operation the same as another, if anything does.
    ///
    /// # Returns
    ///
    /// `None` for everything that may produce a different result the second
    /// time it runs -- see the note on memory above.
    fn key(&self, op: &Op) -> Option<Key> {
        match *op {
            Op::Binary(operator, width, lhs, rhs) => {
                let (lhs, rhs) = (self.canonical(lhs), self.canonical(rhs));
                // Ordering the operands of a commutative operator is what
                // makes `a + b` and `b + a` one expression rather than two.
                let (lhs, rhs) = if commutative(operator) && rhs < lhs {
                    (rhs, lhs)
                } else {
                    (lhs, rhs)
                };
                Some(Key::Binary(operator, width, lhs, rhs))
            }
            Op::AddrOf { slot } => Some(Key::AddrOf(slot)),
            Op::ArrayAddr { base, index, .. } => Some(Key::ArrayAddr(base, self.canonical(index))),

            // Everything that reads memory or calls out, and the operations
            // that define nothing. Listed rather than caught by a wildcard, so
            // that a new operation has to be classified before it compiles.
            Op::Copy(_)
            | Op::Convert { .. }
            | Op::Call { .. }
            | Op::GetParam(_)
            | Op::Undef
            | Op::SlotLoad { .. }
            | Op::SlotStore { .. }
            | Op::ArrayLoad { .. }
            | Op::ArrayStore { .. }
            | Op::Load { .. }
            | Op::Store { .. } => None,
        }
    }

    /// The operand to key on: whatever this one has been found equal to.
    ///
    /// Following replacements here rather than only rewriting at the end means
    /// an expression built out of a redundant value is recognised in the same
    /// pass as the value itself.
    fn canonical(&self, operand: Operand) -> Operand {
        let mut current = operand;
        while let Operand::Value(value) = current {
            match self.replacement.get(&value) {
                Some(&next) => current = next,
                None => break,
            }
        }
        current
    }

    /// Redirect the readers of every redundant value and delete its
    /// definition.
    fn apply(self) -> bool {
        if self.replacement.is_empty() {
            return false;
        }

        // Resolved through any chain, so that a value replaced by a value that
        // was itself replaced ends up at the one that survives.
        let resolved: HashMap<ValueId, Operand> = self
            .replacement
            .keys()
            .map(|&value| (value, self.canonical(Operand::Value(value))))
            .collect();

        let function = self.function;
        function.substitute_operands(&resolved);

        for (block, definition) in self.redundant {
            match definition {
                Redundant::Inst(inst) => function.remove(block, inst),
                Redundant::Phi(dest) => function.retain_phis(block, |phi| phi.dest != dest),
            }
        }

        true
    }
}

/// One step of the walk over the dominator tree.
enum Step {
    Enter(BlockId),
    Leave(BlockId),
}

/// Does the order of this operator's operands matter?
fn commutative(operator: BinOp) -> bool {
    matches!(operator, BinOp::Add | BinOp::Mul | BinOp::Eq | BinOp::Neq)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::middle::ssa::verify::verify_ssa;
    use crate::middle::ssa::{SlotOrigin, Terminator};

    fn function() -> Function {
        Function::new("f".to_string(), "entry".to_string(), "exit".to_string())
    }

    /// Two opaque values to build expressions out of.
    fn arguments(function: &mut Function) -> (Operand, Operand) {
        let entry = function.entry();
        let first = function
            .emit(entry, Op::GetParam(0))
            .expect("a parameter read defines a value");
        let second = function
            .emit(entry, Op::GetParam(1))
            .expect("a parameter read defines a value");
        (Operand::Value(first), Operand::Value(second))
    }

    fn binary(
        function: &mut Function,
        block: BlockId,
        operator: BinOp,
        lhs: Operand,
        rhs: Operand,
    ) -> ValueId {
        function
            .emit(block, Op::Binary(operator, Width::Bits64, lhs, rhs))
            .expect("a binary operation defines a value")
    }

    /// How many instructions the block holds.
    fn instructions(function: &Function, block: BlockId) -> usize {
        function.block(block).insts.len()
    }

    #[test]
    fn the_same_expression_twice_is_computed_once() {
        // Arrange: `(a + b) + (a + b)`.
        let mut function = function();
        let entry = function.entry();
        let (a, b) = arguments(&mut function);
        let first = binary(&mut function, entry, BinOp::Add, a, b);
        let second = binary(&mut function, entry, BinOp::Add, a, b);
        let total = binary(
            &mut function,
            entry,
            BinOp::Add,
            Operand::Value(first),
            Operand::Value(second),
        );
        function.set_terminator(entry, Terminator::Return(Some(Operand::Value(total))));

        // Act
        assert!(run(&mut function));

        // Assert: two parameter reads, one addition, and the total.
        assert_eq!(verify_ssa(&function), Ok(()));
        assert_eq!(instructions(&function, entry), 4);
    }

    #[test]
    fn the_order_of_a_commutative_operator_does_not_make_it_a_different_expression() {
        // Arrange: `a + b` and `b + a`.
        let mut function = function();
        let entry = function.entry();
        let (a, b) = arguments(&mut function);
        let first = binary(&mut function, entry, BinOp::Add, a, b);
        let second = binary(&mut function, entry, BinOp::Add, b, a);
        let total = binary(
            &mut function,
            entry,
            BinOp::Add,
            Operand::Value(first),
            Operand::Value(second),
        );
        function.set_terminator(entry, Terminator::Return(Some(Operand::Value(total))));

        // Act / Assert
        assert!(run(&mut function));
        assert_eq!(instructions(&function, entry), 4);
    }

    #[test]
    fn the_order_of_a_subtraction_does_make_it_a_different_expression() {
        // Arrange: `a - b` and `b - a`, which are not the same number.
        let mut function = function();
        let entry = function.entry();
        let (a, b) = arguments(&mut function);
        let first = binary(&mut function, entry, BinOp::Sub, a, b);
        let second = binary(&mut function, entry, BinOp::Sub, b, a);
        let total = binary(
            &mut function,
            entry,
            BinOp::Add,
            Operand::Value(first),
            Operand::Value(second),
        );
        function.set_terminator(entry, Terminator::Return(Some(Operand::Value(total))));

        // Act / Assert
        assert!(!run(&mut function));
        assert_eq!(instructions(&function, entry), 5);
    }

    #[test]
    fn an_expression_is_reused_where_the_block_computing_it_dominates() {
        // Arrange: `a + b` in the entry and again in a block it dominates.
        let mut function = function();
        let entry = function.entry();
        let later = function.add_block("later".to_string());
        let (a, b) = arguments(&mut function);
        binary(&mut function, entry, BinOp::Add, a, b);
        function.set_terminator(entry, Terminator::Jump(later));

        let repeated = binary(&mut function, later, BinOp::Add, a, b);
        function.set_terminator(later, Terminator::Return(Some(Operand::Value(repeated))));

        // Act
        assert!(run(&mut function));

        // Assert: the second one is gone, and the return reads the first.
        assert_eq!(verify_ssa(&function), Ok(()));
        assert_eq!(instructions(&function, later), 0);
    }

    #[test]
    fn an_expression_is_not_reused_from_a_block_that_does_not_dominate() {
        // Arrange: `a + b` in one arm of a branch and again after the join.
        // Control can reach the join without having gone through the arm, so
        // the value is not available there however identical it looks.
        let mut function = function();
        let entry = function.entry();
        let arm = function.add_block("arm".to_string());
        let join = function.add_block("join".to_string());
        let (a, b) = arguments(&mut function);

        function.set_terminator(arm, Terminator::Jump(join));
        function.set_terminator(
            entry,
            Terminator::Branch {
                cond: a,
                then_block: arm,
                else_block: join,
                width: Width::Bits64,
            },
        );

        binary(&mut function, arm, BinOp::Add, a, b);
        let repeated = binary(&mut function, join, BinOp::Add, a, b);
        function.set_terminator(join, Terminator::Return(Some(Operand::Value(repeated))));

        // Act / Assert
        assert!(!run(&mut function));
        assert_eq!(instructions(&function, join), 1);
    }

    #[test]
    fn two_reads_of_the_same_variable_are_not_assumed_to_agree() {
        // Arrange: a load, a call that could write through a pointer to the
        // same variable, and a second load. Deciding these are the same value
        // needs to know what the call can reach, which this compiler does not
        // track -- so neither load is touched.
        let mut function = function();
        let entry = function.entry();
        let slot = function.slot_for(SlotOrigin::Variable(0));
        let first = function
            .emit(
                entry,
                Op::SlotLoad {
                    slot,
                    width: Width::Bits64,
                },
            )
            .expect("a load defines a value");
        function.emit(
            entry,
            Op::Call {
                callee: "g".to_string(),
                args: Vec::new(),
            },
        );
        let second = function
            .emit(
                entry,
                Op::SlotLoad {
                    slot,
                    width: Width::Bits64,
                },
            )
            .expect("a load defines a value");
        let total = binary(
            &mut function,
            entry,
            BinOp::Add,
            Operand::Value(first),
            Operand::Value(second),
        );
        function.set_terminator(entry, Terminator::Return(Some(Operand::Value(total))));

        // Act / Assert
        assert!(!run(&mut function));
        assert_eq!(instructions(&function, entry), 4);
    }

    #[test]
    fn two_calls_to_the_same_function_are_both_kept() {
        // Arrange: what a function returns is not a function of its
        // arguments, and calling it may be the point.
        let mut function = function();
        let entry = function.entry();
        for _ in 0..2 {
            function.emit(
                entry,
                Op::Call {
                    callee: "g".to_string(),
                    args: vec![Operand::Imm(1)],
                },
            );
        }
        function.set_terminator(entry, Terminator::Return(None));

        // Act / Assert
        assert!(!run(&mut function));
        assert_eq!(instructions(&function, entry), 2);
    }

    #[test]
    fn two_merges_of_the_same_values_become_one() {
        // Arrange: two variables carrying the same values round the same
        // edges, which semi-pruned placement gives a phi each.
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

        let first_slot = function.slot_for(SlotOrigin::Variable(0));
        let second_slot = function.slot_for(SlotOrigin::Variable(1));
        let first = function.add_phi(join, first_slot);
        let second = function.add_phi(join, second_slot);
        function.block_mut(join).phis[0].args = vec![Operand::Imm(1), Operand::Imm(2)];
        function.block_mut(join).phis[1].args = vec![Operand::Imm(1), Operand::Imm(2)];

        let total = binary(
            &mut function,
            join,
            BinOp::Add,
            Operand::Value(first),
            Operand::Value(second),
        );
        function.set_terminator(join, Terminator::Return(Some(Operand::Value(total))));

        // Act
        assert!(run(&mut function));

        // Assert
        assert_eq!(verify_ssa(&function), Ok(()));
        assert_eq!(function.block(join).phis.len(), 1);
    }

    #[test]
    fn the_address_of_one_element_is_computed_once() {
        // Arrange: `&a[i]` twice over. Nothing a program can do moves an
        // element, so the second address is the first one.
        let mut function = function();
        let entry = function.entry();
        let (index, _) = arguments(&mut function);
        let base = function.slot_for(SlotOrigin::Variable(0));
        let address = |function: &mut Function| {
            function
                .emit(
                    entry,
                    Op::ArrayAddr {
                        base,
                        index,
                        width: Width::Bits32,
                    },
                )
                .expect("an element address defines a value")
        };
        address(&mut function);
        let second = address(&mut function);
        function.set_terminator(entry, Terminator::Return(Some(Operand::Value(second))));

        // Act
        assert!(run(&mut function));

        // Assert: two parameter reads and the one surviving address.
        assert_eq!(verify_ssa(&function), Ok(()));
        assert_eq!(instructions(&function, entry), 3);
    }

    #[test]
    fn an_element_address_of_a_different_element_is_a_different_address() {
        // Arrange: `&a[i]` and `&a[j]`, which are two elements apart.
        let mut function = function();
        let entry = function.entry();
        let (first_index, second_index) = arguments(&mut function);
        let base = function.slot_for(SlotOrigin::Variable(0));
        let address = |function: &mut Function, index| {
            function
                .emit(
                    entry,
                    Op::ArrayAddr {
                        base,
                        index,
                        width: Width::Bits32,
                    },
                )
                .expect("an element address defines a value")
        };
        address(&mut function, first_index);
        let second = address(&mut function, second_index);
        function.set_terminator(entry, Terminator::Return(Some(Operand::Value(second))));

        // Act / Assert
        assert!(!run(&mut function));
        assert_eq!(instructions(&function, entry), 4);
    }

    #[test]
    fn running_it_twice_replaces_nothing_the_second_time() {
        // Arrange: the pass pipeline's fixed point depends on this.
        let mut function = function();
        let entry = function.entry();
        let (a, b) = arguments(&mut function);
        binary(&mut function, entry, BinOp::Add, a, b);
        let second = binary(&mut function, entry, BinOp::Add, a, b);
        function.set_terminator(entry, Terminator::Return(Some(Operand::Value(second))));

        // Act / Assert
        assert!(run(&mut function));
        assert!(!run(&mut function));
    }
}
