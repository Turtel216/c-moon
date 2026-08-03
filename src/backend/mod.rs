//! Compiler backend: code generation.
//!
//! The backend is split into stages that are the same for every machine and a
//! module per target that supplies the rest.
//!
//! Target-independent:
//! - [`vreg`] -- the virtual-register view of the TAC IR.
//! - [`linear`] -- one total order over a function's instructions.
//! - [`liveness`] -- how long each value must be kept.
//! - [`regalloc`] -- linear-scan allocation over any [`regalloc::RegisterFile`].
//! - [`frame`] -- where spills, arrays and address-taken variables live.
//! - [`target`] -- the [`target::Target`] trait and the pipeline over it.
//!
//! Per target:
//! - [`x86`] -- x86-64 with the System V AMD64 ABI.
//!
//! Adding an architecture therefore means describing its registers, its frame
//! parameters, its instruction selection and its assembly syntax; everything
//! before instruction selection is reused as it stands.

pub mod frame;
pub mod linear;
pub mod liveness;
pub mod regalloc;
pub mod target;
pub mod vreg;
pub mod x86;

use crate::middle::desuger::ProgramIr;

/// The target this compiler emits code for.
pub type DefaultTarget = x86::X86Target;

/// Compile an IR program to assembly text for the default target.
///
/// # Returns
///
/// The complete contents of a `.s` file, ready to hand to an assembler.
pub fn compile_to_assembly(ir: &ProgramIr) -> String {
    target::compile_program::<DefaultTarget>(ir).to_string()
}
