//! Pretty Printer for the CFG-TAC IR
//!
//! `Display` implementations that print the IR.

use crate::middle::desuger::*;
use crate::middle::ir::*;
use core::fmt::Write;
use std::fmt;

pub struct IrPrinter;

impl IrPrinter {
    pub fn print_ir(program: &ProgramIr, w: &mut impl Write) -> fmt::Result {
        write!(w, "{}", program)
    }
}

// Operand Formatting
impl fmt::Display for Operand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Operand::Var(name) => write!(f, "%r{}", name),
            Operand::Temp(name) => write!(f, "%{}", name),
            Operand::ImmInt(val) => write!(f, "{}", val),
            // Prefix labels with '.'
            Operand::Label(name) => write!(f, ".{}", name),
        }
    }
}

// Opcode Formatting
impl fmt::Display for Opcode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let op_str = match self {
            Opcode::Add => "+",
            Opcode::Sub => "-",
            Opcode::Mul => "*",
            Opcode::Div => "/",
            Opcode::Eq => "==",
            Opcode::Neq => "!=",
            Opcode::Lt => "<",
            Opcode::Lte => "<=",
            Opcode::Gt => ">",
            Opcode::Gte => ">=",
            Opcode::Mov => "=",
            Opcode::Jump => "jmp",
            Opcode::BranchIf => "br_if",
            Opcode::BranchIfNot => "br_if_not",
            Opcode::Call => "call",
            Opcode::Param => "param",
            Opcode::Ret => "ret",
            Opcode::GetParam => "get_param",
            Opcode::ArrayStore => "array_store",
            Opcode::ArrayLoad => "array_load",
        };
        write!(f, "{}", op_str)
    }
}

// TAC Instruction Formatting
impl fmt::Display for TACInstruction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Helper closure to safely format optional operands,
        // printing "_" if an expected operand is missing due to a compiler bug.
        let format_op = |op: &Option<Operand>| -> String {
            op.as_ref()
                .map_or_else(|| "_".to_string(), |o| o.to_string())
        };

        match self.opcode {
            // Binary Operations
            Opcode::Add
            | Opcode::Sub
            | Opcode::Mul
            | Opcode::Div
            | Opcode::Eq
            | Opcode::Neq
            | Opcode::Lt
            | Opcode::Lte
            | Opcode::Gt
            | Opcode::Call
            | Opcode::Gte => {
                write!(
                    f,
                    "{} = {} {} {}",
                    format_op(&self.dest),
                    format_op(&self.arg1),
                    self.opcode,
                    format_op(&self.arg2)
                )
            }
            // Data Movement
            Opcode::Mov => {
                write!(f, "{} = {}", format_op(&self.dest), format_op(&self.arg1))
            }
            // Unary Control Flow
            Opcode::Jump => {
                write!(f, "jmp {}", format_op(&self.arg1))
            }
            Opcode::Ret => {
                write!(f, "ret {}", format_op(&self.arg1))
            }
            Opcode::Param => {
                write!(f, "param {}", format_op(&self.arg1))
            }
            Opcode::GetParam => {
                write!(
                    f,
                    "{} = get_param {}",
                    format_op(&self.dest),
                    format_op(&self.arg1)
                )
            }
            // Binary Control Flow
            Opcode::BranchIf | Opcode::BranchIfNot => {
                write!(
                    f,
                    "{} {} goto {}",
                    self.opcode,
                    format_op(&self.arg1),
                    format_op(&self.arg2)
                )
            }

            // Array operations
            Opcode::ArrayStore => {
                // array_store base[index] = value
                write!(
                    f,
                    "array_store {}[{}] = {}",
                    format_op(&self.dest),
                    format_op(&self.arg1),
                    format_op(&self.arg2)
                )
            }
            Opcode::ArrayLoad => {
                // dest = array_load base[index]
                write!(
                    f,
                    "{} = array_load {}[{}]",
                    format_op(&self.dest),
                    format_op(&self.arg1),
                    format_op(&self.arg2)
                )
            }
        }
    }
}

// Basic Block Formatting
impl fmt::Display for BasicBlock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, ".{}:", self.label)?;

        // Print edges as comments to make CFG debugging easier
        if !self.predecessors.is_empty() {
            writeln!(f, "    /* preds: {} */", self.predecessors.join(", "))?;
        }

        for instr in &self.instructions {
            writeln!(f, "    {}", instr)?;
        }

        if !self.successors.is_empty() {
            writeln!(f, "    /* succs: {} */", self.successors.join(", "))?;
        }

        Ok(())
    }
}

// CFG Formatting
impl fmt::Display for CFG {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for block in self.blocks.values() {
            writeln!(f, "{}", block)?;
        }

        Ok(())
    }
}

impl fmt::Display for ProgramIr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "=== PROGRAM ===")?;
        for fun in self.functions.clone() {
            writeln!(f, "{}", fun.1)?;
        }

        Ok(())
    }
}
