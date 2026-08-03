//! Instruction selection for x86-64.
//!
//! Turns a function's linearized TAC into [`X86Instruction`]s, asking the
//! [`FrameLayout`] where every value lives.  Three rules shape almost all of
//! the code here:
//!
//! - x86 arithmetic is two-operand and destructive, so a result is built in a
//!   register and written back to the frame afterwards -- see [`FunctionLowering::define`].
//! - At most one operand of an instruction may be memory, so anything spilled
//!   goes through a scratch register -- see [`FunctionLowering::in_register`].
//! - Values that must move between fixed ABI registers form a simultaneous
//!   assignment, not a sequence of `mov`s -- see
//!   [`FunctionLowering::emit_parallel_moves`].

use std::collections::HashSet;

use crate::backend::frame::{FrameLayout, Home};
use crate::backend::linear::LinearizedCfg;
use crate::backend::x86::abi::{
    self, ARGUMENT_REGISTERS, FRAME_POINTER, RETURN_VALUE, SCRATCH, SCRATCH2, STACK_POINTER,
    WORD_SIZE,
};
use crate::backend::x86::isa::{
    ConditionCode, X86Function, X86Instruction, X86Operand, X86Register,
};
use crate::middle::ir::{Opcode, Operand, TACInstruction};

/// Builds the x86 instruction sequence for one function.
pub struct FunctionLowering<'a> {
    /// Instructions emitted so far, in order.
    out: Vec<X86Instruction>,
    /// Where every value of this function lives.
    layout: &'a FrameLayout<X86Register>,
    /// The label every `Ret` jumps to, so the frame is torn down in one place.
    epilogue: String,
    /// `Param` operands buffered until the `Call` that consumes them.
    pending_arguments: Vec<Operand>,
}

impl<'a> FunctionLowering<'a> {
    /// Lower one function into x86-64.
    ///
    /// # Arguments
    ///
    /// * `name` - the function's symbol name
    /// * `body` - its instructions in linear order, grouped into blocks
    /// * `layout` - where every value of the function lives
    pub fn lower(
        name: &str,
        body: &LinearizedCfg,
        layout: &'a FrameLayout<X86Register>,
    ) -> X86Function {
        let mut lowering = Self {
            // Most TAC instructions expand to one or two x86 instructions.
            out: Vec::with_capacity(body.instructions().len() * 2),
            layout,
            epilogue: format!(".{}_epilogue", name),
            pending_arguments: Vec::new(),
        };

        lowering.emit_prologue();
        for block in body.blocks() {
            lowering.emit(X86Instruction::Label(block_label(&block.label)));
            lowering.lower_block(body.body(block));
        }
        lowering.emit_epilogue();

        X86Function {
            name: name.to_string(),
            instructions: lowering.out,
        }
    }

    /// Lower the instructions of one basic block.
    fn lower_block(&mut self, block: &[TACInstruction]) {
        let mut position = 0;
        while position < block.len() {
            // A run of `GetParam`s has to be lowered as one group: each reads
            // an incoming argument register that a later one's destination
            // may overwrite.  See `lower_incoming_arguments`.
            if block[position].opcode == Opcode::GetParam {
                let run = block[position..]
                    .iter()
                    .take_while(|instr| instr.opcode == Opcode::GetParam)
                    .count();
                self.lower_incoming_arguments(&block[position..position + run]);
                position += run;
                continue;
            }

            self.lower_instruction(&block[position]);
            position += 1;
        }
    }

    fn lower_instruction(&mut self, instr: &TACInstruction) {
        match instr.opcode {
            // The x86 mnemonic is passed as the enum's own constructor, so
            // the three two-operand cases share one lowering.
            Opcode::Add => self.lower_binary(instr, X86Instruction::Add),
            Opcode::Sub => self.lower_binary(instr, X86Instruction::Sub),
            Opcode::Mul => self.lower_binary(instr, X86Instruction::Imul),
            Opcode::Div => self.lower_division(instr),

            Opcode::Eq => self.lower_comparison(instr, ConditionCode::E),
            Opcode::Neq => self.lower_comparison(instr, ConditionCode::Ne),
            Opcode::Lt => self.lower_comparison(instr, ConditionCode::L),
            Opcode::Lte => self.lower_comparison(instr, ConditionCode::Le),
            Opcode::Gt => self.lower_comparison(instr, ConditionCode::G),
            Opcode::Gte => self.lower_comparison(instr, ConditionCode::Ge),

            Opcode::Mov => self.lower_move(instr),
            Opcode::Jump => {
                let target = branch_target(instr.arg1.as_ref());
                self.emit(X86Instruction::Jmp(target));
            }
            Opcode::BranchIf => self.lower_branch(instr, ConditionCode::Ne),
            Opcode::BranchIfNot => self.lower_branch(instr, ConditionCode::E),

            // The argument is buffered until the `Call` that consumes it.
            Opcode::Param => {
                let argument = operand(instr, &instr.arg1).clone();
                self.pending_arguments.push(argument);
            }
            Opcode::Call => self.lower_call(instr),
            Opcode::Ret => self.lower_return(instr),
            // A lone `GetParam` is a group of one; runs are batched by
            // `lower_block` before reaching this dispatcher.
            Opcode::GetParam => self.lower_incoming_arguments(std::slice::from_ref(instr)),

            Opcode::ArrayStore => self.lower_array_store(instr),
            Opcode::ArrayLoad => self.lower_array_load(instr),

            Opcode::Load => self.lower_load(instr),
            Opcode::Store => self.lower_store(instr),
            Opcode::AddrOf => self.lower_address_of(instr),
        }
    }

    // ### Frame setup and teardown ###

    /// Establish the frame: save the caller's frame pointer, take the callee-
    /// saved registers this function uses, and reserve its frame slots.
    fn emit_prologue(&mut self) {
        // Copy the shared reference out so the emit calls below can borrow
        // `self` mutably; `layout` outlives this function either way.
        let layout = self.layout;

        self.emit(X86Instruction::Push(reg(FRAME_POINTER)));
        self.emit(X86Instruction::Mov(reg(FRAME_POINTER), reg(STACK_POINTER)));

        for &register in layout.callee_saved() {
            self.emit(X86Instruction::Push(reg(register)));
        }

        let reserved = layout.stack_adjust();
        if reserved > 0 {
            self.emit(X86Instruction::Sub(
                reg(STACK_POINTER),
                X86Operand::Imm(reserved.into()),
            ));
        }
    }

    /// Undo the prologue and return.  Every `Ret` jumps here.
    fn emit_epilogue(&mut self) {
        let layout = self.layout;
        let epilogue = self.epilogue.clone();
        self.emit(X86Instruction::Label(epilogue));

        if layout.callee_saved().is_empty() {
            // Nothing was pushed below the frame pointer, so it alone says
            // where the caller's stack pointer was.
            self.emit(X86Instruction::Mov(reg(STACK_POINTER), reg(FRAME_POINTER)));
        } else {
            // Point the stack pointer at the topmost saved register, whatever
            // the body left it at, and pop them in reverse order.
            self.emit(X86Instruction::Lea(
                reg(STACK_POINTER),
                frame(-layout.saved_bytes()),
            ));
            for &register in layout.callee_saved().iter().rev() {
                self.emit(X86Instruction::Pop(reg(register)));
            }
        }

        self.emit(X86Instruction::Pop(reg(FRAME_POINTER)));
        self.emit(X86Instruction::Ret);
    }

    // ### Arithmetic and comparison ###

    /// Lower `dest = lhs <op> rhs` for an x86 two-operand instruction.
    ///
    /// `build` is the instruction's enum constructor, e.g.
    /// [`X86Instruction::Add`].
    fn lower_binary(
        &mut self,
        instr: &TACInstruction,
        build: fn(X86Operand, X86Operand) -> X86Instruction,
    ) {
        let (dest, lhs, rhs) = three_operands(instr);

        self.define(dest, |lowering, result| {
            // The instruction overwrites its left operand, so the left-hand
            // side is loaded into the result register first.
            lowering.move_into(lhs, result);
            let right = lowering.in_place(rhs);
            let right = lowering.encodable(right, SCRATCH2);
            lowering.emit(build(reg(result), right));
        });
    }

    /// Lower `dest = lhs / rhs`.
    ///
    /// `idiv` is fixed to RDX:RAX, so the dividend is moved into RAX and
    /// sign-extended before the divide, and the quotient copied out of RAX
    /// afterwards.
    fn lower_division(&mut self, instr: &TACInstruction) {
        let (dest, lhs, rhs) = three_operands(instr);

        self.move_into(lhs, RETURN_VALUE);
        self.emit(X86Instruction::Cqo);

        // `idiv` divides by a register or memory operand, never an immediate.
        let divisor = match self.in_place(rhs) {
            immediate @ X86Operand::Imm(_) => reg(self.ensure_register(immediate, SCRATCH2)),
            other => other,
        };
        self.emit(X86Instruction::Idiv(divisor));

        self.define(dest, |lowering, result| lowering.copy(RETURN_VALUE, result));
    }

    /// Lower `dest = lhs <cmp> rhs` into a 0 or 1 in `dest`.
    fn lower_comparison(&mut self, instr: &TACInstruction, condition: ConditionCode) {
        let (dest, lhs, rhs) = three_operands(instr);

        // Both operands cannot be memory, so the left one is loaded first.
        let left = self.in_register(lhs, SCRATCH);
        let right = self.in_place(rhs);
        let right = self.encodable(right, SCRATCH2);
        self.emit(X86Instruction::Cmp(reg(left), right));

        self.define(dest, |lowering, result| {
            // `setcc` writes one byte and leaves the rest of the register
            // untouched, so `movzx` from that byte supplies the zeroes.
            lowering.emit(X86Instruction::SetCC(condition, RETURN_VALUE));
            lowering.emit(X86Instruction::Movzx(result, RETURN_VALUE));
        });
    }

    // ### Data movement and control flow ###

    fn lower_move(&mut self, instr: &TACInstruction) {
        let (dest, source) = two_operands(instr);

        // A destination with no register of its own is written straight to the
        // frame; routing it through one would cost an instruction per move.
        if self.layout.register_of(dest).is_none()
            && let Some(offset) = self.layout.write_back_offset(dest)
        {
            let value = self.storable(source, SCRATCH);
            self.emit(X86Instruction::Mov(frame(offset), value));
            return;
        }

        self.define(dest, |lowering, result| lowering.move_into(source, result));
    }

    /// Lower a conditional branch: jump when the condition value is non-zero
    /// (`Ne`) or zero (`E`).
    fn lower_branch(&mut self, instr: &TACInstruction, taken_when: ConditionCode) {
        let condition = operand(instr, &instr.arg1);
        let target = branch_target(instr.arg2.as_ref());

        // `test x, x` sets ZF exactly when x is zero, and needs no immediate.
        let value = self.in_register(condition, SCRATCH);
        self.emit(X86Instruction::Test(reg(value), reg(value)));
        self.emit(X86Instruction::Jcc(taken_when, target));
    }

    fn lower_return(&mut self, instr: &TACInstruction) {
        if let Some(value) = &instr.arg1 {
            self.move_into(value, RETURN_VALUE);
        }
        // The frame is torn down once, in the epilogue.
        self.emit(X86Instruction::Jmp(self.epilogue.clone()));
    }

    /// Lower a call: place the arguments, call, then take the result.
    fn lower_call(&mut self, instr: &TACInstruction) {
        let callee = match &instr.arg1 {
            Some(Operand::Label(name)) => name.clone(),
            other => panic!("Compiler Bug: a call needs a callee label, got {:?}", other),
        };
        let arguments: Vec<Operand> = self.pending_arguments.drain(..).collect();
        let on_stack = arguments
            .get(ARGUMENT_REGISTERS.len()..)
            .unwrap_or_default();

        // The stack pointer is aligned throughout the body and `call` pushes
        // one word, so an odd number of pushed arguments needs one word of
        // padding.  It has to be reserved *below* them: between them and the
        // return address the callee would read it as the next argument.
        let padding = (on_stack.len() % 2) as i32 * WORD_SIZE;
        if padding > 0 {
            self.emit(X86Instruction::Sub(
                reg(STACK_POINTER),
                X86Operand::Imm(padding.into()),
            ));
        }

        // Pushed right to left, so the first stack argument ends up closest
        // to the return address, where the callee looks for it.
        for argument in on_stack.iter().rev() {
            let value = self.in_register(argument, SCRATCH);
            self.emit(X86Instruction::Push(reg(value)));
        }

        // The register arguments are a simultaneous assignment: one of them
        // may already be sitting in another's destination register.
        let moves: Vec<(X86Register, X86Operand)> = arguments
            .iter()
            .zip(ARGUMENT_REGISTERS)
            .map(|(argument, &destination)| (destination, self.in_place(argument)))
            .collect();
        self.emit_parallel_moves(&moves);

        self.emit(X86Instruction::Call(callee));

        let pushed = padding + on_stack.len() as i32 * WORD_SIZE;
        if pushed > 0 {
            self.emit(X86Instruction::Add(
                reg(STACK_POINTER),
                X86Operand::Imm(pushed.into()),
            ));
        }

        if let Some(dest) = &instr.dest {
            self.define(dest, |lowering, result| lowering.copy(RETURN_VALUE, result));
        }
    }

    /// Lower a run of consecutive `GetParam` instructions.
    ///
    /// Incoming arguments arrive in fixed ABI locations while the allocator is
    /// free to place the corresponding locals anywhere.  Emitting one `mov`
    /// per parameter in isolation is therefore wrong: a local assigned to,
    /// say, RCX would clobber the incoming fourth argument before the
    /// parameter that needs it has been read.  The whole run is one
    /// simultaneous assignment, lowered in two phases:
    ///
    /// 1. Parameters that live in the frame are written first, while every
    ///    incoming argument register still holds the caller's value.
    /// 2. Parameters that live in registers are shuffled with
    ///    [`Self::emit_parallel_moves`].
    fn lower_incoming_arguments(&mut self, group: &[TACInstruction]) {
        let mut moves: Vec<(X86Register, X86Operand)> = Vec::with_capacity(group.len());

        for instr in group {
            let (dest, index) = match (&instr.dest, &instr.arg1) {
                (Some(dest), Some(Operand::ImmInt(index))) => (dest, *index as usize),
                _ => panic!("Compiler Bug: GetParam needs a destination and an index"),
            };
            let source = abi::incoming_argument(index);

            // Phase 1: frame destinations.
            if let Some(offset) = self.layout.write_back_offset(dest) {
                // x86 has no memory-to-memory `mov`, so a stack-passed
                // argument goes through a scratch register.
                let value = self.ensure_register(source.clone(), SCRATCH);
                self.emit(X86Instruction::Mov(frame(offset), reg(value)));
            }

            // Phase 2 is deferred until the whole run has been seen.
            if let Some(register) = self.layout.register_of(dest) {
                moves.push((register, source));
            }
        }

        keep_last_write_per_register(&mut moves);
        self.emit_parallel_moves(&moves);
    }

    // ### Arrays and pointers ###

    /// Lower `base[index] = value`.
    fn lower_array_store(&mut self, instr: &TACInstruction) {
        // `dest` names the array being written into, not a definition.
        let (base, index, value) = three_operands(instr);

        let element = self.element(base, index);
        // The element is the instruction's one memory operand, so the value
        // has to be a register or an immediate.  `element` may be holding the
        // index in SCRATCH2, so the other scratch register is used here.
        let value = self.storable(value, SCRATCH);
        self.emit(X86Instruction::Mov(element, value));
    }

    /// Lower `dest = base[index]`.
    fn lower_array_load(&mut self, instr: &TACInstruction) {
        let (dest, base, index) = three_operands(instr);

        let element = self.element(base, index);
        self.define(dest, |lowering, result| {
            lowering.emit(X86Instruction::Mov(reg(result), element));
        });
    }

    /// The memory operand addressing `base[index]`.
    ///
    /// A constant index folds into the displacement and a computed one becomes
    /// a scaled index, so an element access costs a single instruction either
    /// way.  Element 0 is at the lowest address of the array's storage, which
    /// is what lets the scale be positive.
    fn element(&mut self, base: &Operand, index: &Operand) -> X86Operand {
        let element_zero = self.layout.array_base(base);

        match index {
            Operand::ImmInt(constant) => {
                let offset = constant
                    .checked_mul(i64::from(WORD_SIZE))
                    .and_then(|bytes| i32::try_from(bytes).ok())
                    .expect("Compiler Bug: array index is too large to address");
                frame(element_zero + offset)
            }
            computed => {
                let index_register = self.in_register(computed, SCRATCH2);
                X86Operand::indexed(FRAME_POINTER, index_register, WORD_SIZE as u8, element_zero)
            }
        }
    }

    /// Lower `dest = &variable`.
    ///
    /// The variable is pinned to a frame slot precisely so that it has an
    /// address to hand out.
    fn lower_address_of(&mut self, instr: &TACInstruction) {
        let (dest, variable) = two_operands(instr);
        let offset = self.layout.pinned(variable);

        self.define(dest, |lowering, result| {
            lowering.emit(X86Instruction::Lea(reg(result), frame(offset)));
        });
    }

    /// Lower `dest = *address`.
    fn lower_load(&mut self, instr: &TACInstruction) {
        let (dest, address) = two_operands(instr);

        let pointer = self.in_register(address, SCRATCH2);
        self.define(dest, |lowering, result| {
            lowering.emit(X86Instruction::Mov(
                reg(result),
                X86Operand::mem(pointer, 0),
            ));
        });
    }

    /// Lower `*address = value`.
    fn lower_store(&mut self, instr: &TACInstruction) {
        let address = operand(instr, &instr.arg1);
        let value = operand(instr, &instr.arg2);

        // The store is the instruction's one memory operand, so the pointer
        // must be in a register and the value a register or an immediate.
        let pointer = self.in_register(address, SCRATCH2);
        let value = self.storable(value, SCRATCH);
        self.emit(X86Instruction::Mov(X86Operand::mem(pointer, 0), value));
    }

    // ### Emission helpers ###

    fn emit(&mut self, instr: X86Instruction) {
        self.out.push(instr);
    }

    /// Produce a definition of `dest`.
    ///
    /// `produce` is handed the register the value must be built in -- `dest`'s
    /// own register, or a scratch register when it only lives in the frame --
    /// and the write back to its frame slot is emitted afterwards.  Every
    /// instruction that defines a value goes through here, so spilling and
    /// address-taken variables are handled in exactly one place.
    fn define(&mut self, dest: &Operand, produce: impl FnOnce(&mut Self, X86Register)) {
        let result = self.layout.register_of(dest).unwrap_or(SCRATCH);
        produce(self, result);

        if let Some(offset) = self.layout.write_back_offset(dest) {
            self.emit(X86Instruction::Mov(frame(offset), reg(result)));
        }
    }

    /// The operand that holds `value`, emitting nothing.
    ///
    /// Prefer [`Self::in_register`] unless a memory operand is genuinely
    /// acceptable: most x86 instructions allow at most one.
    fn in_place(&self, value: &Operand) -> X86Operand {
        match value {
            Operand::ImmInt(constant) => X86Operand::Imm(*constant),
            Operand::Label(label) => X86Operand::Label(label.clone()),
            Operand::Var(_) | Operand::Temp(_) => match self.layout.home_of(value) {
                Home::Register(register) => reg(register),
                Home::Frame(offset) => frame(offset),
            },
        }
    }

    /// Get `value` into a register, loading it into `scratch` if it is not in
    /// one already.
    fn in_register(&mut self, value: &Operand, scratch: X86Register) -> X86Register {
        let operand = self.in_place(value);
        self.ensure_register(operand, scratch)
    }

    /// Get an x86 operand into a register, moving it into `scratch` unless it
    /// is one already.
    fn ensure_register(&mut self, operand: X86Operand, scratch: X86Register) -> X86Register {
        match operand {
            X86Operand::Reg(register) => register,
            other => {
                self.emit(X86Instruction::Mov(reg(scratch), other));
                scratch
            }
        }
    }

    /// Move `value` into `register`, unless it is already there.
    fn move_into(&mut self, value: &Operand, register: X86Register) {
        let source = self.in_place(value);
        if source != reg(register) {
            self.emit(X86Instruction::Mov(reg(register), source));
        }
    }

    /// Copy `from` into `to`, unless they are the same register.
    fn copy(&mut self, from: X86Register, to: X86Register) {
        if from != to {
            self.emit(X86Instruction::Mov(reg(to), reg(from)));
        }
    }

    /// Make an operand usable as the source of an arithmetic instruction or a
    /// store.
    ///
    /// x86 encodes at most a 32-bit immediate, so a wider constant -- which
    /// constant folding can easily produce -- has to be materialized in
    /// `scratch` first.
    fn encodable(&mut self, operand: X86Operand, scratch: X86Register) -> X86Operand {
        match operand {
            X86Operand::Imm(constant) if i32::try_from(constant).is_err() => {
                reg(self.ensure_register(X86Operand::Imm(constant), scratch))
            }
            other => other,
        }
    }

    /// Get `value` into a form that can be stored to memory: a register, or an
    /// immediate small enough to encode.
    ///
    /// Only one of the two conversions can fire, so a single `scratch`
    /// register serves both.
    fn storable(&mut self, value: &Operand, scratch: X86Register) -> X86Operand {
        let operand = self.in_place(value);
        match operand {
            X86Operand::Imm(_) => self.encodable(operand, scratch),
            other => reg(self.ensure_register(other, scratch)),
        }
    }

    /// Emit `mov`s into registers that must take effect *as if* they all
    /// happened at once.
    ///
    /// Emitting them naively is wrong whenever one move's destination is
    /// another move's source, because the second would then read a value that
    /// has already been overwritten.  This is the classic parallel-move
    /// problem, and it shows up wherever values are shuffled between fixed ABI
    /// registers and allocated ones.
    ///
    /// A move is safe to emit once no other pending move reads its
    /// destination.  When no such move exists the remainder forms a cycle
    /// (e.g. `rdi <- rsi` together with `rsi <- rdi`), which is broken by
    /// stashing one register in [`SCRATCH`] and rewriting the reads of it.
    /// Only register sources can conflict; immediates and frame slots are
    /// never written here.
    ///
    /// Destinations must never be [`SCRATCH`]: every caller uses ABI argument
    /// registers or allocated ones, and neither pool contains it.
    fn emit_parallel_moves(&mut self, moves: &[(X86Register, X86Operand)]) {
        // Self-moves are dropped: they are no-ops, and keeping them would
        // make every such register look "still read" and stall the loop below.
        let mut pending: Vec<(X86Register, X86Operand)> = moves
            .iter()
            .filter(|(destination, source)| *source != reg(*destination))
            .cloned()
            .collect();

        while !pending.is_empty() {
            let ready = pending
                .iter()
                .position(|(destination, _)| !reads_register(&pending, *destination));

            let index = match ready {
                Some(index) => index,
                None => {
                    // Every remaining destination is still needed as a source,
                    // so break the cycle: keep one register's value in the
                    // scratch register and read it from there instead.
                    let stashed = pending[0].0;
                    self.emit(X86Instruction::Mov(reg(SCRATCH), reg(stashed)));
                    for (_, source) in pending.iter_mut() {
                        if *source == reg(stashed) {
                            *source = reg(SCRATCH);
                        }
                    }
                    // `stashed` is no longer read, so its move is now ready.
                    0
                }
            };

            let (destination, source) = pending.remove(index);
            self.emit(X86Instruction::Mov(reg(destination), source));
        }
    }
}

// ### Free helpers ###

/// A register as an operand.
const fn reg(register: X86Register) -> X86Operand {
    X86Operand::Reg(register)
}

/// The frame slot `offset` bytes from the frame pointer.
const fn frame(offset: i32) -> X86Operand {
    X86Operand::mem(FRAME_POINTER, offset)
}

/// The assembler label of an IR basic block.
///
/// The leading dot makes it a local symbol, so block labels of different
/// functions cannot collide with each other or with a function name.
fn block_label(label: &str) -> String {
    format!(".{}", label)
}

/// The block label a control-flow operand names.
///
/// # Panics
///
/// Panics unless the operand is a label, which means the IR is malformed.
fn branch_target(operand: Option<&Operand>) -> String {
    match operand {
        Some(Operand::Label(label)) => block_label(label),
        other => panic!("Compiler Bug: expected a branch target, got {:?}", other),
    }
}

/// The `dest`, `arg1` and `arg2` of an instruction that must have all three.
///
/// # Panics
///
/// Panics if any operand is absent, which means the IR is malformed.
fn three_operands(instr: &TACInstruction) -> (&Operand, &Operand, &Operand) {
    match (&instr.dest, &instr.arg1, &instr.arg2) {
        (Some(dest), Some(first), Some(second)) => (dest, first, second),
        _ => panic!("Compiler Bug: {:?} needs three operands", instr.opcode),
    }
}

/// The `dest` and `arg1` of an instruction that must have both.
///
/// # Panics
///
/// Panics if either operand is absent, which means the IR is malformed.
fn two_operands(instr: &TACInstruction) -> (&Operand, &Operand) {
    match (&instr.dest, &instr.arg1) {
        (Some(dest), Some(first)) => (dest, first),
        _ => panic!(
            "Compiler Bug: {:?} needs a destination and an operand",
            instr.opcode
        ),
    }
}

/// The contents of an operand field the instruction must have.
///
/// # Panics
///
/// Panics if the field is empty, which means the IR is malformed.
fn operand<'i>(instr: &TACInstruction, field: &'i Option<Operand>) -> &'i Operand {
    field
        .as_ref()
        .unwrap_or_else(|| panic!("Compiler Bug: {:?} is missing an operand", instr.opcode))
}

/// Returns `true` if any pending parallel move still reads `register`.
fn reads_register(pending: &[(X86Register, X86Operand)], register: X86Register) -> bool {
    pending.iter().any(|(_, source)| *source == reg(register))
}

/// Drop every move whose destination is written again later in the group,
/// keeping only the last write to each register.
///
/// Two parameters can be handed the same register: an unused parameter's live
/// interval collapses to its definition, so the allocator is free to reuse its
/// register for a later one.  The earlier copy is dead on arrival -- nothing
/// between the two writes reads it -- and leaving it in would make the
/// destination's final contents ambiguous, silently destroying the live
/// parameter.
fn keep_last_write_per_register(moves: &mut Vec<(X86Register, X86Operand)>) {
    // Walk backwards so the *first* sighting of a register is its last write.
    // `HashSet::insert` returns false for a register already seen.
    let mut written: HashSet<X86Register> = HashSet::with_capacity(moves.len());
    let mut survives: Vec<bool> = moves
        .iter()
        .rev()
        .map(|(destination, _)| written.insert(*destination))
        .collect();
    survives.reverse();

    // `retain` visits elements front to back, in step with `survives`.
    let mut survivor = survives.into_iter();
    moves.retain(|_| survivor.next().unwrap_or(false));
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::{BTreeMap, BTreeSet, HashMap};

    use crate::backend::frame::FrameParams;
    use crate::backend::regalloc::linear_scan;
    use crate::backend::x86::abi::SystemV;

    const PARAMS: FrameParams = FrameParams {
        word_size: WORD_SIZE,
        stack_alignment: abi::STACK_ALIGNMENT,
    };

    /// An empty frame layout -- enough to exercise the helpers that only
    /// emit instructions.
    fn empty_layout() -> FrameLayout<X86Register> {
        FrameLayout::plan(
            PARAMS,
            linear_scan::<SystemV>(&[], &[]),
            &BTreeMap::new(),
            &BTreeSet::new(),
        )
    }

    fn lowering(layout: &FrameLayout<X86Register>) -> FunctionLowering<'_> {
        FunctionLowering {
            out: Vec::new(),
            layout,
            epilogue: ".test_epilogue".to_string(),
            pending_arguments: Vec::new(),
        }
    }

    /// Interpret a sequence of register-to-register `mov`s.
    ///
    /// Every register starts out holding itself as a unique marker value, so
    /// the final contents say exactly which original value each register
    /// ended up with.
    fn simulate(instructions: &[X86Instruction]) -> HashMap<X86Register, X86Register> {
        let mut state: HashMap<X86Register, X86Register> = HashMap::new();
        for instr in instructions {
            match instr {
                X86Instruction::Mov(X86Operand::Reg(destination), X86Operand::Reg(source)) => {
                    // A register not yet written still holds its own marker.
                    let value = *state.get(source).unwrap_or(source);
                    state.insert(*destination, value);
                }
                other => panic!("Expected a register-to-register mov, got {:?}", other),
            }
        }
        state
    }

    /// Assert that `moves` are emitted as a genuine simultaneous assignment:
    /// every destination must end up with the value its source held *before*
    /// any move was executed.
    fn assert_moves_are_parallel(moves: &[(X86Register, X86Operand)]) {
        let layout = empty_layout();
        let mut lowering = lowering(&layout);
        lowering.emit_parallel_moves(moves);
        let state = simulate(&lowering.out);

        for (destination, source) in moves {
            let expected = match source {
                X86Operand::Reg(register) => *register,
                other => panic!("This helper only checks register sources, got {:?}", other),
            };
            assert_eq!(
                state.get(destination).copied().unwrap_or(*destination),
                expected,
                "{:?} should hold the original value of {:?}; emitted: {:?}",
                destination,
                expected,
                lowering.out
            );
        }
    }

    #[test]
    fn parallel_moves_drop_self_moves() {
        // Arrange
        let moves = [
            (X86Register::Rdi, reg(X86Register::Rdi)),
            (X86Register::Rsi, reg(X86Register::Rsi)),
        ];
        let layout = empty_layout();
        let mut lowering = lowering(&layout);

        // Act
        lowering.emit_parallel_moves(&moves);

        // Assert
        assert!(lowering.out.is_empty(), "no-op moves must not be emitted");
    }

    #[test]
    fn parallel_moves_order_chains_before_overwriting_sources() {
        // `rcx <- rdi` must precede `rdi <- rdx`, or RDI's value is lost.
        assert_moves_are_parallel(&[
            (X86Register::Rcx, reg(X86Register::Rdi)),
            (X86Register::Rdi, reg(X86Register::Rdx)),
        ]);
    }

    #[test]
    fn parallel_moves_handle_a_two_register_swap() {
        // A pure cycle: no ordering works, so it must be broken with a
        // scratch register.
        assert_moves_are_parallel(&[
            (X86Register::Rdi, reg(X86Register::Rsi)),
            (X86Register::Rsi, reg(X86Register::Rdi)),
        ]);
    }

    #[test]
    fn parallel_moves_handle_a_cycle_with_a_dangling_chain() {
        // A three-register rotation plus a move that feeds off it.
        assert_moves_are_parallel(&[
            (X86Register::Rdi, reg(X86Register::Rsi)),
            (X86Register::Rsi, reg(X86Register::Rdx)),
            (X86Register::Rdx, reg(X86Register::Rdi)),
            (X86Register::R8, reg(X86Register::Rdx)),
        ]);
    }

    #[test]
    fn parallel_moves_handle_the_full_argument_shuffle() {
        // The System V argument registers rotated by one -- the worst case a
        // six-parameter function can hand the allocator.
        assert_moves_are_parallel(&[
            (X86Register::Rdi, reg(X86Register::R9)),
            (X86Register::Rsi, reg(X86Register::Rdi)),
            (X86Register::Rdx, reg(X86Register::Rsi)),
            (X86Register::Rcx, reg(X86Register::Rdx)),
            (X86Register::R8, reg(X86Register::Rcx)),
            (X86Register::R9, reg(X86Register::R8)),
        ]);
    }

    #[test]
    fn a_register_written_twice_keeps_only_the_last_write() {
        // Arrange: an unused parameter shares R8 with a live one, because its
        // live interval collapsed to its definition.
        let mut moves = vec![
            (X86Register::R8, reg(X86Register::Rcx)),
            (X86Register::Rcx, reg(X86Register::Rdi)),
            (X86Register::R8, reg(X86Register::R8)),
        ];

        // Act
        keep_last_write_per_register(&mut moves);

        // Assert: the dead copy is gone, the rest keeps its order.
        assert_eq!(
            moves,
            vec![
                (X86Register::Rcx, reg(X86Register::Rdi)),
                (X86Register::R8, reg(X86Register::R8)),
            ]
        );
    }

    #[test]
    fn distinct_destinations_survive_deduplication() {
        // Arrange
        let original = vec![
            (X86Register::Rdi, reg(X86Register::Rsi)),
            (X86Register::Rsi, reg(X86Register::Rdx)),
        ];
        let mut moves = original.clone();

        // Act
        keep_last_write_per_register(&mut moves);

        // Assert
        assert_eq!(moves, original);
    }

    #[test]
    fn a_wide_immediate_is_materialized_before_it_is_used() {
        // Arrange: constant folding can produce a value no arithmetic
        // instruction can encode as an immediate.
        let layout = empty_layout();
        let mut lowering = lowering(&layout);

        // Act
        let operand = lowering.encodable(X86Operand::Imm(i64::from(i32::MAX) + 1), SCRATCH2);

        // Assert
        assert_eq!(operand, reg(SCRATCH2));
        assert_eq!(
            lowering.out,
            vec![X86Instruction::Mov(
                reg(SCRATCH2),
                X86Operand::Imm(i64::from(i32::MAX) + 1)
            )]
        );
    }

    #[test]
    fn an_encodable_immediate_is_left_alone() {
        // Arrange / Act
        let layout = empty_layout();
        let mut lowering = lowering(&layout);
        let operand = lowering.encodable(X86Operand::Imm(42), SCRATCH2);

        // Assert
        assert_eq!(operand, X86Operand::Imm(42));
        assert!(lowering.out.is_empty());
    }
}
