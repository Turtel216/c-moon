//! The x86-64 code-generation target.
//!
//! The module is split along the lines a second target would also want:
//! [`isa`] is the instruction set as data, [`abi`] the calling convention,
//! [`lower`] the instruction selector, and [`emit`] the assembly text.  Only
//! [`X86Target`] is visible to the shared pipeline.

pub mod abi;
pub mod emit;
pub mod isa;
pub mod lower;

use crate::backend::frame::{FrameLayout, FrameParams};
use crate::backend::linear::LinearizedCfg;
use crate::backend::target::Target;
use crate::backend::x86::abi::SystemV;
use crate::backend::x86::isa::{X86Function, X86Program, X86Register};
use crate::backend::x86::lower::FunctionLowering;

/// x86-64 under the System V AMD64 ABI, as GNU/Linux uses.
pub struct X86Target;

impl Target for X86Target {
    type Registers = SystemV;
    type Function = X86Function;
    type Program = X86Program;

    const FRAME: FrameParams = FrameParams {
        word_size: abi::WORD_SIZE,
        stack_alignment: abi::STACK_ALIGNMENT,
    };

    fn lower_function(
        name: &str,
        body: &LinearizedCfg,
        layout: &FrameLayout<X86Register>,
    ) -> X86Function {
        FunctionLowering::lower(name, body, layout)
    }

    fn assemble(functions: Vec<X86Function>) -> X86Program {
        X86Program { functions }
    }
}
