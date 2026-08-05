//! The promotion gate: which memory locations may become SSA values.
//!
//! Construction starts with every variable in memory.  A variable may leave
//! memory and become an SSA value only if nothing can reach it except by name,
//! because an SSA value has no address and no storage that a pointer, an index
//! or a longjmp could get at.  A variable is promoted only if **all** of these
//! hold:
//!
//! 1. Its address is never taken.
//! 2. It is a scalar -- not an array, and not indexed as one.
//! 3. It is not `volatile`.
//! 4. It does not need a stable memory location for the ABI.
//! 5. It does not cross a `setjmp` boundary.
//!
//! Conditions 3 to 5 cannot arise in the language this compiler accepts today.
//! They are encoded anyway, because the cost of writing them down now is four
//! lines and the cost of remembering them later is a miscompile:
//!
//! - **`volatile`** is a keyword the lexer knows and the parser rejects, so no
//!   volatile object reaches the IR.  A test below asserts that this is still
//!   true, and fails the day it stops being.
//! - **The ABI** needs no variable pinned to memory.  Incoming arguments are
//!   read out of their ABI locations by `get_param` and placed wherever the
//!   allocator wants them -- including stack-passed ones, which are copied out
//!   of the caller's frame -- so a parameter is exactly as promotable as any
//!   other local.  See `lower_incoming_arguments` in the x86 lowering.
//! - **`setjmp`** cannot be called: there is no `extern` and no way to name a
//!   function outside the translation unit.  A call to one of [`SETJMP_LIKE`]
//!   nevertheless disqualifies every variable in the function, because a
//!   `longjmp` back into it would restore registers but not values the
//!   compiler decided to keep only in registers.
//!
//! # Conservative by construction
//!
//! [`promotable`] decides by an exhaustive match over every operation that can
//! mention a slot.  There is no catch-all arm, so an operation added later
//! will not compile until someone has said whether it lets a variable escape.
//! That is deliberate: the failure mode of forgetting is then a build error,
//! not a program that computes the wrong answer.
//!
//! There is no escape analysis, and there will not be one here.  "The address
//! is taken but the pointer never escapes" is a later optimisation, and one
//! whose mistakes only show up in the generated binary.

use std::collections::{BTreeSet, HashMap};

use crate::frontend::renamer::VarId;

use super::{Function, Op, SlotId, SlotOrigin};

/// Functions whose presence means a variable may be read after control returns
/// to this function by way of a `longjmp`.
///
/// None of these can be called today; see the module documentation.
pub const SETJMP_LIKE: &[&str] = &["setjmp", "_setjmp", "sigsetjmp", "__sigsetjmp"];

/// The slots that may become SSA values.
///
/// # Arguments
///
/// * `function` - the function in its all-in-memory form, as construction
///   first builds it
/// * `array_sizes` - element count of every array variable in the program
///
/// # Returns
///
/// The eligible slots, in slot order.  Everything else stays in memory and
/// keeps being read and written through loads and stores.
pub fn promotable(function: &Function, array_sizes: &HashMap<VarId, usize>) -> BTreeSet<SlotId> {
    // A call that could come back by way of `longjmp` disqualifies the whole
    // function, so it is worth answering first.
    if function.block_ids().any(|block| {
        function.block(block).insts.iter().any(|&inst| {
            matches!(&function.inst(inst).op, Op::Call { callee, .. }
                if SETJMP_LIKE.contains(&callee.as_str()))
        })
    }) {
        return BTreeSet::new();
    }

    let mut eligible: BTreeSet<SlotId> = function.slot_ids().collect();

    // An array occupies storage the frame planner reserved, whether or not
    // this function ever indexes it.
    eligible.retain(|&slot| match function.slot(slot).origin {
        SlotOrigin::Variable(id) => !array_sizes.contains_key(&id),
        SlotOrigin::Temporary(_) => true,
    });

    for block in function.block_ids() {
        for &inst in &function.block(block).insts {
            match &function.inst(inst).op {
                // Reading and writing a variable by name is what a value can
                // do; these are the only two operations that do not disqualify
                // the slot they mention.
                Op::SlotLoad { .. } | Op::SlotStore { .. } => {}

                // The address is handed out, so the variable needs one.
                Op::AddrOf { slot } => {
                    eligible.remove(slot);
                }

                // Indexed storage: several values live at one name, which a
                // single SSA value cannot stand for.
                Op::ArrayLoad { base, .. } | Op::ArrayStore { base, .. } => {
                    eligible.remove(base);
                }

                // The remaining operations mention no slot at all. Listing
                // them rather than using a catch-all is what makes a new
                // operation carrying a slot a compile error here.
                Op::Binary(..)
                | Op::Copy(_)
                | Op::Call { .. }
                | Op::GetParam(_)
                | Op::Undef
                | Op::Load { .. }
                | Op::Store { .. } => {}
            }
        }
    }

    eligible
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::frontend::lexer::Lexer;
    use crate::frontend::parser::Parser;
    use crate::middle::ssa::{BlockId, Operand, Terminator};

    /// A function with one block, and the slots named by `variables`.
    fn function() -> (Function, BlockId) {
        let function = Function::new("f".to_string(), "entry".to_string(), "exit".to_string());
        let entry = function.entry();
        (function, entry)
    }

    fn slot(function: &mut Function, id: VarId) -> SlotId {
        function.slot_for(SlotOrigin::Variable(id))
    }

    fn promoted(function: &Function, array_sizes: &[(VarId, usize)]) -> Vec<SlotId> {
        let sizes: HashMap<VarId, usize> = array_sizes.iter().copied().collect();
        promotable(function, &sizes).into_iter().collect()
    }

    #[test]
    fn a_scalar_read_and_written_by_name_is_promoted() {
        // Arrange: `r0 = 1; return r0`.
        let (mut function, entry) = function();
        let scalar = slot(&mut function, 0);
        function.emit(
            entry,
            Op::SlotStore {
                slot: scalar,
                value: Operand::Imm(1),
            },
        );
        let loaded = function
            .emit(entry, Op::SlotLoad { slot: scalar })
            .expect("a load defines a value");
        function.set_terminator(entry, Terminator::Return(Some(Operand::Value(loaded))));

        // Act / Assert
        assert_eq!(promoted(&function, &[]), vec![scalar]);
    }

    #[test]
    fn a_variable_whose_address_is_taken_stays_in_memory() {
        // Arrange: `p = &x` pins `x`, whatever is done with the pointer.
        let (mut function, entry) = function();
        let pinned = slot(&mut function, 0);
        let pointer = slot(&mut function, 1);
        let address = function
            .emit(entry, Op::AddrOf { slot: pinned })
            .expect("addr_of defines a value");
        function.emit(
            entry,
            Op::SlotStore {
                slot: pointer,
                value: Operand::Value(address),
            },
        );

        // Act / Assert: the pointer itself is an ordinary scalar and is
        // promoted; only what it points at is not.
        assert_eq!(promoted(&function, &[]), vec![pointer]);
    }

    #[test]
    fn an_indexed_variable_stays_in_memory() {
        // Arrange: `a[i] = 1` and `x = a[i]` both name storage, not a value.
        let (mut function, entry) = function();
        let array = slot(&mut function, 0);
        let scalar = slot(&mut function, 1);
        function.emit(
            entry,
            Op::ArrayStore {
                base: array,
                index: Operand::Imm(0),
                value: Operand::Imm(1),
            },
        );
        let element = function
            .emit(
                entry,
                Op::ArrayLoad {
                    base: array,
                    index: Operand::Imm(0),
                },
            )
            .expect("an array load defines a value");
        function.emit(
            entry,
            Op::SlotStore {
                slot: scalar,
                value: Operand::Value(element),
            },
        );

        // Act / Assert
        assert_eq!(promoted(&function, &[]), vec![scalar]);
    }

    #[test]
    fn a_declared_array_stays_in_memory_even_when_it_is_never_indexed() {
        // Arrange: the frame planner has already reserved storage for it, so
        // its name stands for an address rather than for a value.
        let (mut function, _) = function();
        let array = slot(&mut function, 0);
        let scalar = slot(&mut function, 1);

        // Act / Assert
        assert_eq!(promoted(&function, &[(0, 4)]), vec![scalar]);
        assert_ne!(promoted(&function, &[]), vec![scalar]);
        assert!(promoted(&function, &[]).contains(&array));
    }

    #[test]
    fn a_call_that_could_longjmp_back_disqualifies_every_variable() {
        // Arrange: a plain scalar that would otherwise be promoted, in a
        // function that calls `setjmp`.
        let (mut function, entry) = function();
        let scalar = slot(&mut function, 0);
        function.emit(
            entry,
            Op::SlotStore {
                slot: scalar,
                value: Operand::Imm(1),
            },
        );

        // Act / Assert: promotable on its own ...
        assert_eq!(promoted(&function, &[]), vec![scalar]);

        // ... and not once control can come back from outside.
        function.emit(
            entry,
            Op::Call {
                callee: "setjmp".to_string(),
                args: Vec::new(),
            },
        );
        assert!(promoted(&function, &[]).is_empty());
    }

    #[test]
    fn an_ordinary_call_disqualifies_nothing() {
        // Arrange: the same shape with a function that returns normally.
        let (mut function, entry) = function();
        let scalar = slot(&mut function, 0);
        function.emit(
            entry,
            Op::SlotStore {
                slot: scalar,
                value: Operand::Imm(1),
            },
        );
        function.emit(
            entry,
            Op::Call {
                callee: "g".to_string(),
                args: Vec::new(),
            },
        );

        // Act / Assert
        assert_eq!(promoted(&function, &[]), vec![scalar]);
    }

    #[test]
    fn compiler_temporaries_are_promoted_by_the_same_rule() {
        // Arrange: a temporary read by name, and one whose address is taken --
        // which the lowering cannot produce today, but the rule does not
        // depend on that.
        let (mut function, entry) = function();
        let plain = function.slot_for(SlotOrigin::Temporary("t1".to_string()));
        let pinned = function.slot_for(SlotOrigin::Temporary("t2".to_string()));
        function.emit(
            entry,
            Op::SlotStore {
                slot: plain,
                value: Operand::Imm(1),
            },
        );
        function.emit(entry, Op::AddrOf { slot: pinned });

        // Act / Assert
        assert_eq!(promoted(&function, &[]), vec![plain]);
    }

    #[test]
    fn volatile_cannot_reach_the_ir_at_all() {
        // The gate's `volatile` clause is satisfied by the front end refusing
        // the qualifier outright. If this test ever fails, promotion has
        // silently become wrong for volatile objects and this module has to
        // learn about them before the parser does.
        let mut parser = Parser::from_lexer(Lexer::new("int main() { volatile int x; return 0; }"))
            .expect("lexing should succeed");
        let (_, errors) = parser.parse_translation_unit();

        assert!(
            !errors.is_empty(),
            "`volatile` now parses: the promotion gate must reject volatile \
             variables before this compiles"
        );
    }
}
