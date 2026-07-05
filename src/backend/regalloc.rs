//! Linear Scan Register Allocator
//!
//! Maps each `VirtualReg` to either a physical x86-64 register or a
//! stack spill slot.  Implements the linear-scan algorithm
//!
//! ## Register budget
//!
//! | Register | Role                                      |
//! |----------|-------------------------------------------|
//! | RAX      | Return value / `idiv` quotient — reserved |
//! | RDX      | `idiv` remainder / `cqo` — reserved       |
//! | R10      | Scratch for spill loads — reserved         |
//! | R11      | Scratch for spill loads — reserved         |
//! | RSP      | Stack pointer — reserved                  |
//! | RBP      | Frame pointer — reserved                  |
//! | *rest*   | **10 allocatable GPRs**                   |

use std::collections::{HashMap, HashSet};

use crate::backend::liveness::{LiveInterval, VirtualReg};
use crate::backend::x86::{StorageLocation, X86Register};

/// The pool of registers available for allocation.
/// Caller-saved registers are listed first so they are preferred
/// (avoids unnecessary callee-save overhead when possible).
const ALLOCATABLE_REGS: &[X86Register] = &[
    // Caller-saved (no save/restore needed unless there's a call)
    X86Register::Rcx,
    X86Register::Rsi,
    X86Register::Rdi,
    X86Register::R8,
    X86Register::R9,
    // Callee-saved (must be saved/restored in prologue/epilogue)
    X86Register::Rbx,
    X86Register::R12,
    X86Register::R13,
    X86Register::R14,
    X86Register::R15,
];

/// Returns `true` if `reg` is callee-saved under the System V AMD64 ABI.
fn is_callee_saved(reg: X86Register) -> bool {
    matches!(
        reg,
        X86Register::Rbx
            | X86Register::R12
            | X86Register::R13
            | X86Register::R14
            | X86Register::R15
    )
}

/// The output of the register allocator.
#[derive(Debug)]
pub struct AllocationResult {
    /// Every virtual register mapped to a physical register or stack slot.
    pub mapping: HashMap<VirtualReg, StorageLocation>,
    /// Number of 8-byte spill slots used (1-indexed in `StorageLocation::Stack`).
    pub stack_slots: usize,
    /// Callee-saved registers that were actually used and must be
    /// saved/restored around the function body.
    pub callee_saved_used: Vec<X86Register>,
}

/// Run linear-scan register allocation over a sorted list of live intervals.
///
/// `intervals` **must** be sorted by ascending start point (the output of
/// `compute_live_intervals` already satisfies this).
pub fn linear_scan(intervals: &[LiveInterval]) -> AllocationResult {
    // Free register pool — we pop from the end, so the *last* element is
    // allocated next.  Reversing the slice puts caller-saved regs at the
    // end so they are preferred.
    let mut free_regs: Vec<X86Register> = ALLOCATABLE_REGS.to_vec();
    free_regs.reverse();

    // Active intervals sorted by *increasing end point*.
    let mut active: Vec<(LiveInterval, X86Register)> = Vec::new();

    let mut mapping: HashMap<VirtualReg, StorageLocation> = HashMap::new();
    let mut next_spill_slot: usize = 0;
    let mut callee_saved_used: HashSet<X86Register> = HashSet::new();

    for interval in intervals {
        // --- Expire old intervals whose end point is strictly before
        //     the current interval's start point. ---
        expire_old_intervals(&mut active, &mut free_regs, interval.start);

        if free_regs.is_empty() {
            // No free register — must spill.
            spill_at_interval(
                &mut active,
                &mut free_regs,
                &mut mapping,
                &mut next_spill_slot,
                interval,
                &mut callee_saved_used,
            );
        } else {
            // Allocate the next free register.
            let reg = free_regs.pop().unwrap();
            mapping.insert(interval.vreg.clone(), StorageLocation::Register(reg));

            if is_callee_saved(reg) {
                callee_saved_used.insert(reg);
            }

            // Insert into `active`, keeping it sorted by end point.
            let pos = active.partition_point(|(a, _)| a.end <= interval.end);
            active.insert(pos, (interval.clone(), reg));
        }
    }

    // Deterministic ordering for callee-saved saves (nice for diffing output).
    let mut callee_vec: Vec<X86Register> = callee_saved_used.into_iter().collect();
    callee_vec.sort_by_key(|r| format!("{:?}", r));

    AllocationResult {
        mapping,
        stack_slots: next_spill_slot,
        callee_saved_used: callee_vec,
    }
}

/// Remove intervals from `active` whose end point is strictly before
/// `current_start`, returning their registers to the free pool.
fn expire_old_intervals(
    active: &mut Vec<(LiveInterval, X86Register)>,
    free_regs: &mut Vec<X86Register>,
    current_start: usize,
) {
    // `active` is sorted by end point — drain from the front while
    // the end point is before `current_start`.
    while let Some((interval, _)) = active.first() {
        if interval.end >= current_start {
            break;
        }
        let (_, reg) = active.remove(0);
        free_regs.push(reg);
    }
}

/// Handle the case where no free register is available.
///
/// Strategy: compare the current interval with the active interval that
/// ends latest.  Spill whichever one lives *longer* — this keeps the
/// shorter-lived value in a register, minimising total spill traffic.
fn spill_at_interval(
    active: &mut Vec<(LiveInterval, X86Register)>,
    _free_regs: &mut Vec<X86Register>,
    mapping: &mut HashMap<VirtualReg, StorageLocation>,
    next_spill_slot: &mut usize,
    current: &LiveInterval,
    callee_saved_used: &mut HashSet<X86Register>,
) {
    // `active` is sorted by end point — the last element ends latest.
    let (longest_active, _) = active.last().unwrap();

    if longest_active.end > current.end {
        // Spill the existing long-lived interval; give its register
        // to the current (shorter-lived) interval.
        let (spilled, freed_reg) = active.pop().unwrap();

        *next_spill_slot += 1;
        mapping.insert(
            spilled.vreg.clone(),
            StorageLocation::Stack(*next_spill_slot as i32),
        );

        mapping.insert(current.vreg.clone(), StorageLocation::Register(freed_reg));
        if is_callee_saved(freed_reg) {
            callee_saved_used.insert(freed_reg);
        }

        // Re-insert current into active (sorted by end).
        let pos = active.partition_point(|(a, _)| a.end <= current.end);
        active.insert(pos, (current.clone(), freed_reg));
    } else {
        // Current interval lives longest — spill it directly.
        *next_spill_slot += 1;
        mapping.insert(
            current.vreg.clone(),
            StorageLocation::Stack(*next_spill_slot as i32),
        );
    }
}
