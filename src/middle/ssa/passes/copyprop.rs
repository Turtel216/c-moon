//! Copy propagation: read the value, not the copy of it.
//!
//! In SSA this is what is left of a pass that classically needs a dataflow
//! analysis.  A value defined by `x = y` is `y`, everywhere and for ever --
//! there is no point at which `y` could have changed since, because `y` is
//! written exactly once.  So the pass is a substitution: work out what each
//! copy really names, rewrite every operand that reads one, and drop the
//! copies that are left over.
//!
//! # Trivial phi nodes
//!
//! A phi whose arguments are all the same value is a copy in disguise, and
//! semi-pruned placement produces them: it places a phi wherever two
//! *definitions* could meet, without asking whether they are actually
//! different.  Arguments that read the phi's own result are ignored when
//! deciding, since a value can always be itself -- that is what makes the phi
//! of a loop variable the loop never assigns collapse.

use std::collections::{HashMap, HashSet};

use crate::middle::ssa::{Function, Op, Operand, ValueId};

/// Replace every read of a copied value with the value it copies.
///
/// # Returns
///
/// Whether anything changed, which is what the pass pipeline's fixed point
/// needs.  Running it twice over the same function changes nothing the second
/// time.
pub fn run(function: &mut Function) -> bool {
    let direct = direct_copies(function);
    if direct.is_empty() {
        return false;
    }

    // What each copied value really names, following chains of copies to
    // their end.
    let resolved: HashMap<ValueId, Operand> = direct
        .keys()
        .filter_map(|&value| Some((value, resolve(value, &direct)?)))
        .collect();
    if resolved.is_empty() {
        return false;
    }

    function.substitute_operands(&resolved);

    // Every read of these has just been replaced, so the definitions have no
    // reader left.
    for block in function.block_ids() {
        function.retain_phis(block, |phi| !resolved.contains_key(&phi.dest));

        let dead: Vec<_> = function
            .block(block)
            .insts
            .iter()
            .copied()
            .filter(|&inst| {
                matches!(function.inst(inst).op, Op::Copy(_))
                    && function
                        .inst(inst)
                        .dest
                        .is_some_and(|dest| resolved.contains_key(&dest))
            })
            .collect();
        for inst in dead {
            function.remove(block, inst);
        }
    }

    true
}

/// The values that are a copy of something else: an explicit copy, or a phi
/// whose arguments agree.
fn direct_copies(function: &Function) -> HashMap<ValueId, Operand> {
    let mut direct = HashMap::new();

    for block in function.block_ids() {
        for phi in &function.block(block).phis {
            if let Some(source) = phi_source(phi.dest, &phi.args) {
                direct.insert(phi.dest, source);
            }
        }

        for &inst in &function.block(block).insts {
            if let Op::Copy(source) = function.inst(inst).op
                && let Some(dest) = function.inst(inst).dest
            {
                direct.insert(dest, source);
            }
        }
    }

    direct
}

/// The one value a phi stands for, if it stands for one.
///
/// An argument reading the phi's own result says nothing: the value on that
/// edge is whatever the phi already decided.
fn phi_source(dest: ValueId, args: &[Operand]) -> Option<Operand> {
    let mut source: Option<Operand> = None;

    for &arg in args {
        if arg == Operand::Value(dest) {
            continue;
        }
        match source {
            None => source = Some(arg),
            Some(seen) if seen == arg => {}
            Some(_) => return None,
        }
    }

    source
}

/// Follow a chain of copies to the value at its end.
///
/// # Returns
///
/// `None` when the chain closes on itself, which two phis reading each other
/// and nothing else can produce.  Such a value stands for nothing in
/// particular, and substituting it for another link of the same cycle would
/// only move the problem.
fn resolve(start: ValueId, direct: &HashMap<ValueId, Operand>) -> Option<Operand> {
    let mut seen = HashSet::from([start]);
    let mut current = *direct.get(&start)?;

    while let Operand::Value(value) = current {
        if !seen.insert(value) {
            return None;
        }
        match direct.get(&value) {
            Some(&next) => current = next,
            None => break,
        }
    }

    Some(current)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::middle::ssa::verify::verify_ssa;
    use crate::middle::ssa::{BinOp, BlockId, SlotOrigin, Terminator};

    /// A function with an entry block and nothing in it.
    fn function() -> Function {
        Function::new("f".to_string(), "entry".to_string(), "exit".to_string())
    }

    /// A loop, so that a phi has somewhere to be: `entry -> header`,
    /// `header -> latch | done`, `latch -> header`.
    fn loop_function() -> (Function, BlockId, BlockId, BlockId) {
        let mut function = function();
        let entry = function.entry();
        let header = function.add_block("header".to_string());
        let latch = function.add_block("latch".to_string());
        let done = function.add_block("done".to_string());

        function.set_terminator(entry, Terminator::Jump(header));
        function.set_terminator(latch, Terminator::Jump(header));
        function.set_terminator(
            header,
            Terminator::Branch {
                cond: Operand::Imm(1),
                then_block: latch,
                else_block: done,
            },
        );
        (function, header, latch, done)
    }

    fn copy(function: &mut Function, block: BlockId, source: Operand) -> ValueId {
        function
            .emit(block, Op::Copy(source))
            .expect("a copy defines a value")
    }

    #[test]
    fn a_chain_of_copies_collapses_to_the_value_at_its_end() {
        // Arrange: `a = 7; b = a; c = b; return c + c`.
        let mut function = function();
        let entry = function.entry();
        let first = copy(&mut function, entry, Operand::Imm(7));
        let second = copy(&mut function, entry, Operand::Value(first));
        let third = copy(&mut function, entry, Operand::Value(second));
        let sum = function
            .emit(
                entry,
                Op::Binary(BinOp::Add, Operand::Value(third), Operand::Value(third)),
            )
            .expect("an addition defines a value");
        function.set_terminator(entry, Terminator::Return(Some(Operand::Value(sum))));

        // Act
        assert!(run(&mut function));

        // Assert: the addition reads the literal, and no copy is left.
        assert_eq!(verify_ssa(&function), Ok(()));
        assert_eq!(
            function.to_string(),
            "\
function f {
.entry:
    %v3 = 7 + 7
    ret %v3
}
"
        );
    }

    #[test]
    fn a_phi_whose_arguments_agree_is_a_copy_in_disguise() {
        // Arrange: both edges into the header carry the same value, which is
        // what semi-pruned placement produces when it places a phi without
        // asking whether the definitions differ.
        let (mut function, header, _, done) = loop_function();
        let entry = function.entry();
        let value = copy(&mut function, entry, Operand::Imm(3));

        let slot = function.slot_for(SlotOrigin::Variable(0));
        let merged = function.add_phi(header, slot);
        function.block_mut(header).phis[0].args =
            vec![Operand::Value(value), Operand::Value(value)];
        function.set_terminator(done, Terminator::Return(Some(Operand::Value(merged))));

        // Act
        assert!(run(&mut function));

        // Assert: the phi is gone, and so is the copy behind it, so the
        // return reads the literal.
        assert_eq!(verify_ssa(&function), Ok(()));
        assert!(function.block(header).phis.is_empty());
        assert_eq!(
            *function.block(done).terminator(),
            Terminator::Return(Some(Operand::Imm(3)))
        );
    }

    #[test]
    fn a_phi_that_only_ever_takes_its_own_result_back_collapses_too() {
        // Arrange: the loop carries a value it never assigns, so the back edge
        // hands the phi its own result.
        let (mut function, header, _, done) = loop_function();
        let entry = function.entry();
        let value = copy(&mut function, entry, Operand::Imm(5));

        let slot = function.slot_for(SlotOrigin::Variable(0));
        let carried = function.add_phi(header, slot);
        function.block_mut(header).phis[0].args =
            vec![Operand::Value(value), Operand::Value(carried)];
        function.set_terminator(done, Terminator::Return(Some(Operand::Value(carried))));

        // Act
        assert!(run(&mut function));

        // Assert: an argument that reads the phi's own result says nothing
        // about what the phi is, so the other argument decides it.
        assert_eq!(verify_ssa(&function), Ok(()));
        assert!(function.block(header).phis.is_empty());
        assert_eq!(
            *function.block(done).terminator(),
            Terminator::Return(Some(Operand::Imm(5)))
        );
    }

    #[test]
    fn a_phi_whose_arguments_differ_is_left_alone() {
        // Arrange: a genuine merge of two different values.
        let (mut function, header, latch, done) = loop_function();
        let entry = function.entry();
        let initial = copy(&mut function, entry, Operand::Imm(0));

        let slot = function.slot_for(SlotOrigin::Variable(0));
        let carried = function.add_phi(header, slot);
        let stepped = function
            .emit(
                latch,
                Op::Binary(BinOp::Add, Operand::Value(carried), Operand::Imm(1)),
            )
            .expect("an addition defines a value");
        function.block_mut(header).phis[0].args =
            vec![Operand::Value(initial), Operand::Value(stepped)];
        function.set_terminator(done, Terminator::Return(Some(Operand::Value(carried))));

        // Act: the copy in the entry block still propagates, but the phi stays.
        run(&mut function);

        // Assert
        assert_eq!(verify_ssa(&function), Ok(()));
        assert_eq!(function.block(header).phis.len(), 1);
    }

    #[test]
    fn two_phis_that_read_only_each_other_are_left_alone() {
        // Arrange: a chain of copies that closes on itself stands for no
        // particular value, and substituting one link for another would only
        // move the problem.
        let (mut function, header, latch, done) = loop_function();
        let slot = function.slot_for(SlotOrigin::Variable(0));
        let second_header = function.add_block("second".to_string());

        // `header` and `second` each carry a phi reading the other's result.
        function.set_terminator(latch, Terminator::Jump(second_header));
        function.set_terminator(second_header, Terminator::Jump(header));

        let first = function.add_phi(header, slot);
        let second = function.add_phi(second_header, slot);
        function.block_mut(header).phis[0].args = vec![Operand::Value(second)];
        function.block_mut(second_header).phis[0].args = vec![Operand::Value(first)];
        function.set_terminator(done, Terminator::Return(Some(Operand::Value(first))));

        // Act / Assert: nothing to substitute, and no infinite chase.
        assert!(!run(&mut function));
        assert_eq!(function.block(header).phis.len(), 1);
    }

    #[test]
    fn running_it_twice_changes_nothing_the_second_time() {
        // Arrange: the pass pipeline's fixed point depends on this.
        let mut function = function();
        let entry = function.entry();
        let first = copy(&mut function, entry, Operand::Imm(7));
        let second = copy(&mut function, entry, Operand::Value(first));
        function.set_terminator(entry, Terminator::Return(Some(Operand::Value(second))));

        // Act / Assert
        assert!(run(&mut function));
        assert!(!run(&mut function));
    }
}
