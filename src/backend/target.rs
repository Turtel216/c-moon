//! The target abstraction and the shared backend pipeline.
//!
//! Everything a machine has to describe in order to be a code-generation
//! target is gathered in the [`Target`] trait.  [`compile_program`] then runs
//! the stages that are the same for every machine -- linearization, liveness,
//! register allocation and frame layout -- and asks the target only for the
//! two steps that genuinely depend on it: instruction selection and assembly.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Display;

use crate::backend::frame::{FrameLayout, FrameParams};
use crate::backend::linear::{LinearizedCfg, linearize_cfg};
use crate::backend::liveness::{compute_live_intervals, find_call_sites};
use crate::backend::regalloc::{PhysReg, RegisterFile, linear_scan};
use crate::backend::vreg::instruction_operands;
use crate::middle::desuger::ProgramIr;
use crate::middle::ir::{CFG, Opcode, Operand};

/// A code-generation target.
///
/// Implementing this trait is all a new architecture needs in order to reuse
/// the whole shared backend.
pub trait Target {
    /// The target's allocatable registers and calling-convention roles.
    type Registers: RegisterFile;
    /// One function after instruction selection.
    type Function;
    /// A whole program, printable as assembly text.
    type Program: Display;

    /// The word size and stack alignment its frames are laid out with.
    const FRAME: FrameParams;

    /// Select instructions for one function.
    ///
    /// # Arguments
    ///
    /// * `name` - the function's symbol name
    /// * `body` - its instructions in linear order, grouped into blocks
    /// * `layout` - where every value of the function lives
    fn lower_function(
        name: &str,
        body: &LinearizedCfg,
        layout: &FrameLayout<PhysReg<Self::Registers>>,
    ) -> Self::Function;

    /// Combine the lowered functions into a complete program.
    fn assemble(functions: Vec<Self::Function>) -> Self::Program;
}

/// Compile a whole IR program for target `T`.
pub fn compile_program<T: Target>(ir: &ProgramIr) -> T::Program {
    let functions = ir
        .functions
        .iter()
        .map(|(name, cfg)| compile_function::<T>(name, cfg, &ir.array_sizes))
        .collect();

    T::assemble(functions)
}

/// Run every backend stage over a single function.
fn compile_function<T: Target>(
    name: &str,
    cfg: &CFG,
    array_sizes: &HashMap<usize, usize>,
) -> T::Function {
    // One total order over the instructions, which liveness and allocation
    // both measure positions in.
    let body = linearize_cfg(cfg);

    // How long each value must be kept, and where a callee could destroy it.
    let intervals = compute_live_intervals(cfg, &body);
    let call_sites = find_call_sites(&body);

    let allocation = linear_scan::<T::Registers>(&intervals, &call_sites);

    let (arrays, addr_taken) = frame_requirements(&body, array_sizes);
    let layout = FrameLayout::plan(T::FRAME, allocation, &arrays, &addr_taken);

    T::lower_function(name, &body, &layout)
}

/// The frame storage this function needs beyond its spill slots.
///
/// `array_sizes` covers the whole program, so it is narrowed to the arrays
/// this function actually mentions -- otherwise every function would reserve
/// space for every array in the program.
///
/// # Returns
///
/// The element count of each array the function touches, and the variables
/// whose address it takes.  Both are ordered so that the layout, and with it
/// the emitted assembly, is reproducible.
fn frame_requirements(
    body: &LinearizedCfg,
    array_sizes: &HashMap<usize, usize>,
) -> (BTreeMap<usize, usize>, BTreeSet<usize>) {
    let mut arrays = BTreeMap::new();
    let mut addr_taken = BTreeSet::new();

    for instr in body.instructions() {
        for operand in instruction_operands(instr) {
            if let Operand::Var(id) = operand
                && let Some(&count) = array_sizes.get(id)
            {
                arrays.insert(*id, count);
            }
        }

        // A variable whose address escapes cannot live in a register alone:
        // the pointer must have something to point at.
        if instr.opcode == Opcode::AddrOf
            && let Some(Operand::Var(id)) = &instr.arg1
        {
            addr_taken.insert(*id);
        }
    }

    (arrays, addr_taken)
}
