//! The System V AMD64 ABI.
//!
//! Every convention the x86-64 backend has to obey is stated once here:
//! which registers carry arguments, which ones a callee must preserve, and
//! which are reserved for the code generator itself.
//!
//! - Arguments 0-5 in RDI, RSI, RDX, RCX, R8, R9; the rest on the stack.
//! - Return value in RAX.
//! - Callee-saved: RBX, RBP, RSP, R12-R15.
//! - The stack pointer is 16-byte aligned at every `call`.

use crate::backend::regalloc::RegisterFile;
use crate::backend::x86::isa::{X86Operand, X86Register};

/// Registers carrying the first six integer arguments, in order.
pub const ARGUMENT_REGISTERS: &[X86Register] = &[
    X86Register::Rdi,
    X86Register::Rsi,
    X86Register::Rdx,
    X86Register::Rcx,
    X86Register::R8,
    X86Register::R9,
];

/// The register a function returns its value in.
pub const RETURN_VALUE: X86Register = X86Register::Rax;

/// The high half of the dividend an `idiv` or a `div` reads, and where the
/// remainder is left.  Reserved for exactly that reason.
pub const DIVIDEND_HIGH: X86Register = X86Register::Rdx;

/// The frame pointer.  Every frame slot is addressed relative to it.
pub const FRAME_POINTER: X86Register = X86Register::Rbp;

/// The stack pointer.
pub const STACK_POINTER: X86Register = X86Register::Rsp;

/// Primary scratch register: loads a spilled operand, or breaks a cycle in a
/// parallel move.  Never allocatable, so nothing can be holding a live value.
pub const SCRATCH: X86Register = X86Register::R10;

/// Secondary scratch register, for when two memory operands meet in one
/// instruction.
pub const SCRATCH2: X86Register = X86Register::R11;

/// Size of a machine word, and of one frame slot.
pub const WORD_SIZE: i32 = 8;

/// Stack-pointer alignment required at a call boundary.
pub const STACK_ALIGNMENT: i32 = 16;

/// Bytes between the frame pointer and the first stack-passed argument: the
/// saved frame pointer plus the return address the `call` pushed.
const FIRST_STACK_ARGUMENT: i32 = 2 * WORD_SIZE;

/// The System V register file.
///
/// RAX and RDX are reserved because `idiv` and `cqo` overwrite them, R10 and
/// R11 because they are the code generator's scratch registers, and RSP and
/// RBP because they address the frame.  That leaves ten allocatable
/// registers.
pub struct SystemV;

impl RegisterFile for SystemV {
    type Register = X86Register;

    fn allocatable() -> &'static [X86Register] {
        // Caller-saved first: a function that makes no calls then never pays
        // for a save/restore pair in its prologue.
        &[
            X86Register::Rcx,
            X86Register::Rsi,
            X86Register::Rdi,
            X86Register::R8,
            X86Register::R9,
            X86Register::Rbx,
            X86Register::R12,
            X86Register::R13,
            X86Register::R14,
            X86Register::R15,
        ]
    }

    fn is_callee_saved(register: X86Register) -> bool {
        matches!(
            register,
            X86Register::Rbx
                | X86Register::R12
                | X86Register::R13
                | X86Register::R14
                | X86Register::R15
        )
    }
}

/// Where the caller left incoming argument `index`.
///
/// The first six arrive in registers; the rest sit just above the saved
/// return address, so argument 7 is at `[rbp + 16]`.
pub fn incoming_argument(index: usize) -> X86Operand {
    match ARGUMENT_REGISTERS.get(index) {
        Some(&register) => X86Operand::Reg(register),
        None => X86Operand::mem(
            FRAME_POINTER,
            FIRST_STACK_ARGUMENT + (index - ARGUMENT_REGISTERS.len()) as i32 * WORD_SIZE,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incoming_arguments_use_registers_then_the_caller_frame() {
        // Arrange / Act / Assert -- first six in ABI registers ...
        assert_eq!(incoming_argument(0), X86Operand::Reg(X86Register::Rdi));
        assert_eq!(incoming_argument(5), X86Operand::Reg(X86Register::R9));
        // ... the rest just above the saved return address.
        assert_eq!(incoming_argument(6), X86Operand::mem(X86Register::Rbp, 16));
        assert_eq!(incoming_argument(7), X86Operand::mem(X86Register::Rbp, 24));
    }

    #[test]
    fn no_reserved_register_is_allocatable() {
        // The code generator assumes it can always clobber these.
        for reserved in [
            RETURN_VALUE,
            DIVIDEND_HIGH,
            SCRATCH,
            SCRATCH2,
            FRAME_POINTER,
            STACK_POINTER,
        ] {
            assert!(
                !SystemV::allocatable().contains(&reserved),
                "{:?} is reserved but the allocator may hand it out",
                reserved
            );
        }
    }

    #[test]
    fn caller_saved_registers_are_preferred_over_callee_saved_ones() {
        // Arrange / Act
        let first_callee_saved = SystemV::allocatable()
            .iter()
            .position(|&register| SystemV::is_callee_saved(register))
            .expect("the ABI has callee-saved registers");

        // Assert: no caller-saved register follows a callee-saved one.
        assert!(
            SystemV::allocatable()[first_callee_saved..]
                .iter()
                .all(|&register| SystemV::is_callee_saved(register))
        );
    }
}
