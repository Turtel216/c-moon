//! Pretty Printer for the CFG-TAC IR
//!
//! `Display` implementations that print the IR.

use crate::middle::desuger::*;
use crate::middle::ir::*;
use crate::middle::ssa;
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
            Opcode::Load => "load",
            Opcode::Store => "store",
            Opcode::AddrOf => "addr_of",
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

            // Pointer operations
            Opcode::Load => {
                // dest = load addr
                write!(
                    f,
                    "{} = load {}",
                    format_op(&self.dest),
                    format_op(&self.arg1)
                )
            }
            Opcode::Store => {
                // store addr, value
                write!(
                    f,
                    "store {}, {}",
                    format_op(&self.arg1),
                    format_op(&self.arg2)
                )
            }
            Opcode::AddrOf => {
                // dest = addr_of var
                write!(
                    f,
                    "{} = addr_of {}",
                    format_op(&self.dest),
                    format_op(&self.arg1)
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

// ### SSA IR ###

/// One SSA value, printed in the context of the function that defines it.
///
/// A value that came from a source variable prints as that variable and its
/// version, so a dump can be read against the program it was compiled from;
/// one the compiler invented prints as a plain number.
pub struct ValueText<'a> {
    pub function: &'a ssa::Function,
    pub value: ssa::ValueId,
}

impl fmt::Display for ValueText<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.function.value_def(self.value).source {
            Some(source) => write!(f, "%r{}.{}", source.variable, source.version),
            None => write!(f, "%v{}", self.value.index()),
        }
    }
}

/// One SSA operand.
pub struct OperandText<'a> {
    pub function: &'a ssa::Function,
    pub operand: ssa::Operand,
}

impl fmt::Display for OperandText<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.operand {
            ssa::Operand::Value(value) => write!(
                f,
                "{}",
                ValueText {
                    function: self.function,
                    value
                }
            ),
            ssa::Operand::Imm(constant) => write!(f, "{}", constant),
        }
    }
}

/// One memory location, printed as the TAC operand it stands for.
pub struct SlotText<'a> {
    pub function: &'a ssa::Function,
    pub slot: ssa::SlotId,
}

impl fmt::Display for SlotText<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.function.slot(self.slot).origin {
            ssa::SlotOrigin::Variable(id) => write!(f, "%r{}", id),
            ssa::SlotOrigin::Temporary(name) => write!(f, "%{}", name),
        }
    }
}

/// One SSA instruction, printed in the context of the function that owns it.
pub struct InstText<'a> {
    pub function: &'a ssa::Function,
    pub inst: ssa::InstId,
}

impl fmt::Display for InstText<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let function = self.function;
        let instr = function.inst(self.inst);
        let operand = |operand| OperandText { function, operand };
        let slot = |slot| SlotText { function, slot };

        if let Some(dest) = instr.dest {
            write!(
                f,
                "{} = ",
                ValueText {
                    function,
                    value: dest
                }
            )?;
        }

        match &instr.op {
            ssa::Op::Binary(operator, lhs, rhs) => write!(
                f,
                "{} {} {}",
                operand(*lhs),
                binary_symbol(*operator),
                operand(*rhs)
            ),
            ssa::Op::Copy(source) => write!(f, "{}", operand(*source)),
            ssa::Op::Call { callee, args } => {
                write!(f, "call {}(", callee)?;
                for (position, argument) in args.iter().enumerate() {
                    if position > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", operand(*argument))?;
                }
                write!(f, ")")
            }
            ssa::Op::GetParam(index) => write!(f, "get_param {}", index),
            ssa::Op::Undef => write!(f, "undef"),
            ssa::Op::SlotLoad { slot: from } => write!(f, "load_slot {}", slot(*from)),
            ssa::Op::SlotStore { slot: into, value } => {
                write!(f, "store_slot {}, {}", slot(*into), operand(*value))
            }
            ssa::Op::ArrayLoad { base, index } => {
                write!(f, "array_load {}[{}]", slot(*base), operand(*index))
            }
            ssa::Op::ArrayStore { base, index, value } => write!(
                f,
                "array_store {}[{}] = {}",
                slot(*base),
                operand(*index),
                operand(*value)
            ),
            ssa::Op::Load { address } => write!(f, "load {}", operand(*address)),
            ssa::Op::Store { address, value } => {
                write!(f, "store {}, {}", operand(*address), operand(*value))
            }
            ssa::Op::AddrOf { slot: of } => write!(f, "addr_of {}", slot(*of)),
        }
    }
}

/// One basic block in SSA form, with its predecessors and its phi nodes.
pub struct BlockText<'a> {
    pub function: &'a ssa::Function,
    pub block: ssa::BlockId,
}

impl fmt::Display for BlockText<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let function = self.function;
        let block = function.block(self.block);
        let label = |block: ssa::BlockId| function.block(block).label.clone();

        writeln!(f, ".{}:", block.label)?;
        if !block.preds().is_empty() {
            let preds: Vec<String> = block.preds().iter().map(|&pred| label(pred)).collect();
            writeln!(f, "    /* preds: {} */", preds.join(", "))?;
        }

        for phi in &block.phis {
            write!(
                f,
                "    {} = phi",
                ValueText {
                    function,
                    value: phi.dest
                }
            )?;
            for (position, argument) in phi.args.iter().enumerate() {
                let from = block
                    .preds()
                    .get(position)
                    .map_or_else(|| "?".to_string(), |&pred| label(pred));
                write!(
                    f,
                    " [{} from .{}]",
                    OperandText {
                        function,
                        operand: *argument
                    },
                    from
                )?;
            }
            writeln!(f)?;
        }

        for &inst in &block.insts {
            writeln!(f, "    {}", InstText { function, inst })?;
        }

        match block.terminator() {
            ssa::Terminator::Jump(target) => writeln!(f, "    jmp .{}", label(*target)),
            ssa::Terminator::Branch {
                cond,
                then_block,
                else_block,
            } => writeln!(
                f,
                "    br {} ? .{} : .{}",
                OperandText {
                    function,
                    operand: *cond
                },
                label(*then_block),
                label(*else_block)
            ),
            ssa::Terminator::Return(None) => writeln!(f, "    ret"),
            ssa::Terminator::Return(Some(value)) => writeln!(
                f,
                "    ret {}",
                OperandText {
                    function,
                    operand: *value
                }
            ),
        }
    }
}

impl fmt::Display for ssa::Function {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "function {} {{", self.name)?;
        for block in self.block_ids() {
            write!(
                f,
                "{}",
                BlockText {
                    function: self,
                    block
                }
            )?;
        }
        writeln!(f, "}}")
    }
}

/// The symbol a binary operator prints as.
fn binary_symbol(operator: ssa::BinOp) -> &'static str {
    match operator {
        ssa::BinOp::Add => "+",
        ssa::BinOp::Sub => "-",
        ssa::BinOp::Mul => "*",
        ssa::BinOp::Div => "/",
        ssa::BinOp::Eq => "==",
        ssa::BinOp::Neq => "!=",
        ssa::BinOp::Lt => "<",
        ssa::BinOp::Lte => "<=",
        ssa::BinOp::Gt => ">",
        ssa::BinOp::Gte => ">=",
    }
}
