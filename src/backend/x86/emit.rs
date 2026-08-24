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

/// An operand written the way the instruction that uses it reads it.
///
/// The same frame slot is `DWORD PTR [rbp - 8]` to an instruction operating on
/// an `int` and `QWORD PTR [rbp - 8]` to one operating on a `long int`, and
/// the same register is `eax` or `rax`.  Nothing about an operand says which,
/// so the width comes from the instruction and is paired with it here.
#[derive(Debug, Clone, Copy)]
struct Sized<'a>(RegisterWidth, &'a X86Operand);

impl fmt::Display for Sized<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Sized(width, operand) = *self;
        match operand {
            X86Operand::Reg(register) => write!(f, "{}", register.name(width)),
            X86Operand::Mem(reference) => memory(f, width, reference),
            X86Operand::Imm(value) => write!(f, "{}", value),
            X86Operand::Label(label) => write!(f, "{}", label),
        }
    }
}

/// Write a memory reference, e.g. `DWORD PTR [rbp + rcx*4 - 24]`.
fn memory(f: &mut fmt::Formatter<'_>, width: RegisterWidth, reference: &MemoryRef) -> fmt::Result {
    // The size qualifier is what tells the assembler how much to touch when no
    // register operand settles it, as in `mov DWORD PTR [rbp - 8], 5`.
    let qualifier = match width {
        RegisterWidth::Quad => "QWORD",
        RegisterWidth::Double => "DWORD",
        RegisterWidth::Byte => "BYTE",
    };
    write!(f, "{} PTR [{}", qualifier, reference.base)?;

    if let Some((index, scale)) = reference.index {
        write!(f, " + {}*{}", index, scale)?;
    }

    // `unsigned_abs` keeps the negation correct even at `i32::MIN`.
    match reference.disp.cmp(&0) {
        Ordering::Greater => write!(f, " + {}", reference.disp)?,
        Ordering::Less => write!(f, " - {}", reference.disp.unsigned_abs())?,
        Ordering::Equal => {}
    }

    write!(f, "]")
}

impl fmt::Display for X86Instruction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Mov(width, destination, source) => binary(f, "mov", *width, destination, source),
            // An address is a machine word whatever it points at, and `lea`
            // never touches what is there.
            Self::Lea(destination, source) => {
                binary(f, "lea", RegisterWidth::Quad, destination, source)
            }
            Self::Add(width, destination, source) => binary(f, "add", *width, destination, source),
            Self::Sub(width, destination, source) => binary(f, "sub", *width, destination, source),
            Self::Imul(width, destination, source) => {
                binary(f, "imul", *width, destination, source)
            }
            Self::Cmp(width, left, right) => binary(f, "cmp", *width, left, right),
            Self::Test(width, left, right) => binary(f, "test", *width, left, right),

            Self::Idiv(width, source) => unary(f, "idiv", *width, source),
            Self::Div(width, source) => unary(f, "div", *width, source),
            Self::Push(source) => unary(f, "push", RegisterWidth::Quad, source),
            Self::Pop(destination) => unary(f, "pop", RegisterWidth::Quad, destination),

            // One mnemonic per width: `cdq` fills EDX from EAX, `cqo` fills
            // RDX from RAX.  There is no byte case: C promotes both operands
            // of a division to at least an `int`, so nothing divides at eight
            // bits.
            Self::SignExtendAccumulator(RegisterWidth::Quad) => write!(f, "{}cqo", INDENT),
            Self::SignExtendAccumulator(RegisterWidth::Double) => write!(f, "{}cdq", INDENT),
            Self::SignExtendAccumulator(RegisterWidth::Byte) => {
                panic!("Compiler Bug: nothing divides at byte width")
            }
            Self::Ret => write!(f, "{}ret", INDENT),

            Self::Jmp(label) => write!(f, "{}jmp {}", INDENT, label),
            Self::Call(label) => write!(f, "{}call {}", INDENT, label),
            Self::Jcc(condition, label) => write!(f, "{}j{} {}", INDENT, condition, label),

            // `setcc` writes a byte, which `movzx` is then what turns into
            // the 0 or 1 of an `int`.
            Self::SetCC(condition, destination) => write!(
                f,
                "{}set{} {}",
                INDENT,
                condition,
                destination.name(RegisterWidth::Byte)
            ),
            Self::Movzx {
                to,
                from,
                destination,
                source,
            } => write!(
                f,
                "{}movzx {}, {}",
                INDENT,
                destination.name(*to),
                Sized(*from, source)
            ),
            Self::Movsx {
                to,
                from,
                destination,
                source,
            } => write!(
                f,
                "{}movsx {}, {}",
                INDENT,
                destination.name(*to),
                Sized(*from, source)
            ),

            Self::Label(label) => write!(f, "{}:", label),
        }
    }
}

/// Write `mnemonic operand`, reading the operand at `width`.
fn unary(
    f: &mut fmt::Formatter<'_>,
    mnemonic: &str,
    width: RegisterWidth,
    operand: &X86Operand,
) -> fmt::Result {
    write!(f, "{}{} {}", INDENT, mnemonic, Sized(width, operand))
}

/// Write `mnemonic destination, source`, both at `width`.
fn binary(
    f: &mut fmt::Formatter<'_>,
    mnemonic: &str,
    width: RegisterWidth,
    destination: &X86Operand,
    source: &X86Operand,
) -> fmt::Result {
    write!(
        f,
        "{}{} {}, {}",
        INDENT,
        mnemonic,
        Sized(width, destination),
        Sized(width, source)
    )
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

    /// The operand as the instruction that uses it at `width` writes it.
    fn rendered(width: RegisterWidth, operand: X86Operand) -> String {
        Sized(width, &operand).to_string()
    }

    #[test]
    fn a_frame_slot_prints_its_displacement_with_a_sign() {
        // Arrange / Act / Assert
        assert_eq!(
            rendered(RegisterWidth::Quad, X86Operand::mem(X86Register::Rbp, -8)),
            "QWORD PTR [rbp - 8]"
        );
        assert_eq!(
            rendered(RegisterWidth::Quad, X86Operand::mem(X86Register::Rbp, 16)),
            "QWORD PTR [rbp + 16]"
        );
        assert_eq!(
            rendered(RegisterWidth::Quad, X86Operand::mem(X86Register::R11, 0)),
            "QWORD PTR [r11]"
        );
    }

    #[test]
    fn a_memory_operand_is_qualified_with_the_size_the_instruction_touches() {
        // Arrange / Act / Assert: the same slot, read as an `int` and as a
        // `long int`.
        let slot = X86Operand::mem(X86Register::Rbp, -8);
        assert_eq!(
            rendered(RegisterWidth::Double, slot.clone()),
            "DWORD PTR [rbp - 8]"
        );
        assert_eq!(rendered(RegisterWidth::Quad, slot), "QWORD PTR [rbp - 8]");
    }

    #[test]
    fn a_scaled_index_prints_the_whole_addressing_mode() {
        // Arrange / Act / Assert: one instruction reaches an array element,
        // scaled by the size of the element type.
        assert_eq!(
            rendered(
                RegisterWidth::Quad,
                X86Operand::indexed(X86Register::Rbp, X86Register::Rcx, 8, -24)
            ),
            "QWORD PTR [rbp + rcx*8 - 24]"
        );
        assert_eq!(
            rendered(
                RegisterWidth::Double,
                X86Operand::indexed(X86Register::Rbp, X86Register::Rcx, 4, -24)
            ),
            "DWORD PTR [rbp + rcx*4 - 24]"
        );
    }

    #[test]
    fn an_instruction_names_its_registers_at_its_own_width() {
        // Arrange / Act / Assert: `int` arithmetic is 32-bit throughout, and
        // wraps where the language says it does.
        assert_eq!(
            X86Instruction::Add(
                RegisterWidth::Double,
                X86Operand::Reg(X86Register::Rax),
                X86Operand::Reg(X86Register::R12)
            )
            .to_string(),
            "    add eax, r12d"
        );
        assert_eq!(
            X86Instruction::Add(
                RegisterWidth::Quad,
                X86Operand::Reg(X86Register::Rax),
                X86Operand::Reg(X86Register::R12)
            )
            .to_string(),
            "    add rax, r12"
        );
    }

    #[test]
    fn widening_sign_extends_from_the_source_width_to_the_destination_one() {
        // Arrange / Act / Assert: an `int` becoming a `long int` ...
        assert_eq!(
            X86Instruction::Movsx {
                to: RegisterWidth::Quad,
                from: RegisterWidth::Double,
                destination: X86Register::Rax,
                source: X86Operand::Reg(X86Register::Rcx),
            }
            .to_string(),
            "    movsx rax, ecx"
        );
        assert_eq!(
            X86Instruction::Movsx {
                to: RegisterWidth::Quad,
                from: RegisterWidth::Double,
                destination: X86Register::Rax,
                source: X86Operand::mem(X86Register::Rbp, -4),
            }
            .to_string(),
            "    movsx rax, DWORD PTR [rbp - 4]"
        );
        // ... and a `char` becoming an `int`, which is `movsbl`.
        assert_eq!(
            X86Instruction::Movsx {
                to: RegisterWidth::Double,
                from: RegisterWidth::Byte,
                destination: X86Register::Rax,
                source: X86Operand::mem(X86Register::Rbp, -1),
            }
            .to_string(),
            "    movsx eax, BYTE PTR [rbp - 1]"
        );
    }

    #[test]
    fn a_byte_register_is_named_the_way_a_rex_prefix_requires() {
        // Arrange / Act / Assert: the low byte of RSI is `sil`, never `ah`;
        // the two cannot appear in one instruction, and only `sil` can be
        // encoded alongside the extended registers this backend uses.
        assert_eq!(
            X86Instruction::Mov(
                RegisterWidth::Byte,
                X86Operand::Reg(X86Register::Rsi),
                X86Operand::Reg(X86Register::R12)
            )
            .to_string(),
            "    mov sil, r12b"
        );
        assert_eq!(
            X86Instruction::Mov(
                RegisterWidth::Byte,
                X86Operand::mem(X86Register::Rbp, -1),
                X86Operand::Imm(65)
            )
            .to_string(),
            "    mov BYTE PTR [rbp - 1], 65"
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
            X86Instruction::Movzx {
                to: RegisterWidth::Double,
                from: RegisterWidth::Byte,
                destination: X86Register::R12,
                source: X86Operand::Reg(X86Register::Rax),
            }
            .to_string(),
            "    movzx r12d, al"
        );
    }

    #[test]
    fn an_unsigned_value_widens_by_filling_with_zeroes() {
        // Arrange / Act / Assert: `movzx` where `movsx` would have copied the
        // top bit, from a byte in memory ...
        assert_eq!(
            X86Instruction::Movzx {
                to: RegisterWidth::Double,
                from: RegisterWidth::Byte,
                destination: X86Register::Rax,
                source: X86Operand::mem(X86Register::Rbp, -1),
            }
            .to_string(),
            "    movzx eax, BYTE PTR [rbp - 1]"
        );
        // ... and into the whole of a 64-bit register.
        assert_eq!(
            X86Instruction::Movzx {
                to: RegisterWidth::Quad,
                from: RegisterWidth::Byte,
                destination: X86Register::Rax,
                source: X86Operand::Reg(X86Register::Rsi),
            }
            .to_string(),
            "    movzx rax, sil"
        );
    }

    #[test]
    fn the_two_divides_are_two_instructions() {
        // Arrange / Act / Assert: same operand, different answer.
        assert_eq!(
            X86Instruction::Idiv(RegisterWidth::Double, X86Operand::Reg(X86Register::Rcx))
                .to_string(),
            "    idiv ecx"
        );
        assert_eq!(
            X86Instruction::Div(RegisterWidth::Double, X86Operand::Reg(X86Register::Rcx))
                .to_string(),
            "    div ecx"
        );
    }

    #[test]
    fn an_unsigned_comparison_uses_the_carry_flag_conditions() {
        // Arrange / Act / Assert: `below` and `above`, not `less` and
        // `greater` -- the flags an ordering reads depend on how its operands
        // read.
        assert_eq!(
            X86Instruction::SetCC(ConditionCode::B, X86Register::Rax).to_string(),
            "    setb al"
        );
        assert_eq!(
            X86Instruction::Jcc(ConditionCode::Ae, ".L1".to_string()).to_string(),
            "    jae .L1"
        );
    }

    #[test]
    fn labels_start_at_the_left_margin_and_instructions_are_indented() {
        // Arrange / Act / Assert
        assert_eq!(
            X86Instruction::Label(".main_exit".to_string()).to_string(),
            ".main_exit:"
        );
        assert_eq!(
            X86Instruction::SignExtendAccumulator(RegisterWidth::Quad).to_string(),
            "    cqo"
        );
        assert_eq!(
            X86Instruction::SignExtendAccumulator(RegisterWidth::Double).to_string(),
            "    cdq"
        );
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
