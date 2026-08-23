//! Virtual registers: the backend's view of TAC operands.
//!
//! The shared backend never looks at a `TACInstruction` field directly.  It
//! asks this module three questions instead -- which operands does an
//! instruction mention, which values does it read, and which value does it
//! write -- and every answer is phrased in [`VirtualReg`]s.  Liveness
//! analysis, register allocation and frame layout are built on nothing else,
//! which is what keeps them independent of both the source language and the
//! target machine.

use crate::middle::ir::{Opcode, Operand, TACInstruction};

/// A value the register allocator can place: a TAC variable or temporary.
///
/// TAC distinguishes renamed source variables (`Var(usize)`) from
/// compiler-generated temporaries (`Temp(String)`); to the backend they are
/// the same thing, an unlimited supply of virtual registers.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum VirtualReg {
    /// A compiler-generated temporary, e.g. `t1`, `t2`.
    Temp(String),
    /// A renamed source-level variable, e.g. var #0, var #1.
    Var(usize),
}

impl std::fmt::Display for VirtualReg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VirtualReg::Temp(name) => write!(f, "%{}", name),
            VirtualReg::Var(id) => write!(f, "%r{}", id),
        }
    }
}

/// Try to read a TAC `Operand` as a virtual register.
///
/// # Returns
///
/// `None` for immediates and labels -- they name no storage, so they are not
/// virtual registers and take no part in allocation.
pub fn operand_to_vreg(operand: &Operand) -> Option<VirtualReg> {
    match operand {
        Operand::Var(id) => Some(VirtualReg::Var(*id)),
        Operand::Temp(name) => Some(VirtualReg::Temp(name.clone())),
        Operand::ImmInt(_) | Operand::Label(_) => None,
    }
}

/// Every operand an instruction mentions, in `dest`, `arg1`, `arg2` order.
///
/// This is a syntactic view: it makes no claim about which operands are read
/// and which are written.  Use [`instruction_uses`] and [`instruction_def`]
/// for that.
pub fn instruction_operands(instr: &TACInstruction) -> impl Iterator<Item = &Operand> {
    // `&Option<T>` iterates over zero or one `&T`, so `flatten` drops the
    // absent fields -- a Rust idiom that replaces three `if let` arms.
    [&instr.dest, &instr.arg1, &instr.arg2]
        .into_iter()
        .flatten()
}

/// The virtual registers an instruction reads, in operand order.
pub fn instruction_uses(instr: &TACInstruction) -> impl Iterator<Item = VirtualReg> + '_ {
    let reads = data_flow(&instr.opcode).reads;

    [
        (reads.dest, &instr.dest),
        (reads.arg1, &instr.arg1),
        (reads.arg2, &instr.arg2),
    ]
    .into_iter()
    .filter(|(is_read, _)| *is_read)
    .filter_map(|(_, field)| field.as_ref().and_then(operand_to_vreg))
}

/// The virtual register an instruction writes, if any.
pub fn instruction_def(instr: &TACInstruction) -> Option<VirtualReg> {
    if !data_flow(&instr.opcode).writes_dest {
        return None;
    }
    instr.dest.as_ref().and_then(operand_to_vreg)
}

// ### Opcode dataflow table ###

/// Which of the three operand fields an instruction reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Reads {
    dest: bool,
    arg1: bool,
    arg2: bool,
}

/// Reads none of its operands.
const NOTHING: Reads = Reads {
    dest: false,
    arg1: false,
    arg2: false,
};
/// Reads `arg1` only.
const ARG1: Reads = Reads {
    arg1: true,
    ..NOTHING
};
/// Reads both arguments.
const BOTH_ARGS: Reads = Reads { arg2: true, ..ARG1 };
/// Reads `dest` as an input as well as both arguments.
const DEST_AND_ARGS: Reads = Reads {
    dest: true,
    ..BOTH_ARGS
};

/// The dataflow behaviour of one opcode.
///
/// Every TAC instruction has the same `dest`/`arg1`/`arg2` shape, so its
/// dataflow is captured entirely by which fields it reads and whether it
/// writes `dest`.  Keeping that in one table -- rather than spread over a
/// `use` match and a `def` match -- means a new opcode is described once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DataFlow {
    reads: Reads,
    writes_dest: bool,
}

/// Describe how `opcode` moves values between its operand fields.
fn data_flow(opcode: &Opcode) -> DataFlow {
    /// Shorthand for a table entry.
    const fn flow(reads: Reads, writes_dest: bool) -> DataFlow {
        DataFlow { reads, writes_dest }
    }

    match opcode {
        // Arithmetic and comparison: `dest = arg1 <op> arg2`.
        Opcode::Add
        | Opcode::Sub
        | Opcode::Mul
        | Opcode::Div
        | Opcode::Eq
        | Opcode::Neq
        | Opcode::Lt
        | Opcode::Lte
        | Opcode::Gt
        | Opcode::Gte => flow(BOTH_ARGS, true),

        // `dest = arg1`, `dest = *arg1`, and `dest = (width) arg1`.
        Opcode::Mov | Opcode::Load | Opcode::Convert => flow(ARG1, true),

        // `dest = arg1[arg2]`.
        Opcode::ArrayLoad => flow(BOTH_ARGS, true),

        // `arg1[arg2] = ...`: `dest` names the array being written *into*,
        // so it is an input, not a definition.
        Opcode::ArrayStore => flow(DEST_AND_ARGS, false),

        // `*arg1 = arg2` writes memory, not a virtual register.
        Opcode::Store => flow(BOTH_ARGS, false),

        // Control flow and argument passing read the value they test or pass.
        Opcode::BranchIf | Opcode::BranchIfNot | Opcode::Param | Opcode::Ret => flow(ARG1, false),

        // `arg1` is a label or an immediate index, never a value: a call
        // names its callee, `GetParam` an argument position, `Jump` a target.
        Opcode::Call | Opcode::GetParam => flow(NOTHING, true),
        Opcode::Jump => flow(NOTHING, false),

        // `AddrOf` needs its operand's *address*, not its value, so the
        // variable is not live-in here -- it is pinned to the stack instead.
        Opcode::AddrOf => flow(NOTHING, true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::middle::ir::Width;

    fn instruction(opcode: Opcode) -> TACInstruction {
        TACInstruction::new(
            opcode,
            Width::Bits64,
            Some(Operand::Var(0)),
            Some(Operand::Temp("t1".to_string())),
            Some(Operand::Var(2)),
        )
    }

    fn uses(instr: &TACInstruction) -> Vec<VirtualReg> {
        instruction_uses(instr).collect()
    }

    #[test]
    fn a_binary_operation_reads_both_arguments_and_writes_its_destination() {
        // Arrange
        let instr = instruction(Opcode::Add);

        // Act / Assert
        assert_eq!(
            uses(&instr),
            vec![VirtualReg::Temp("t1".to_string()), VirtualReg::Var(2)]
        );
        assert_eq!(instruction_def(&instr), Some(VirtualReg::Var(0)));
    }

    #[test]
    fn an_array_store_reads_its_destination_and_defines_nothing() {
        // The array being written is an input: the instruction stores into
        // memory it addresses, it does not produce a new value.
        let instr = instruction(Opcode::ArrayStore);

        assert_eq!(
            uses(&instr),
            vec![
                VirtualReg::Var(0),
                VirtualReg::Temp("t1".to_string()),
                VirtualReg::Var(2),
            ]
        );
        assert_eq!(instruction_def(&instr), None);
    }

    #[test]
    fn taking_an_address_does_not_read_the_variable() {
        // Arrange: `dest = &arg1` needs a location, not a value.
        let instr = instruction(Opcode::AddrOf);

        // Act / Assert
        assert!(uses(&instr).is_empty());
        assert_eq!(instruction_def(&instr), Some(VirtualReg::Var(0)));
    }

    #[test]
    fn immediates_and_labels_are_not_virtual_registers() {
        // Arrange: `dest = call arg1(label), arg2(count)`.
        let call = TACInstruction::new(
            Opcode::Call,
            Width::Bits64,
            Some(Operand::Temp("t9".to_string())),
            Some(Operand::Label("f".to_string())),
            Some(Operand::ImmInt(0)),
        );

        // Act / Assert
        assert!(uses(&call).is_empty());
        assert_eq!(
            instruction_def(&call),
            Some(VirtualReg::Temp("t9".to_string()))
        );
        assert_eq!(instruction_operands(&call).count(), 3);
    }
}
