//! Stack-frame layout.
//!
//! Three kinds of value need space in a function's frame: spilled virtual
//! registers, local arrays, and variables whose address is taken.  All three
//! are laid out here, by the same slot arithmetic, so that the offset of
//! anything in the frame is computed in exactly one place.
//!
//! Nothing in this module is target-specific beyond the two numbers in
//! [`FrameParams`], so a new target reuses it by naming its word size and
//! stack alignment.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::hash::Hash;

use crate::backend::regalloc::{Allocation, Assignment};
use crate::backend::vreg::operand_to_vreg;
use crate::middle::ir::Operand;

/// The machine parameters the frame layout depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameParams {
    /// Size of one stack slot in bytes, normally the machine word.
    pub word_size: i32,
    /// Stack-pointer alignment the ABI requires at a call, in bytes.
    pub stack_alignment: i32,
}

/// The run-time home of a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Home<R> {
    /// Held in a physical register.
    Register(R),
    /// Held in the frame, `offset` bytes from the frame pointer.  Offsets are
    /// negative for anything below it, which is everything a callee owns.
    Frame(i32),
}

/// Where everything in one function lives.
///
/// The frame grows downward from the frame pointer:
///
/// ```text
///   [fp]                          saved frame pointer
///   [fp - 1*word .. ]             saved callee-saved registers
///   [ .. ]                        spill slots       (slot 1, 2, ...)
///   [ .. ]                        local arrays
///   [ .. ]                        address-taken variables
/// ```
///
/// Slots are numbered from 1 starting just below the saved registers, so slot
/// `n` sits at `fp - saved_bytes - n * word_size`.
#[derive(Debug, Clone)]
pub struct FrameLayout<R> {
    allocation: Allocation<R>,
    params: FrameParams,
    /// Bytes occupied by the saved callee-saved registers.
    saved_bytes: i32,
    /// Total slots reserved below the saved registers.
    slots: usize,
    /// Array variable id to the frame offset of its element 0.
    arrays: HashMap<usize, i32>,
    /// Address-taken variable id to its pinned frame offset.
    addr_taken: HashMap<usize, i32>,
}

impl<R: Copy + Eq + Hash> FrameLayout<R> {
    /// Lay out a frame for an allocated function.
    ///
    /// # Arguments
    ///
    /// * `params` - the target's word size and stack alignment
    /// * `allocation` - the register allocation, whose spill slots come first
    /// * `arrays` - bytes of storage per local array variable
    /// * `addr_taken` - variables that `AddrOf` takes the address of
    ///
    /// Both collections are ordered, so a given function always produces the
    /// same layout and therefore the same assembly.
    pub fn plan(
        params: FrameParams,
        allocation: Allocation<R>,
        arrays: &BTreeMap<usize, usize>,
        addr_taken: &BTreeSet<usize>,
    ) -> Self {
        let saved_bytes = allocation.callee_saved().len() as i32 * params.word_size;
        let mut slots = allocation.spill_slots();

        // `offset_of` needs the same fields, so build the layout empty and
        // fill it in through its own accessor.
        let mut layout = Self {
            allocation,
            params,
            saved_bytes,
            slots,
            arrays: HashMap::with_capacity(arrays.len()),
            addr_taken: HashMap::with_capacity(addr_taken.len()),
        };

        // An array occupies as many whole slots as its elements need: they sit
        // packed at their own size -- four bytes for `int`, eight for `long
        // int` -- and the array as a whole is rounded up to a slot so that
        // everything after it stays word-aligned.  Element 0 goes at the
        // lowest address, i.e. in the last slot reserved, so that element `i`
        // is at `element0 + i * size` and indexing can be folded into a single
        // scaled-index memory operand.
        for (&variable, &bytes) in arrays {
            slots += bytes.div_ceil(params.word_size as usize);
            let element_zero = layout.offset_of(slots);
            layout.arrays.insert(variable, element_zero);
        }

        for &variable in addr_taken {
            slots += 1;
            let offset = layout.offset_of(slots);
            layout.addr_taken.insert(variable, offset);
        }

        layout.slots = slots;
        layout
    }

    /// The callee-saved registers the prologue must push.
    pub fn callee_saved(&self) -> &[R] {
        self.allocation.callee_saved()
    }

    /// Bytes the saved callee-saved registers occupy below the frame pointer.
    pub fn saved_bytes(&self) -> i32 {
        self.saved_bytes
    }

    /// Bytes the prologue must subtract from the stack pointer after pushing
    /// the callee-saved registers.
    ///
    /// The frame pointer is aligned on entry, so padding the whole frame to
    /// the ABI's alignment leaves the stack pointer aligned for every call
    /// this function makes.
    pub fn stack_adjust(&self) -> i32 {
        let occupied = self.saved_bytes + self.slots as i32 * self.params.word_size;
        let aligned = align_up(occupied, self.params.stack_alignment);
        aligned - self.saved_bytes
    }

    /// Where `operand` can be read from.
    ///
    /// An address-taken variable is pinned to its own slot: `AddrOf` hands
    /// that address out, so a write through the resulting pointer has to be
    /// visible when the variable is next read by name.
    ///
    /// # Panics
    ///
    /// Panics unless `operand` is a variable or a temporary; immediates and
    /// labels have no home.
    pub fn home_of(&self, operand: &Operand) -> Home<R> {
        if let Some(offset) = self.pinned_offset(operand) {
            return Home::Frame(offset);
        }
        let vreg = operand_to_vreg(operand)
            .expect("Compiler Bug: only variables and temporaries have a home");
        match self.allocation.assignment_of(&vreg) {
            Assignment::Register(register) => Home::Register(register),
            Assignment::Spill(slot) => Home::Frame(self.offset_of(slot)),
        }
    }

    /// The register a definition of `operand` should be produced in, or `None`
    /// when the value only lives in the frame.
    pub fn register_of(&self, operand: &Operand) -> Option<R> {
        let vreg = operand_to_vreg(operand)
            .expect("Compiler Bug: only variables and temporaries have a home");
        self.allocation.register_of(&vreg)
    }

    /// The frame offset that must be refreshed when `operand` is written, if
    /// any.
    ///
    /// A pinned variable is read from its own slot, so that is the copy to
    /// keep current; otherwise a spilled value has to be written back to its
    /// spill slot.
    pub fn write_back_offset(&self, operand: &Operand) -> Option<i32> {
        if let Some(offset) = self.pinned_offset(operand) {
            return Some(offset);
        }
        let vreg = operand_to_vreg(operand)
            .expect("Compiler Bug: only variables and temporaries have a home");
        match self.allocation.assignment_of(&vreg) {
            Assignment::Spill(slot) => Some(self.offset_of(slot)),
            Assignment::Register(_) => None,
        }
    }

    /// The frame offset of element 0 of the array variable `operand` names.
    ///
    /// # Panics
    ///
    /// Panics if `operand` is not a variable with reserved array storage,
    /// which means the IR indexed something the frame planner never saw.
    pub fn array_base(&self, operand: &Operand) -> i32 {
        let Operand::Var(id) = operand else {
            panic!(
                "Compiler Bug: an array base must be a variable, got {:?}",
                operand
            );
        };
        *self
            .arrays
            .get(id)
            .expect("Compiler Bug: array variable has no reserved frame storage")
    }

    /// The pinned frame offset of an address-taken variable.
    ///
    /// # Panics
    ///
    /// Panics if the variable's address was never seen to be taken, which
    /// means `AddrOf` reached lowering without the frame being planned for it.
    pub fn pinned(&self, operand: &Operand) -> i32 {
        self.pinned_offset(operand)
            .expect("Compiler Bug: AddrOf on a variable with no pinned frame slot")
    }

    /// The pinned offset of `operand`, if it is an address-taken variable.
    fn pinned_offset(&self, operand: &Operand) -> Option<i32> {
        match operand {
            Operand::Var(id) => self.addr_taken.get(id).copied(),
            _ => None,
        }
    }

    /// The frame offset of slot `slot`, counted from 1.
    fn offset_of(&self, slot: usize) -> i32 {
        -(self.saved_bytes + slot as i32 * self.params.word_size)
    }
}

/// Round `value` up to the next multiple of `alignment`.
fn align_up(value: i32, alignment: i32) -> i32 {
    debug_assert!(
        alignment > 0 && alignment & (alignment - 1) == 0,
        "alignment must be a positive power of two"
    );
    // The classic power-of-two trick: adding `alignment - 1` carries into the
    // next multiple, and the mask clears whatever is below it.
    (value + alignment - 1) & !(alignment - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::backend::liveness::LiveInterval;
    use crate::backend::regalloc::{RegisterFile, linear_scan};
    use crate::backend::vreg::VirtualReg;
    use crate::backend::x86::abi::{self, SystemV};
    use crate::backend::x86::isa::X86Register;

    const PARAMS: FrameParams = FrameParams {
        word_size: 8,
        stack_alignment: 16,
    };

    /// A layout with `spilled` values forced onto the frame, plus whatever
    /// arrays and pinned variables the caller asks for.
    fn layout(
        spilled: usize,
        arrays: &BTreeMap<usize, usize>,
        addr_taken: &BTreeSet<usize>,
    ) -> FrameLayout<X86Register> {
        // Long overlapping intervals exhaust the register file, so the excess
        // is spilled -- the only way to get spill slots without hand-building
        // an `Allocation`.
        let count = SystemV::allocatable().len() + spilled;
        let intervals: Vec<LiveInterval> = (0..count)
            .map(|index| LiveInterval {
                vreg: VirtualReg::Temp(format!("t{}", index)),
                start: 0,
                end: 100,
            })
            .collect();
        let allocation = linear_scan::<SystemV>(&intervals, &[]);
        assert_eq!(allocation.spill_slots(), spilled);

        FrameLayout::plan(PARAMS, allocation, arrays, addr_taken)
    }

    #[test]
    fn the_first_spill_slot_sits_just_below_the_saved_registers() {
        // Arrange / Act
        let layout = layout(1, &BTreeMap::new(), &BTreeSet::new());

        // Assert: slot 1 is one word past the last saved register.
        assert_eq!(layout.offset_of(1), -(layout.saved_bytes() + 8));
    }

    #[test]
    fn array_elements_run_upward_from_the_lowest_slot() {
        // Arrange: `long int a[3]`, twenty-four bytes, and no spills.
        let arrays = BTreeMap::from([(7, 3 * 8)]);

        // Act
        let layout = layout(0, &arrays, &BTreeSet::new());

        // Assert: element 0 is in the last of the three slots, so element `i`
        // is `i * 8` bytes above it and the scale can be positive.
        assert_eq!(
            layout.array_base(&Operand::Var(7)),
            -(layout.saved_bytes() + 3 * 8)
        );
    }

    #[test]
    fn an_array_of_ints_packs_two_elements_into_each_slot() {
        // Arrange: `int a[3]`, twelve bytes, which is two whole slots.
        let arrays = BTreeMap::from([(7, 3 * 4)]);

        // Act
        let layout = layout(0, &arrays, &BTreeSet::new());

        // Assert: the array is rounded up to a slot boundary, so whatever is
        // laid out after it stays word-aligned.
        assert_eq!(
            layout.array_base(&Operand::Var(7)),
            -(layout.saved_bytes() + 2 * 8)
        );
    }

    #[test]
    fn every_kind_of_frame_value_gets_its_own_space() {
        // Arrange: one spill, one array of two words, one pinned variable.
        let arrays = BTreeMap::from([(1, 2 * 8)]);
        let pinned = BTreeSet::from([2]);

        // Act
        let layout = layout(1, &arrays, &pinned);

        // Assert: spill slot 1, array slots 2-3, pinned slot 4 -- nothing
        // overlaps and nothing is left over.
        let saved = layout.saved_bytes();
        assert_eq!(layout.offset_of(1), -(saved + 8));
        assert_eq!(layout.array_base(&Operand::Var(1)), -(saved + 24));
        assert_eq!(layout.pinned(&Operand::Var(2)), -(saved + 32));
        assert_eq!(layout.stack_adjust(), align_up(saved + 32, 16) - saved);
    }

    #[test]
    fn a_pinned_variable_is_read_from_its_slot_even_when_it_has_a_register() {
        // Arrange: variable 3 is address-taken, so pointer writes must be
        // visible when it is read by name.
        let mut intervals = vec![LiveInterval {
            vreg: VirtualReg::Var(3),
            start: 0,
            end: 4,
        }];
        intervals.push(LiveInterval {
            vreg: VirtualReg::Temp("t1".to_string()),
            start: 1,
            end: 4,
        });
        let allocation = linear_scan::<SystemV>(&intervals, &[]);
        let layout = FrameLayout::plan(PARAMS, allocation, &BTreeMap::new(), &BTreeSet::from([3]));

        // Act / Assert: reads go to the frame, definitions still go to the
        // register and are written back to the same slot.
        let pinned = layout.pinned(&Operand::Var(3));
        assert_eq!(layout.home_of(&Operand::Var(3)), Home::Frame(pinned));
        assert!(layout.register_of(&Operand::Var(3)).is_some());
        assert_eq!(layout.write_back_offset(&Operand::Var(3)), Some(pinned));

        // A value with a register and no pinned slot needs no write-back.
        assert_eq!(
            layout.write_back_offset(&Operand::Temp("t1".to_string())),
            None
        );
    }

    #[test]
    fn the_frame_is_padded_to_the_abi_alignment() {
        // Arrange / Act
        let layout = layout(1, &BTreeMap::new(), &BTreeSet::new());

        // Assert: the whole frame is a multiple of the alignment, and it is
        // still large enough for the slot it reserved.
        let frame_bytes = layout.saved_bytes() + layout.stack_adjust();
        assert_eq!(frame_bytes % PARAMS.stack_alignment, 0);
        assert!(layout.stack_adjust() >= PARAMS.word_size);
    }

    #[test]
    fn an_odd_number_of_saved_registers_is_padded_even_with_no_slots() {
        // Arrange: a value live across a call takes a callee-saved register,
        // whose push leaves the stack pointer 8 bytes out of alignment.
        let intervals = [LiveInterval {
            vreg: VirtualReg::Temp("across".to_string()),
            start: 0,
            end: 4,
        }];
        let allocation = linear_scan::<SystemV>(
            &intervals,
            &[crate::backend::liveness::CallSite {
                position: 2,
                defines: None,
            }],
        );
        assert_eq!(allocation.callee_saved().len(), 1);

        // Act
        let layout = FrameLayout::plan(PARAMS, allocation, &BTreeMap::new(), &BTreeSet::new());

        // Assert: 8 bytes of padding restore 16-byte alignment.
        assert_eq!(layout.stack_adjust(), 8);
        assert_eq!(abi::STACK_ALIGNMENT, PARAMS.stack_alignment);
    }
}
