//! Backend Compilation Pipeline
//!
//! Orchestrates the backend stages for each function:
//! 1. Linearize the CFG
//! 2. Compute live intervals (liveness analysis) and locate call sites
//! 3. Run linear-scan register allocation
//! 4. Lower TAC -> x86-64 instructions
//!
//! The result is an `X86Program` ready for text emission.

use std::collections::HashSet;

use crate::backend::liveness::{compute_live_intervals, find_call_sites, linearize_cfg};
use crate::backend::lowering::LoweringContext;
use crate::backend::regalloc::linear_scan;
use crate::backend::x86::X86Program;
use crate::middle::desuger::ProgramIr;
use crate::middle::ir::{Opcode, Operand};

/// Compile an entire IR program to x86-64.
pub fn compile_program(ir: &ProgramIr) -> X86Program {
    let mut functions = Vec::new();

    for (name, cfg) in &ir.functions {
        // Linearize the CFG into a flat instruction sequence.
        let linear = linearize_cfg(cfg);

        // Compute live intervals via liveness analysis.
        let intervals = compute_live_intervals(cfg, &linear);

        // Locate the calls, so values live across one are kept out of
        // caller-saved registers.
        let call_sites = find_call_sites(&linear);

        // Allocate registers
        let alloc = linear_scan(&intervals, &call_sites);

        // Pre-scan for address-taken variables (those used in AddrOf).
        let mut addr_taken_vars: HashSet<usize> = HashSet::new();
        for (instr, _block) in &linear.instructions {
            if instr.opcode == Opcode::AddrOf {
                if let Some(Operand::Var(id)) = &instr.arg1 {
                    addr_taken_vars.insert(*id);
                }
            }
        }

        // Instruction selection
        let x86_fn = LoweringContext::lower_function(
            name,
            &linear.instructions,
            &linear.block_order,
            alloc,
            &ir.array_sizes,
            &addr_taken_vars,
        );

        functions.push(x86_fn);
    }

    X86Program { functions }
}
