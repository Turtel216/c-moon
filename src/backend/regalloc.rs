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
//!
//! ## Calls
//!
//! A callee may destroy every caller-saved register, so any value that is
//! live on both sides of a `call` is restricted to a callee-saved register
//! and spilled to the stack when none is available.

use std::collections::{HashMap, HashSet};

use crate::backend::liveness::{CallSite, LiveInterval, VirtualReg};
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
/// `compute_live_intervals` already satisfies this), and `call_sites` by
/// ascending position (the output of `find_call_sites` already does).
pub fn linear_scan(intervals: &[LiveInterval], call_sites: &[CallSite]) -> AllocationResult {
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

        // A value held across a call survives only in a callee-saved
        // register; the callee may clobber all the others.
        let needs_callee_saved = crosses_any_call(interval, call_sites);

        // Pick the next free register, honouring that restriction.
        // `free_regs` is reversed, so the *last* match is the one listed
        // earliest in `ALLOCATABLE_REGS` and therefore the most preferred.
        let choice = if needs_callee_saved {
            free_regs.iter().rposition(|reg| is_callee_saved(*reg))
        } else {
            free_regs.len().checked_sub(1)
        };

        match choice {
            Some(index) => {
                let reg = free_regs.remove(index);
                mapping.insert(interval.vreg.clone(), StorageLocation::Register(reg));

                if is_callee_saved(reg) {
                    callee_saved_used.insert(reg);
                }

                // Insert into `active`, keeping it sorted by end point.
                let pos = active.partition_point(|(a, _)| a.end <= interval.end);
                active.insert(pos, (interval.clone(), reg));
            }
            // No register the interval is allowed to use — must spill.
            None => spill_at_interval(
                &mut active,
                &mut mapping,
                &mut next_spill_slot,
                interval,
                needs_callee_saved,
                &mut callee_saved_used,
            ),
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

/// Returns `true` if `interval` is live across any call in `call_sites`.
///
/// `call_sites` is sorted by position, so a binary search skips straight to
/// the first call that could overlap the interval instead of scanning them
/// all.
fn crosses_any_call(interval: &LiveInterval, call_sites: &[CallSite]) -> bool {
    let first_candidate = call_sites.partition_point(|call| call.position < interval.start);
    call_sites[first_candidate..]
        .iter()
        .take_while(|call| call.position <= interval.end)
        .any(|call| interval.crosses_call(call))
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

/// Handle the case where no register the current interval may use is free.
///
/// Strategy: compare the current interval with the longest-lived active
/// interval holding a register it is allowed to take.  Spill whichever one
/// lives *longer* — this keeps the shorter-lived value in a register,
/// minimising total spill traffic.  Spilling is always safe across a call:
/// stack slots live in this function's own frame, which the callee cannot
/// touch.
fn spill_at_interval(
    active: &mut Vec<(LiveInterval, X86Register)>,
    mapping: &mut HashMap<VirtualReg, StorageLocation>,
    next_spill_slot: &mut usize,
    current: &LiveInterval,
    needs_callee_saved: bool,
    callee_saved_used: &mut HashSet<X86Register>,
) {
    // `active` is sorted by end point, so the last candidate ends latest.
    let victim = active
        .iter()
        .rposition(|(_, reg)| !needs_callee_saved || is_callee_saved(*reg));

    match victim {
        Some(index) if active[index].0.end > current.end => {
            // Spill the existing long-lived interval; give its register
            // to the current (shorter-lived) interval.
            let (spilled, freed_reg) = active.remove(index);

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
        }
        // Current interval lives longest (or nothing may be evicted) —
        // spill it directly.
        _ => {
            *next_spill_slot += 1;
            mapping.insert(
                current.vreg.clone(),
                StorageLocation::Stack(*next_spill_slot as i32),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn interval(name: &str, start: usize, end: usize) -> LiveInterval {
        LiveInterval {
            vreg: VirtualReg::Temp(name.to_string()),
            start,
            end,
        }
    }

    fn call_at(position: usize) -> CallSite {
        CallSite {
            position,
            defines: None,
        }
    }

    fn location_of(alloc: &AllocationResult, name: &str) -> StorageLocation {
        *alloc
            .mapping
            .get(&VirtualReg::Temp(name.to_string()))
            .expect("vreg was not allocated")
    }

    #[test]
    fn values_not_spanning_a_call_prefer_caller_saved_registers() {
        // Arrange: one short-lived value, one call it ends before.
        let intervals = [interval("short", 0, 2)];

        // Act
        let alloc = linear_scan(&intervals, &[call_at(5)]);

        // Assert: no callee-save overhead was taken on.
        assert_eq!(
            location_of(&alloc, "short"),
            StorageLocation::Register(X86Register::Rcx)
        );
        assert!(alloc.callee_saved_used.is_empty());
    }

    #[test]
    fn values_spanning_a_call_get_a_callee_saved_register() {
        // Arrange: the value is live on both sides of the call at index 3.
        let intervals = [interval("across", 0, 6)];

        // Act
        let alloc = linear_scan(&intervals, &[call_at(3)]);

        // Assert
        let StorageLocation::Register(reg) = location_of(&alloc, "across") else {
            panic!("expected a register allocation");
        };
        assert!(
            is_callee_saved(reg),
            "{:?} is caller-saved and would be destroyed by the call",
            reg
        );
        assert_eq!(alloc.callee_saved_used, vec![reg]);
    }

    #[test]
    fn a_value_produced_by_a_call_is_not_forced_to_callee_saved() {
        // Arrange: the interval starts *at* the call that defines it, so it
        // only comes into existence once the callee has returned.
        let returned = VirtualReg::Temp("returned".to_string());
        let intervals = [interval("returned", 4, 9)];
        let call = CallSite {
            position: 4,
            defines: Some(returned),
        };

        // Act
        let alloc = linear_scan(&intervals, &[call]);

        // Assert
        assert_eq!(
            location_of(&alloc, "returned"),
            StorageLocation::Register(X86Register::Rcx)
        );
    }

    #[test]
    fn excess_values_spanning_a_call_are_spilled_not_left_in_caller_saved() {
        // Arrange: eight values live across one call, but only five
        // callee-saved registers exist.
        let intervals: Vec<LiveInterval> = (0..8)
            .map(|index| interval(&format!("v{}", index), 0, 20))
            .collect();

        // Act
        let alloc = linear_scan(&intervals, &[call_at(10)]);

        // Assert: every one is either callee-saved or on the stack.
        for index in 0..8 {
            match location_of(&alloc, &format!("v{}", index)) {
                StorageLocation::Register(reg) => assert!(
                    is_callee_saved(reg),
                    "v{} landed in caller-saved {:?}",
                    index,
                    reg
                ),
                StorageLocation::Stack(_) => {}
            }
        }
        assert_eq!(alloc.stack_slots, 3, "8 values - 5 callee-saved registers");
    }

    #[test]
    fn intervals_ending_at_a_call_do_not_span_it() {
        // An argument's last use is the instruction before the call, so it
        // may stay in a caller-saved register.
        assert!(!interval("arg", 0, 4).crosses_call(&call_at(5)));
        assert!(interval("arg", 0, 5).crosses_call(&call_at(5)));
    }
}
