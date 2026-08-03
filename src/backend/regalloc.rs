//! Linear-scan register allocation, independent of any target.
//!
//! The allocator knows nothing about x86: it is parameterized over a
//! [`RegisterFile`], which is the only thing a new target has to describe in
//! order to reuse it.
//!
//! ## Algorithm
//!
//! Intervals are walked in order of increasing start point while a set of
//! *active* intervals -- those currently holding a register -- is kept sorted
//! by end point.  Before each interval, every active interval that has
//! already ended returns its register to the pool.  If no permitted register
//! is free, the longest-lived candidate is spilled (see
//! [`linear_scan`] for the choice between it and the
//! current interval).
//!
//! ## Calls
//!
//! A callee may destroy every caller-saved register, so a value that is live
//! on both sides of a call is restricted to a callee-saved register, and
//! spilled to the frame when none is available.

use std::collections::HashMap;
use std::fmt::Debug;
use std::hash::Hash;

use crate::backend::liveness::{CallSite, LiveInterval};
use crate::backend::vreg::VirtualReg;

/// A target's allocatable registers and their calling-convention roles.
///
/// This is the whole interface between the allocator and a machine.  A RISC-V
/// or ARM backend implements it over its own register enum and gets
/// linear-scan allocation for free.
pub trait RegisterFile {
    /// The target's physical register type, normally a `Copy` enum.
    ///
    /// `'static` because [`Self::allocatable`] hands out a borrow of a table
    /// that lives for the whole program.
    type Register: Copy + Eq + Ord + Hash + Debug + 'static;

    /// The registers the allocator may hand out, most preferred first.
    ///
    /// Listing caller-saved registers first avoids paying for a
    /// save/restore in functions that never need one.
    fn allocatable() -> &'static [Self::Register];

    /// Returns `true` if the ABI requires a callee to preserve `register`.
    fn is_callee_saved(register: Self::Register) -> bool;
}

/// The physical register type of a register file.
///
/// Rust has no way to shorten `<F as RegisterFile>::Register` at use sites,
/// so this alias stands in for it in every generic signature.
pub type PhysReg<F> = <F as RegisterFile>::Register;

/// Where the allocator decided a virtual register lives.
///
/// A spill is identified by slot number rather than by address: laying slots
/// out in the frame is [`FrameLayout`](crate::backend::frame::FrameLayout)'s
/// job, not the allocator's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Assignment<R> {
    /// Held in a physical register.
    Register(R),
    /// Spilled to frame slot `n`, counted from 1.
    Spill(usize),
}

/// The result of allocating one function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Allocation<R> {
    assignments: HashMap<VirtualReg, Assignment<R>>,
    spill_slots: usize,
    callee_saved: Vec<R>,
}

impl<R: Copy + Eq + Hash> Allocation<R> {
    /// Where `vreg` lives.
    ///
    /// # Panics
    ///
    /// Panics if `vreg` was never allocated, which can only happen if the
    /// intervals handed to [`linear_scan`] did not cover the whole function.
    pub fn assignment_of(&self, vreg: &VirtualReg) -> Assignment<R> {
        *self
            .assignments
            .get(vreg)
            .unwrap_or_else(|| panic!("Compiler Bug: {} has no allocation", vreg))
    }

    /// The register holding `vreg`, or `None` if it was spilled.
    pub fn register_of(&self, vreg: &VirtualReg) -> Option<R> {
        match self.assignment_of(vreg) {
            Assignment::Register(register) => Some(register),
            Assignment::Spill(_) => None,
        }
    }

    /// How many frame slots the spilled values need.
    pub fn spill_slots(&self) -> usize {
        self.spill_slots
    }

    /// The callee-saved registers this function actually used, and must
    /// therefore save and restore.  Ordered as in [`RegisterFile::allocatable`].
    pub fn callee_saved(&self) -> &[R] {
        &self.callee_saved
    }
}

/// An interval that currently holds a register.
struct Active {
    /// Index into [`RegisterFile::allocatable`] of the register held.
    register: usize,
    /// Last instruction index at which the value is live.
    end: usize,
    /// The value itself, needed if it later has to be spilled.
    vreg: VirtualReg,
}

/// Assign every interval a register or a spill slot.
///
/// `intervals` **must** be sorted by ascending start point and `call_sites` by
/// ascending position; the [`liveness`](crate::backend::liveness) module
/// already guarantees both.
///
/// When a register is needed and none is available, the longest-lived of the
/// candidates is spilled: keeping the shorter-lived value in a register
/// minimises total spill traffic.  Spilling is always safe across a call,
/// because frame slots belong to this function and the callee cannot touch
/// them.
pub fn linear_scan<F: RegisterFile>(
    intervals: &[LiveInterval],
    call_sites: &[CallSite],
) -> Allocation<PhysReg<F>> {
    let registers = F::allocatable();

    // Availability is tracked by index so that a freed register goes back to
    // its place in the preference order rather than to the end of a pool.
    let mut available = vec![true; registers.len()];
    let mut used_callee_saved = vec![false; registers.len()];
    // Sorted by increasing end point.
    let mut active: Vec<Active> = Vec::with_capacity(registers.len());

    let mut assignments: HashMap<VirtualReg, Assignment<PhysReg<F>>> =
        HashMap::with_capacity(intervals.len());
    let mut spill_slots = 0;

    for interval in intervals {
        expire_before(&mut active, &mut available, interval.start);

        // A value held across a call survives only in a callee-saved
        // register; the callee may clobber all the others.
        let restricted = crosses_any_call(interval, call_sites);

        match pick_register::<F>(&available, restricted) {
            Some(index) => {
                available[index] = false;
                used_callee_saved[index] |= F::is_callee_saved(registers[index]);
                assignments.insert(
                    interval.vreg.clone(),
                    Assignment::Register(registers[index]),
                );
                insert_active(
                    &mut active,
                    Active {
                        register: index,
                        end: interval.end,
                        vreg: interval.vreg.clone(),
                    },
                );
            }
            None => {
                // Nothing is free, so either an active value or this one has
                // to go to the frame.
                let victim = evictable::<F>(&active, restricted)
                    .filter(|&index| active[index].end > interval.end);

                match victim {
                    Some(index) => {
                        let evicted = active.remove(index);
                        spill_slots += 1;
                        assignments.insert(evicted.vreg, Assignment::Spill(spill_slots));

                        // The register was already marked as used when it was
                        // first handed out, so only the mapping changes here.
                        assignments.insert(
                            interval.vreg.clone(),
                            Assignment::Register(registers[evicted.register]),
                        );
                        insert_active(
                            &mut active,
                            Active {
                                register: evicted.register,
                                end: interval.end,
                                vreg: interval.vreg.clone(),
                            },
                        );
                    }
                    None => {
                        spill_slots += 1;
                        assignments.insert(interval.vreg.clone(), Assignment::Spill(spill_slots));
                    }
                }
            }
        }
    }

    let callee_saved = registers
        .iter()
        .enumerate()
        .filter(|(index, _)| used_callee_saved[*index])
        .map(|(_, register)| *register)
        .collect();

    Allocation {
        assignments,
        spill_slots,
        callee_saved,
    }
}

/// The most preferred free register, honouring a callee-saved restriction.
///
/// # Returns
///
/// An index into [`RegisterFile::allocatable`], or `None` when every register
/// the interval may use is taken.
fn pick_register<F: RegisterFile>(available: &[bool], callee_saved_only: bool) -> Option<usize> {
    F::allocatable()
        .iter()
        .enumerate()
        .position(|(index, register)| {
            available[index] && (!callee_saved_only || F::is_callee_saved(*register))
        })
}

/// The active interval that ends latest among those whose register the
/// current interval is allowed to take.
///
/// `active` is sorted by end point, so the last candidate is the answer.
fn evictable<F: RegisterFile>(active: &[Active], callee_saved_only: bool) -> Option<usize> {
    let registers = F::allocatable();
    active
        .iter()
        .rposition(|held| !callee_saved_only || F::is_callee_saved(registers[held.register]))
}

/// Return the registers of every active interval that ends before
/// `current_start` to the free pool.
fn expire_before(active: &mut Vec<Active>, available: &mut [bool], current_start: usize) {
    // `active` is sorted by end point, so the expired intervals form a
    // prefix and can be drained in one pass.
    let expired = active.partition_point(|held| held.end < current_start);
    for held in active.drain(..expired) {
        available[held.register] = true;
    }
}

/// Insert `held` into `active`, keeping it sorted by end point.
fn insert_active(active: &mut Vec<Active>, held: Active) {
    let position = active.partition_point(|other| other.end <= held.end);
    active.insert(position, held);
}

/// Returns `true` if `interval` is live across any call.
///
/// `call_sites` is sorted by position, so a binary search skips straight to
/// the first call that could overlap the interval instead of scanning them all.
fn crosses_any_call(interval: &LiveInterval, call_sites: &[CallSite]) -> bool {
    let first_candidate = call_sites.partition_point(|call| call.position < interval.start);
    call_sites[first_candidate..]
        .iter()
        .take_while(|call| call.position <= interval.end)
        .any(|call| interval.crosses_call(call))
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::backend::x86::abi::SystemV;
    use crate::backend::x86::isa::X86Register;

    /// The allocator is generic, but exercising it through a real ABI keeps
    /// the tests honest about preference order and callee-saved sets.
    type Regs = SystemV;

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

    fn location_of(alloc: &Allocation<X86Register>, name: &str) -> Assignment<X86Register> {
        alloc.assignment_of(&VirtualReg::Temp(name.to_string()))
    }

    #[test]
    fn values_not_spanning_a_call_prefer_caller_saved_registers() {
        // Arrange: one short-lived value, one call it ends before.
        let intervals = [interval("short", 0, 2)];

        // Act
        let alloc = linear_scan::<Regs>(&intervals, &[call_at(5)]);

        // Assert: no callee-save overhead was taken on.
        assert_eq!(
            location_of(&alloc, "short"),
            Assignment::Register(X86Register::Rcx)
        );
        assert!(alloc.callee_saved().is_empty());
    }

    #[test]
    fn values_spanning_a_call_get_a_callee_saved_register() {
        // Arrange: the value is live on both sides of the call at index 3.
        let intervals = [interval("across", 0, 6)];

        // Act
        let alloc = linear_scan::<Regs>(&intervals, &[call_at(3)]);

        // Assert
        let Assignment::Register(register) = location_of(&alloc, "across") else {
            panic!("expected a register allocation");
        };
        assert!(
            SystemV::is_callee_saved(register),
            "{:?} is caller-saved and would be destroyed by the call",
            register
        );
        assert_eq!(alloc.callee_saved(), [register]);
    }

    #[test]
    fn a_value_produced_by_a_call_is_not_forced_to_callee_saved() {
        // Arrange: the interval starts *at* the call that defines it, so it
        // only comes into existence once the callee has returned.
        let intervals = [interval("returned", 4, 9)];
        let call = CallSite {
            position: 4,
            defines: Some(VirtualReg::Temp("returned".to_string())),
        };

        // Act
        let alloc = linear_scan::<Regs>(&intervals, &[call]);

        // Assert
        assert_eq!(
            location_of(&alloc, "returned"),
            Assignment::Register(X86Register::Rcx)
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
        let alloc = linear_scan::<Regs>(&intervals, &[call_at(10)]);

        // Assert: every one is either callee-saved or on the frame.
        for index in 0..8 {
            match location_of(&alloc, &format!("v{}", index)) {
                Assignment::Register(register) => assert!(
                    SystemV::is_callee_saved(register),
                    "v{} landed in caller-saved {:?}",
                    index,
                    register
                ),
                Assignment::Spill(_) => {}
            }
        }
        assert_eq!(
            alloc.spill_slots(),
            3,
            "8 values - 5 callee-saved registers"
        );
    }

    #[test]
    fn an_expired_register_is_reused_in_preference_order() {
        // Arrange: two consecutive short intervals; the first has died by the
        // time the second starts.
        let intervals = [interval("first", 0, 1), interval("second", 3, 4)];

        // Act
        let alloc = linear_scan::<Regs>(&intervals, &[]);

        // Assert: the freed register goes back to the front of the pool
        // instead of to the end of it.
        assert_eq!(
            location_of(&alloc, "second"),
            Assignment::Register(X86Register::Rcx)
        );
        assert_eq!(alloc.spill_slots(), 0);
    }

    #[test]
    fn the_longest_lived_value_is_the_one_that_gets_spilled() {
        // Arrange: fill the register file with long intervals, then add a
        // short one that has to displace a longer one.
        let mut intervals: Vec<LiveInterval> = (0..SystemV::allocatable().len())
            .map(|index| interval(&format!("long{}", index), 0, 100))
            .collect();
        intervals.push(interval("short", 1, 2));

        // Act
        let alloc = linear_scan::<Regs>(&intervals, &[]);

        // Assert: the short interval keeps a register, a long one is spilled.
        assert!(matches!(
            location_of(&alloc, "short"),
            Assignment::Register(_)
        ));
        assert_eq!(alloc.spill_slots(), 1);
    }

    #[test]
    fn intervals_ending_at_a_call_do_not_span_it() {
        // An argument's last use is the instruction before the call, so it
        // may stay in a caller-saved register.
        assert!(!interval("arg", 0, 4).crosses_call(&call_at(5)));
        assert!(interval("arg", 0, 5).crosses_call(&call_at(5)));
    }
}
