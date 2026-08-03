//! Assembly emission.
//!
//! `Display` implementations that turn the typed instructions of
//! [`isa`](super::isa) into Intel-syntax x86-64 text.  Printing an
//! [`X86Program`] yields a complete `.s` file.

use std::cmp::Ordering;
use std::fmt;

use crate::backend::x86::isa::{
    MemoryRef, RegisterWidth, X86Function, X86Instruction, X86Operand, X86Program,
};

/// Indentation of an instruction line; labels and directives start at column 0.
const INDENT: &str = "    ";

impl fmt::Display for MemoryRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Every value this compiler stores is a machine word, so the operand
        // size is always explicit and always the same.
        write!(f, "QWORD PTR [{}", self.base)?;

        if let Some((index, scale)) = self.index {
            write!(f, " + {}*{}", index, scale)?;
        }

        // `unsigned_abs` keeps the negation correct even at `i32::MIN`.
        match self.disp.cmp(&0) {
            Ordering::Greater => write!(f, " + {}", self.disp)?,
            Ordering::Less => write!(f, " - {}", self.disp.unsigned_abs())?,
            Ordering::Equal => {}
        }

        write!(f, "]")
    }
}

impl fmt::Display for X86Operand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Reg(register) => write!(f, "{}", register),
            Self::Mem(reference) => write!(f, "{}", reference),
            Self::Imm(value) => write!(f, "{}", value),
            Self::Label(label) => write!(f, "{}", label),
        }
    }
}

impl fmt::Display for X86Instruction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Mov(destination, source) => binary(f, "mov", destination, source),
            Self::Lea(destination, source) => binary(f, "lea", destination, source),
            Self::Add(destination, source) => binary(f, "add", destination, source),
            Self::Sub(destination, source) => binary(f, "sub", destination, source),
            Self::Imul(destination, source) => binary(f, "imul", destination, source),
            Self::Cmp(left, right) => binary(f, "cmp", left, right),
            Self::Test(left, right) => binary(f, "test", left, right),

            Self::Idiv(source) => unary(f, "idiv", source),
            Self::Push(source) => unary(f, "push", source),
            Self::Pop(destination) => unary(f, "pop", destination),

            Self::Cqo => write!(f, "{}cqo", INDENT),
            Self::Ret => write!(f, "{}ret", INDENT),

            Self::Jmp(label) => write!(f, "{}jmp {}", INDENT, label),
            Self::Call(label) => write!(f, "{}call {}", INDENT, label),
            Self::Jcc(condition, label) => write!(f, "{}j{} {}", INDENT, condition, label),

            // `setcc` writes a byte; `movzx` reads one and writes a dword,
            // which zero-extends to the full register for free.
            Self::SetCC(condition, destination) => write!(
                f,
                "{}set{} {}",
                INDENT,
                condition,
                destination.name(RegisterWidth::Byte)
            ),
            Self::Movzx(destination, source) => write!(
                f,
                "{}movzx {}, {}",
                INDENT,
                destination.name(RegisterWidth::Double),
                source.name(RegisterWidth::Byte)
            ),

            Self::Label(label) => write!(f, "{}:", label),
        }
    }
}

/// Write `mnemonic operand`.
fn unary(f: &mut fmt::Formatter<'_>, mnemonic: &str, operand: &X86Operand) -> fmt::Result {
    write!(f, "{}{} {}", INDENT, mnemonic, operand)
}

/// Write `mnemonic destination, source`.
fn binary(
    f: &mut fmt::Formatter<'_>,
    mnemonic: &str,
    destination: &X86Operand,
    source: &X86Operand,
) -> fmt::Result {
    write!(f, "{}{} {}, {}", INDENT, mnemonic, destination, source)
}

impl fmt::Display for X86Function {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, ".globl {}", self.name)?;
        writeln!(f, ".type {}, @function", self.name)?;
        writeln!(f, "{}:", self.name)?;
        for instr in &self.instructions {
            writeln!(f, "{}", instr)?;
        }
        Ok(())
    }
}

impl fmt::Display for X86Program {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, ".intel_syntax noprefix")?;
        writeln!(f, ".section .text")?;
        writeln!(f)?;
        for function in &self.functions {
            writeln!(f, "{}", function)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::backend::x86::isa::{ConditionCode, X86Register};

    #[test]
    fn a_frame_slot_prints_its_displacement_with_a_sign() {
        // Arrange / Act / Assert
        assert_eq!(
            X86Operand::mem(X86Register::Rbp, -8).to_string(),
            "QWORD PTR [rbp - 8]"
        );
        assert_eq!(
            X86Operand::mem(X86Register::Rbp, 16).to_string(),
            "QWORD PTR [rbp + 16]"
        );
        assert_eq!(
            X86Operand::mem(X86Register::R11, 0).to_string(),
            "QWORD PTR [r11]"
        );
    }

    #[test]
    fn a_scaled_index_prints_the_whole_addressing_mode() {
        // Arrange / Act / Assert: one instruction reaches an array element.
        assert_eq!(
            X86Operand::indexed(X86Register::Rbp, X86Register::Rcx, 8, -24).to_string(),
            "QWORD PTR [rbp + rcx*8 - 24]"
        );
    }

    #[test]
    fn byte_and_dword_views_are_used_where_the_encoding_needs_them() {
        // Arrange / Act / Assert
        assert_eq!(
            X86Instruction::SetCC(ConditionCode::Le, X86Register::Rax).to_string(),
            "    setle al"
        );
        assert_eq!(
            X86Instruction::Movzx(X86Register::R12, X86Register::Rax).to_string(),
            "    movzx r12d, al"
        );
    }

    #[test]
    fn labels_start_at_the_left_margin_and_instructions_are_indented() {
        // Arrange / Act / Assert
        assert_eq!(
            X86Instruction::Label(".main_exit".to_string()).to_string(),
            ".main_exit:"
        );
        assert_eq!(X86Instruction::Cqo.to_string(), "    cqo");
        assert_eq!(
            X86Instruction::Jcc(ConditionCode::Ne, ".L1".to_string()).to_string(),
            "    jne .L1"
        );
    }

    #[test]
    fn a_program_opens_with_the_directives_gcc_needs() {
        // Arrange
        let program = X86Program {
            functions: vec![X86Function {
                name: "main".to_string(),
                instructions: vec![X86Instruction::Ret],
            }],
        };

        // Act
        let assembly = program.to_string();

        // Assert
        assert!(assembly.starts_with(".intel_syntax noprefix\n.section .text\n"));
        assert!(assembly.contains(".globl main\n.type main, @function\nmain:\n    ret\n"));
    }
}
