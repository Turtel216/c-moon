//! Backend Compilation Pipeline
//!
//! Orchestrates the backend stages for each function:
//! 1. Linearize the CFG
//! 2. Compute live intervals (liveness analysis)
//! 3. Run linear-scan register allocation
//! 4. Lower TAC -> x86-64 instructions
//!
//! The result is an `X86Program` ready for text emission.

use crate::backend::liveness::{compute_live_intervals, linearize_cfg};
use crate::backend::lowering::LoweringContext;
use crate::backend::regalloc::linear_scan;
use crate::backend::x86::X86Program;
use crate::middle::desuger::ProgramIr;

/// Compile an entire IR program to x86-64.
pub fn compile_program(ir: &ProgramIr) -> X86Program {
    let mut functions = Vec::new();

    for (name, cfg) in &ir.functions {
        // Linearize the CFG into a flat instruction sequence.
        let linear = linearize_cfg(cfg);

        // Compute live intervals via liveness analysis.
        let intervals = compute_live_intervals(cfg, &linear);

        // Allocate registers
        let alloc = linear_scan(&intervals);

        // Instruction selection
        let x86_fn = LoweringContext::lower_function(
            name,
            &linear.instructions,
            &linear.block_order,
            alloc,
            &ir.array_sizes,
        );

        functions.push(x86_fn);
    }

    X86Program { functions }
}
