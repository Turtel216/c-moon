//! Instruction Selection (Lowering)
//!
//! Translates linearized TAC instructions into x86-64 `X86Instruction`s,
//! consulting the register-allocation map to resolve every virtual register
//! to a physical register or spill slot.
//!
//! Adheres to the **System V AMD64 ABI**:
//! - Arguments 0–5 in RDI, RSI, RDX, RCX, R8, R9
//! - Return value in RAX
//! - Callee-saved: RBX, R12–R15
//! - 16-byte stack alignment before every `call`

use std::collections::{HashMap, HashSet};

use crate::backend::liveness::operand_to_vreg;
use crate::backend::regalloc::AllocationResult;
use crate::backend::x86::*;
use crate::middle::ir::{Opcode, Operand, TACInstruction};

/// System V AMD64 parameter registers, in order.
const PARAM_REGS: &[X86Register] = &[
    X86Register::Rdi,
    X86Register::Rsi,
    X86Register::Rdx,
    X86Register::Rcx,
    X86Register::R8,
    X86Register::R9,
];

/// Primary scratch register for spill loads and intermediate values.
const SCRATCH1: X86Register = X86Register::R10;
/// Secondary scratch register when two spilled operands are needed.
const SCRATCH2: X86Register = X86Register::R11;

// ### Lowering context ###

pub struct LoweringContext {
    /// Emitted x86 instructions (in order).
    out: Vec<X86Instruction>,
    /// Register allocation results.
    alloc: AllocationResult,
    /// Number of callee-saved push instructions (for frame offset maths).
    num_callee_pushes: usize,
    /// Epilogue label -- all `ret` TAC instructions jump here.
    epilogue_label: String,
    /// Buffered Param operands for the next Call instruction.
    param_buffer: Vec<Operand>,
    /// Maps array variable id -> RBP offset of element[0].
    /// element[i] is at `[rbp + offset - i*8]` (stack grows downward).
    array_offsets: HashMap<usize, i32>,
    /// Maps address-taken variable id -> RBP offset.
    /// Variables whose address is taken with AddrOf must live on the stack.
    addr_taken_offsets: HashMap<usize, i32>,
}

impl LoweringContext {
    /// Lower one function's worth of linearized TAC into an `X86Function`.
    pub fn lower_function(
        name: &str,
        instructions: &[(TACInstruction, String)],
        block_order: &[String],
        mut alloc: AllocationResult,
        array_sizes: &HashMap<usize, usize>,
        addr_taken_vars: &HashSet<usize>,
    ) -> X86Function {
        let epilogue_label = format!(".{}_epilogue", name);
        let num_callee_pushes = alloc.callee_saved_used.len();

        // Pre-allocate contiguous stack slots for each local array
        //
        // Arrays live below the scalar spill area.  For each array of
        // `n` elements reserve `n * 8` bytes.  The element at index 0
        // is stored at the *highest* address (closest to RBP) so that
        // increasing indices move toward lower addresses, consistent
        // with the downward-growing stack.
        //
        // Stack layout (relative to RBP, growing downward):
        //   [rbp - 8*1 .. rbp - 8*C]   callee-saved registers (C pushes)
        //   [rbp - 8*(C+1) .. ...]      scalar spill slots
        //   [rbp - 8*(C+S+1) .. ...]    array storage  <-- NEW
        //
        let mut array_offsets: HashMap<usize, i32> = HashMap::new();
        let mut extra_slots: usize = 0;

        for (var_id, elem_count) in array_sizes {
            // element[0] offset = -(callee_pushes + existing_spill + extra_so_far + 1) * 8
            let elem0_slot = alloc.stack_slots + extra_slots + 1;
            let elem0_offset = -((num_callee_pushes as i32) * 8 + (elem0_slot as i32) * 8);
            array_offsets.insert(*var_id, elem0_offset);
            extra_slots += *elem_count;
        }

        // Grow the total stack_slots so the prologue allocates enough space.
        alloc.stack_slots += extra_slots;

        // Allocate dedicated stack slots for address-taken variables.
        // These variables must live on the stack so that AddrOf can
        // compute their address with LEA.
        let mut addr_taken_offsets: HashMap<usize, i32> = HashMap::new();
        let mut addr_extra: usize = 0;
        for var_id in addr_taken_vars {
            addr_extra += 1;
            let slot = alloc.stack_slots + addr_extra;
            let offset = -((num_callee_pushes as i32) * 8 + (slot as i32) * 8);
            addr_taken_offsets.insert(*var_id, offset);
        }

        // Update total stack slots to include address-taken vars.
        alloc.stack_slots += addr_extra;

        let mut ctx = LoweringContext {
            out: Vec::new(),
            alloc,
            num_callee_pushes,
            epilogue_label,
            param_buffer: Vec::new(),
            array_offsets,
            addr_taken_offsets,
        };

        ctx.emit_prologue();

        // Track which block labels we've already emitted so we insert
        // label pseudo-instructions at block boundaries.
        let mut current_block: Option<&str> = None;

        let mut position = 0;
        while position < instructions.len() {
            let (instr, block_label) = &instructions[position];

            // Emit a label when we enter a new basic block.
            if current_block != Some(block_label.as_str()) {
                current_block = Some(block_label.as_str());
                ctx.emit(X86Instruction::Label(format!(".{}", block_label)));
            }

            // Consecutive `GetParam`s must be lowered as one group: each one
            // reads an incoming argument register that a later one's
            // destination may overwrite, so they have to be scheduled
            // together (see `lower_get_params`).
            if instr.opcode == Opcode::GetParam {
                // `position(..)` returns an offset into the sub-slice, hence
                // the `position +` when converting it back to an index.
                let group_end = instructions[position..]
                    .iter()
                    .position(|(next, label)| {
                        next.opcode != Opcode::GetParam || label != block_label
                    })
                    .map_or(instructions.len(), |offset| position + offset);

                let group: Vec<&TACInstruction> = instructions[position..group_end]
                    .iter()
                    .map(|(get_param, _)| get_param)
                    .collect();
                ctx.lower_get_params(&group);
                position = group_end;
                continue;
            }

            ctx.lower_instruction(instr);
            position += 1;
        }

        // Emit labels for blocks that had no instructions (e.g. exit block).
        for label in block_order {
            let asm_label = format!(".{}", label);
            if !ctx
                .out
                .iter()
                .any(|i| matches!(i, X86Instruction::Label(l) if *l == asm_label))
            {
                ctx.emit(X86Instruction::Label(asm_label));
            }
        }

        ctx.emit_epilogue();

        X86Function {
            name: name.to_string(),
            instructions: ctx.out,
        }
    }

    fn emit_prologue(&mut self) {
        self.out
            .push(X86Instruction::Push(X86Operand::Reg(X86Register::Rbp)));
        self.out.push(X86Instruction::Mov(
            X86Operand::Reg(X86Register::Rbp),
            X86Operand::Reg(X86Register::Rsp),
        ));

        // Save callee-saved registers.
        // Clone to avoid borrowing self.alloc while pushing to self.out.
        let callee_saved = self.alloc.callee_saved_used.clone();
        for reg in &callee_saved {
            self.out.push(X86Instruction::Push(X86Operand::Reg(*reg)));
        }

        // Allocate spill area.  The total frame below RBP is
        // (callee_pushes * 8) + (spill_slots * 8), and must be
        // 16-byte aligned.
        let spill_bytes = (self.alloc.stack_slots * 8) as i32;
        if spill_bytes > 0 {
            // Round up to 16-byte alignment considering callee pushes.
            let total = (self.num_callee_pushes as i32) * 8 + spill_bytes;
            let aligned = (total + 15) & !15;
            let sub_amount = aligned - (self.num_callee_pushes as i32) * 8;
            if sub_amount > 0 {
                self.out.push(X86Instruction::Sub(
                    X86Operand::Reg(X86Register::Rsp),
                    X86Operand::Imm(sub_amount as i64),
                ));
            }
        } else if self.num_callee_pushes % 2 != 0 {
            // Odd number of pushes — need padding for 16-byte alignment.
            self.out.push(X86Instruction::Sub(
                X86Operand::Reg(X86Register::Rsp),
                X86Operand::Imm(8),
            ));
        }
    }

    fn emit_epilogue(&mut self) {
        let epilogue_label = self.epilogue_label.clone();
        let callee_saved = self.alloc.callee_saved_used.clone();

        self.out.push(X86Instruction::Label(epilogue_label));

        if !callee_saved.is_empty() {
            // Restore RSP to point just past the callee-saved pushes,
            // then pop them in reverse order.
            let offset = (self.num_callee_pushes as i32) * 8;
            self.out.push(X86Instruction::Lea(
                X86Operand::Reg(X86Register::Rsp),
                X86Operand::Mem(X86Register::Rbp, -offset),
            ));
            for reg in callee_saved.iter().rev() {
                self.out.push(X86Instruction::Pop(X86Operand::Reg(*reg)));
            }
        } else {
            self.out.push(X86Instruction::Mov(
                X86Operand::Reg(X86Register::Rsp),
                X86Operand::Reg(X86Register::Rbp),
            ));
        }

        self.out
            .push(X86Instruction::Pop(X86Operand::Reg(X86Register::Rbp)));
        self.out.push(X86Instruction::Ret);
    }

    fn lower_instruction(&mut self, instr: &TACInstruction) {
        match instr.opcode {
            Opcode::Add => self.lower_binary(instr, BinKind::Add),
            Opcode::Sub => self.lower_binary(instr, BinKind::Sub),
            Opcode::Mul => self.lower_binary(instr, BinKind::Mul),
            Opcode::Div => self.lower_div(instr),

            Opcode::Eq => self.lower_cmp(instr, ConditionCode::E),
            Opcode::Neq => self.lower_cmp(instr, ConditionCode::Ne),
            Opcode::Lt => self.lower_cmp(instr, ConditionCode::L),
            Opcode::Lte => self.lower_cmp(instr, ConditionCode::Le),
            Opcode::Gt => self.lower_cmp(instr, ConditionCode::G),
            Opcode::Gte => self.lower_cmp(instr, ConditionCode::Ge),

            Opcode::Mov => self.lower_mov(instr),
            Opcode::Jump => self.lower_jump(instr),
            Opcode::BranchIf => self.lower_branch(instr, true),
            Opcode::BranchIfNot => self.lower_branch(instr, false),

            Opcode::Param => self.lower_param(instr),
            Opcode::Call => self.lower_call(instr),
            Opcode::Ret => self.lower_ret(instr),
            // A lone `GetParam` is a group of one; runs of them are batched
            // by `lower_function` before reaching this dispatcher.
            Opcode::GetParam => self.lower_get_params(&[instr]),

            Opcode::ArrayStore => self.lower_array_store(instr),
            Opcode::ArrayLoad => self.lower_array_load(instr),

            Opcode::Load => self.lower_load(instr),
            Opcode::Store => self.lower_store(instr),
            Opcode::AddrOf => self.lower_addr_of(instr),
        }
    }

    fn lower_binary(&mut self, instr: &TACInstruction, kind: BinKind) {
        let dest = instr.dest.as_ref().unwrap();
        let arg1 = instr.arg1.as_ref().unwrap();
        let arg2 = instr.arg2.as_ref().unwrap();

        // Determine the working register for the two-operand form.
        let dest_reg = self.dest_reg(dest);

        // mov dest_reg, arg1
        let src1 = self.resolve(arg1, dest_reg);
        if src1 != X86Operand::Reg(dest_reg) {
            self.emit(X86Instruction::Mov(X86Operand::Reg(dest_reg), src1));
        }

        // <op> dest_reg, arg2
        let src2 = self.resolve(arg2, SCRATCH2);
        match kind {
            BinKind::Add => self.emit(X86Instruction::Add(X86Operand::Reg(dest_reg), src2)),
            BinKind::Sub => self.emit(X86Instruction::Sub(X86Operand::Reg(dest_reg), src2)),
            BinKind::Mul => self.emit(X86Instruction::Imul(X86Operand::Reg(dest_reg), src2)),
        }

        // Store back if dest is spilled.
        self.store_if_spilled(dest, dest_reg);
    }

    /// Division: `dest = arg1 / arg2`
    ///   mov rax, arg1 ; cqo ; idiv arg2 ; mov dest, rax
    fn lower_div(&mut self, instr: &TACInstruction) {
        let dest = instr.dest.as_ref().unwrap();
        let arg1 = instr.arg1.as_ref().unwrap();
        let arg2 = instr.arg2.as_ref().unwrap();

        // Load dividend into RAX.
        let src1 = self.resolve(arg1, SCRATCH1);
        self.emit(X86Instruction::Mov(X86Operand::Reg(X86Register::Rax), src1));

        // Sign-extend RAX → RDX:RAX.
        self.emit(X86Instruction::Cqo);

        // idiv cannot take an immediate — load into scratch if needed.
        let divisor = self.resolve(arg2, SCRATCH2);
        let divisor = match divisor {
            X86Operand::Imm(v) => {
                self.emit(X86Instruction::Mov(
                    X86Operand::Reg(SCRATCH2),
                    X86Operand::Imm(v),
                ));
                X86Operand::Reg(SCRATCH2)
            }
            other => other,
        };
        self.emit(X86Instruction::Idiv(divisor));

        // Quotient is in RAX — move to destination.
        let dest_reg = self.dest_reg(dest);
        if dest_reg != X86Register::Rax {
            self.emit(X86Instruction::Mov(
                X86Operand::Reg(dest_reg),
                X86Operand::Reg(X86Register::Rax),
            ));
        }
        self.store_if_spilled(dest, dest_reg);
    }

    /// `dest = arg1 <cmp> arg2` → `cmp; setCC al; movzx dest, al`
    fn lower_cmp(&mut self, instr: &TACInstruction, cc: ConditionCode) {
        let dest = instr.dest.as_ref().unwrap();
        let arg1 = instr.arg1.as_ref().unwrap();
        let arg2 = instr.arg2.as_ref().unwrap();

        // cmp lhs, rhs  (lhs must be a register)
        let lhs = self.resolve(arg1, SCRATCH1);
        let lhs = self.ensure_reg(lhs, SCRATCH1);
        let rhs = self.resolve(arg2, SCRATCH2);
        self.emit(X86Instruction::Cmp(lhs, rhs));

        // setCC al
        self.emit(X86Instruction::SetCC(
            cc,
            X86Operand::Reg(X86Register::Rax), // placeholder — emitter uses low byte name
        ));

        // movzx dest_reg, al  (zero-extend byte → 64-bit)
        let dest_reg = self.dest_reg(dest);
        self.emit(X86Instruction::Movzx(
            X86Operand::Reg(dest_reg),
            X86Operand::Reg(X86Register::Rax),
        ));

        self.store_if_spilled(dest, dest_reg);
    }

    fn lower_mov(&mut self, instr: &TACInstruction) {
        let dest = instr.dest.as_ref().unwrap();
        let arg1 = instr.arg1.as_ref().unwrap();

        let dest_reg = self.dest_reg(dest);
        let src = self.resolve(arg1, dest_reg);

        if src != X86Operand::Reg(dest_reg) {
            self.emit(X86Instruction::Mov(X86Operand::Reg(dest_reg), src));
        }

        self.store_if_spilled(dest, dest_reg);
        // If dest is an address-taken variable, also write to its stack slot.
        self.sync_addr_taken_if_needed(dest, dest_reg);
    }

    fn lower_jump(&mut self, instr: &TACInstruction) {
        if let Some(Operand::Label(ref lbl)) = instr.arg1 {
            self.emit(X86Instruction::Jmp(format!(".{}", lbl)));
        }
    }

    fn lower_branch(&mut self, instr: &TACInstruction, branch_if_true: bool) {
        let cond = instr.arg1.as_ref().unwrap();
        let target = match instr.arg2.as_ref().unwrap() {
            Operand::Label(l) => format!(".{}", l),
            _ => panic!("BranchIf/Not arg2 must be a label"),
        };

        let cond_op = self.resolve(cond, SCRATCH1);
        let cond_op = self.ensure_reg(cond_op, SCRATCH1);

        // test cond, cond  (sets ZF)
        self.emit(X86Instruction::Test(cond_op.clone(), cond_op));

        // jne (BranchIf) or je (BranchIfNot)
        let cc = if branch_if_true {
            ConditionCode::Ne
        } else {
            ConditionCode::E
        };
        self.emit(X86Instruction::Jcc(cc, target));
    }

    fn lower_param(&mut self, instr: &TACInstruction) {
        // Buffer the operand — it will be emitted when we see the Call.
        self.param_buffer.push(instr.arg1.as_ref().unwrap().clone());
    }

    fn lower_call(&mut self, instr: &TACInstruction) {
        let func_label = match instr.arg1.as_ref().unwrap() {
            Operand::Label(l) => l.clone(),
            _ => panic!("Call arg1 must be a label"),
        };

        // Move buffered arguments into ABI registers (first 6) or stack.
        let args: Vec<Operand> = self.param_buffer.drain(..).collect();

        let stack_args = args.get(PARAM_REGS.len()..).unwrap_or(&[]);
        let stack_arg_count = stack_args.len();

        // RSP is 16-byte aligned throughout the body (the prologue keeps it
        // so), and `call` pushes 8 bytes, so an odd number of 8-byte stack
        // arguments needs 8 bytes of padding.  The padding goes *below* the
        // arguments -- it must be reserved before they are pushed, otherwise
        // it would sit between them and the return address and the callee
        // would read it as argument 7.
        let needs_alignment = stack_arg_count % 2 != 0;
        if needs_alignment {
            self.emit(X86Instruction::Sub(
                X86Operand::Reg(X86Register::Rsp),
                X86Operand::Imm(8),
            ));
        }

        // Stack args are pushed right-to-left, so the 7th argument ends up
        // closest to the return address, at `[rbp + 16]` in the callee.
        for arg in stack_args.iter().rev() {
            let val = self.resolve(arg, SCRATCH1);
            let val = self.ensure_reg(val, SCRATCH1);
            self.emit(X86Instruction::Push(val));
        }

        // Register args.  These are a simultaneous assignment: an argument
        // may currently live in a register that is another argument's ABI
        // destination, so the moves must be ordered (or broken with a
        // scratch register) rather than emitted blindly.
        let register_moves: Vec<(X86Register, X86Operand)> = args
            .iter()
            .zip(PARAM_REGS)
            .map(|(arg, &target_reg)| (target_reg, self.resolve_in_place(arg)))
            .collect();
        self.emit_parallel_moves(&register_moves);

        self.emit(X86Instruction::Call(func_label));

        // Clean up stack args + alignment padding.
        let cleanup = (stack_arg_count + if needs_alignment { 1 } else { 0 }) * 8;
        if cleanup > 0 {
            self.emit(X86Instruction::Add(
                X86Operand::Reg(X86Register::Rsp),
                X86Operand::Imm(cleanup as i64),
            ));
        }

        // Move return value (RAX) into destination.
        if let Some(ref dest) = instr.dest {
            let dest_reg = self.dest_reg(dest);
            if dest_reg != X86Register::Rax {
                self.emit(X86Instruction::Mov(
                    X86Operand::Reg(dest_reg),
                    X86Operand::Reg(X86Register::Rax),
                ));
            }
            self.store_if_spilled(dest, dest_reg);
        }
    }

    fn lower_ret(&mut self, instr: &TACInstruction) {
        // Move return value into RAX.
        if let Some(ref val) = instr.arg1 {
            let src = self.resolve(val, X86Register::Rax);
            if src != X86Operand::Reg(X86Register::Rax) {
                self.emit(X86Instruction::Mov(X86Operand::Reg(X86Register::Rax), src));
            }
        }
        // Jump to the shared epilogue.
        self.emit(X86Instruction::Jmp(self.epilogue_label.clone()));
    }

    /// Lower a run of consecutive `GetParam` instructions.
    ///
    /// Incoming arguments arrive in fixed ABI locations (RDI, RSI, RDX, RCX,
    /// R8, R9, then `[rbp + 16]` upwards), while the register allocator is
    /// free to place the corresponding locals anywhere.  Emitting one `mov`
    /// per parameter in isolation is therefore wrong: a local assigned to,
    /// say, RCX would clobber the incoming 4th argument before the parameter
    /// that needs it has been read.  The whole run must be treated as a
    /// single simultaneous assignment.
    ///
    /// The run is lowered in two phases:
    ///
    /// 1. Parameters that live in memory (spilled, or address-taken and thus
    ///    pinned to a stack slot) are written first, while every incoming
    ///    argument register still holds its original value.
    /// 2. Parameters that live in registers are shuffled with
    ///    [`Self::emit_parallel_moves`], which orders the moves so that no
    ///    source is destroyed before it is read.
    fn lower_get_params(&mut self, group: &[&TACInstruction]) {
        let mut register_moves: Vec<(X86Register, X86Operand)> = Vec::with_capacity(group.len());

        for instr in group {
            let dest = instr
                .dest
                .as_ref()
                .expect("Compiler Bug: GetParam must have a destination");
            let index = match instr.arg1.as_ref() {
                Some(Operand::ImmInt(i)) => *i as usize,
                _ => panic!("GetParam arg1 must be an immediate index"),
            };
            let source = Self::incoming_param_location(index);

            // --- Phase 1: memory destinations. ---
            // A variable can need both a spill slot and an address-taken
            // slot, so collect every memory home before emitting.
            let mut memory_offsets: Vec<i32> = Vec::new();
            let vreg = operand_to_vreg(dest).expect("GetParam dest must be a vreg");
            match self.alloc.mapping.get(&vreg) {
                Some(StorageLocation::Register(reg)) => register_moves.push((*reg, source.clone())),
                Some(StorageLocation::Stack(slot)) => memory_offsets.push(self.spill_offset(*slot)),
                None => panic!("VirtualReg {} has no allocation", vreg),
            }
            if let Operand::Var(id) = dest {
                if let Some(&offset) = self.addr_taken_offsets.get(id) {
                    memory_offsets.push(offset);
                }
            }

            if !memory_offsets.is_empty() {
                // x86 has no memory-to-memory `mov`, so a stack-passed
                // argument has to go through a scratch register.
                let value = self.ensure_reg(source, SCRATCH1);
                for offset in memory_offsets {
                    self.emit(X86Instruction::Mov(
                        X86Operand::Mem(X86Register::Rbp, offset),
                        value.clone(),
                    ));
                }
            }
        }

        // --- Phase 2: register destinations. ---
        keep_last_write_per_register(&mut register_moves);
        self.emit_parallel_moves(&register_moves);
    }

    /// Where the caller left incoming argument `index`, per the System V
    /// AMD64 ABI: the first six in registers, the rest on the stack just
    /// above the saved return address (`[rbp + 16]` = argument 7).
    fn incoming_param_location(index: usize) -> X86Operand {
        match PARAM_REGS.get(index) {
            Some(&reg) => X86Operand::Reg(reg),
            None => X86Operand::Mem(
                X86Register::Rbp,
                16 + ((index - PARAM_REGS.len()) * 8) as i32,
            ),
        }
    }

    // ### Helpers ###

    fn emit(&mut self, instr: X86Instruction) {
        self.out.push(instr);
    }

    /// Compute the RBP offset for spill slot `slot` (1-indexed).
    /// Layout below RBP: [callee-saved pushes] [spill slots]
    fn spill_offset(&self, slot: i32) -> i32 {
        -((self.num_callee_pushes as i32) * 8 + slot * 8)
    }

    /// Resolve a TAC `Operand` to the `X86Operand` that holds it, without
    /// emitting anything.  Spilled and address-taken variables resolve to
    /// their RBP-relative stack slot, which `mov` can read directly.
    ///
    /// Prefer [`Self::resolve`] unless the caller can genuinely accept a
    /// memory operand — most x86 instructions allow at most one.
    fn resolve_in_place(&self, op: &Operand) -> X86Operand {
        // For address-taken variables, always read the dedicated stack slot.
        // This ensures pointer writes are visible when reading the variable
        // by name.
        if let Operand::Var(id) = op {
            if let Some(&offset) = self.addr_taken_offsets.get(id) {
                return X86Operand::Mem(X86Register::Rbp, offset);
            }
        }

        match op {
            Operand::ImmInt(v) => X86Operand::Imm(*v),
            Operand::Label(l) => X86Operand::Label(l.clone()),
            Operand::Var(_) | Operand::Temp(_) => {
                let vreg = operand_to_vreg(op).unwrap();
                match self.alloc.mapping.get(&vreg) {
                    Some(StorageLocation::Register(r)) => X86Operand::Reg(*r),
                    Some(StorageLocation::Stack(slot)) => {
                        X86Operand::Mem(X86Register::Rbp, self.spill_offset(*slot))
                    }
                    None => panic!("VirtualReg {} has no allocation", vreg),
                }
            }
        }
    }

    /// Resolve a TAC `Operand` into an `X86Operand`.
    /// If the operand lives in memory (a spilled or address-taken vreg), a
    /// `mov` into `scratch` is emitted and the scratch register is returned.
    fn resolve(&mut self, op: &Operand, scratch: X86Register) -> X86Operand {
        match self.resolve_in_place(op) {
            memory @ X86Operand::Mem(..) => {
                self.emit(X86Instruction::Mov(X86Operand::Reg(scratch), memory));
                X86Operand::Reg(scratch)
            }
            other => other,
        }
    }

    /// Emit a set of `mov`s into registers that must take effect *as if*
    /// they all happened at once.
    ///
    /// Emitting them naively is wrong whenever one move's destination is
    /// another move's source, because the second move would then read a
    /// value that has already been overwritten.  This is the classic
    /// "parallel move" problem; it shows up wherever values are shuffled
    /// between fixed ABI registers and allocated ones.
    ///
    /// A move becomes safe to emit once no other pending move reads its
    /// destination.  When no such move exists, the remaining moves form a
    /// cycle (e.g. `rdi <- rsi` together with `rsi <- rdi`); it is broken by
    /// stashing one register in `SCRATCH1` and rewriting the reads of it.
    /// Only register sources can conflict -- immediates and RBP-relative
    /// memory operands are never written here.
    ///
    /// Destinations must never be `SCRATCH1`; both call sites use ABI
    /// argument registers or allocated ones, and neither pool contains it.
    fn emit_parallel_moves(&mut self, moves: &[(X86Register, X86Operand)]) {
        // Self-moves (`mov rax, rax`) are dropped: they are no-ops, and
        // keeping them would make every such register look "read" and stall
        // the scheduling loop below.
        let mut pending: Vec<(X86Register, X86Operand)> = moves
            .iter()
            .filter(|(dest, src)| *src != X86Operand::Reg(*dest))
            .cloned()
            .collect();

        while !pending.is_empty() {
            let ready = pending
                .iter()
                .position(|(dest, _)| !reads_register(&pending, *dest));

            let index = match ready {
                Some(index) => index,
                None => {
                    // Every remaining destination is still needed as a
                    // source, so break the cycle: save one register's value
                    // in the scratch register and read it from there.
                    let stashed = pending[0].0;
                    self.emit(X86Instruction::Mov(
                        X86Operand::Reg(SCRATCH1),
                        X86Operand::Reg(stashed),
                    ));
                    for (_, src) in pending.iter_mut() {
                        if *src == X86Operand::Reg(stashed) {
                            *src = X86Operand::Reg(SCRATCH1);
                        }
                    }
                    // `stashed` is no longer read, so its move is now ready.
                    0
                }
            };

            let (dest, src) = pending.remove(index);
            self.emit(X86Instruction::Mov(X86Operand::Reg(dest), src));
        }
    }

    /// Return the physical register holding `dest`, or SCRATCH1 if spilled.
    fn dest_reg(&self, dest: &Operand) -> X86Register {
        let vreg = operand_to_vreg(dest).unwrap();
        match self.alloc.mapping.get(&vreg) {
            Some(StorageLocation::Register(r)) => *r,
            Some(StorageLocation::Stack(_)) => SCRATCH1,
            None => panic!("VirtualReg {} has no allocation", vreg),
        }
    }

    /// If `dest` was spilled, emit a store from `value_reg` back to its slot.
    fn store_if_spilled(&mut self, dest: &Operand, value_reg: X86Register) {
        let vreg = operand_to_vreg(dest).unwrap();
        if let Some(StorageLocation::Stack(slot)) = self.alloc.mapping.get(&vreg) {
            let offset = self.spill_offset(*slot);
            self.emit(X86Instruction::Mov(
                X86Operand::Mem(X86Register::Rbp, offset),
                X86Operand::Reg(value_reg),
            ));
        }
    }

    /// Ensure an operand is in a register. If it's an immediate, emit a
    /// `mov` into `scratch` and return the register operand.
    fn ensure_reg(&mut self, op: X86Operand, scratch: X86Register) -> X86Operand {
        match op {
            X86Operand::Reg(_) => op,
            other => {
                self.emit(X86Instruction::Mov(X86Operand::Reg(scratch), other));
                X86Operand::Reg(scratch)
            }
        }
    }

    // ### Array instruction lowering ###

    /// Lower `ArrayStore base_var, index, value`.
    ///
    /// Generates x86 that computes the element address and stores into it:
    ///
    /// ```text
    ///   ; element_addr = rbp + base_offset - index * 8
    ///   ;   base_offset is the RBP offset of element[0] (negative)
    ///   ;   Subtracting index*8 moves to lower addresses for higher indices,
    ///   ;   consistent with downward stack growth.
    ///
    ///   mov  SCRATCH1, <index>       ; load the index into a register
    ///   imul SCRATCH1, SCRATCH1, 8   ; byte offset = index * sizeof(int64)
    ///   lea  SCRATCH2, [rbp + base_offset]  ; address of element[0]
    ///   sub  SCRATCH2, SCRATCH1      ; address of element[index]
    ///   mov  SCRATCH1, <value>       ; load the value to store
    ///   mov  [SCRATCH2], SCRATCH1    ; store value into the array slot
    /// ```
    fn lower_array_store(&mut self, instr: &TACInstruction) {
        // dest = base array var, arg1 = index, arg2 = value
        let base = instr.dest.as_ref().unwrap();
        let index = instr.arg1.as_ref().unwrap();
        let value = instr.arg2.as_ref().unwrap();

        // Look up the RBP offset for element[0] of this array.
        let var_id = match base {
            Operand::Var(id) => *id,
            _ => panic!("ArrayStore base must be a Var"),
        };
        let base_offset = *self
            .array_offsets
            .get(&var_id)
            .expect("Compiler Bug: array base variable has no allocated stack offset");

        // Compute byte offset = index * 8
        let idx_op = self.resolve(index, SCRATCH1);
        if idx_op != X86Operand::Reg(SCRATCH1) {
            self.emit(X86Instruction::Mov(X86Operand::Reg(SCRATCH1), idx_op));
        }
        // SCRATCH1 = index * 8  (each element is 8 bytes wide)
        self.emit(X86Instruction::Imul(
            X86Operand::Reg(SCRATCH1),
            X86Operand::Imm(8),
        ));

        // Compute element address
        //   lea SCRATCH2, [rbp + base_offset]  — address of arr[0]
        self.emit(X86Instruction::Lea(
            X86Operand::Reg(SCRATCH2),
            X86Operand::Mem(X86Register::Rbp, base_offset),
        ));
        // sub SCRATCH2, SCRATCH1  — arr[0] - index*8 = addr of arr[index]
        self.emit(X86Instruction::Sub(
            X86Operand::Reg(SCRATCH2),
            X86Operand::Reg(SCRATCH1),
        ));

        // Store the value
        let val_op = self.resolve(value, SCRATCH1);
        let val_op = self.ensure_reg(val_op, SCRATCH1);
        //   mov [SCRATCH2], value_reg  — write value into computed array slot
        self.emit(X86Instruction::Mov(X86Operand::Mem(SCRATCH2, 0), val_op));
    }

    /// Lower `ArrayLoad dest, base_var, index`.
    ///
    /// Generates x86 that computes the element address and loads from it:
    ///
    /// ```text
    ///   ; Same addressing math as ArrayStore:
    ///   ;   element_addr = rbp + base_offset - index * 8
    ///
    ///   mov  SCRATCH1, <index>
    ///   imul SCRATCH1, SCRATCH1, 8
    ///   lea  SCRATCH2, [rbp + base_offset]
    ///   sub  SCRATCH2, SCRATCH1
    ///   mov  <dest_reg>, [SCRATCH2]  ; load the array element
    /// ```
    fn lower_array_load(&mut self, instr: &TACInstruction) {
        // dest = destination, arg1 = base array var, arg2 = index
        let dest = instr.dest.as_ref().unwrap();
        let base = instr.arg1.as_ref().unwrap();
        let index = instr.arg2.as_ref().unwrap();

        // Look up the RBP offset for element[0].
        let var_id = match base {
            Operand::Var(id) => *id,
            _ => panic!("ArrayLoad base must be a Var"),
        };
        let base_offset = *self
            .array_offsets
            .get(&var_id)
            .expect("Compiler Bug: array base variable has no allocated stack offset");

        // byte offset = index * 8
        let idx_op = self.resolve(index, SCRATCH1);
        if idx_op != X86Operand::Reg(SCRATCH1) {
            self.emit(X86Instruction::Mov(X86Operand::Reg(SCRATCH1), idx_op));
        }
        self.emit(X86Instruction::Imul(
            X86Operand::Reg(SCRATCH1),
            X86Operand::Imm(8),
        ));

        //  element address = base_of_arr[0] - byte_offset
        self.emit(X86Instruction::Lea(
            X86Operand::Reg(SCRATCH2),
            X86Operand::Mem(X86Register::Rbp, base_offset),
        ));
        self.emit(X86Instruction::Sub(
            X86Operand::Reg(SCRATCH2),
            X86Operand::Reg(SCRATCH1),
        ));

        // load the value from the computed address into dest.
        let dest_reg = self.dest_reg(dest);
        self.emit(X86Instruction::Mov(
            X86Operand::Reg(dest_reg),
            X86Operand::Mem(SCRATCH2, 0),
        ));

        self.store_if_spilled(dest, dest_reg);
    }

    // ### Pointer instruction lowering ###

    /// Lower `AddrOf dest, var`.
    ///
    /// Computes the stack address of the variable and loads it into dest:
    /// ```text
    ///   lea dest_reg, [rbp + var_offset]
    /// ```
    fn lower_addr_of(&mut self, instr: &TACInstruction) {
        let dest = instr.dest.as_ref().unwrap();
        let src = instr.arg1.as_ref().unwrap();

        let var_id = match src {
            Operand::Var(id) => *id,
            _ => panic!("AddrOf source must be a Var"),
        };

        // Look up the dedicated stack offset for this address-taken variable.
        let var_offset = *self
            .addr_taken_offsets
            .get(&var_id)
            .expect("Compiler Bug: AddrOf on variable without allocated stack slot");

        let dest_reg = self.dest_reg(dest);
        self.emit(X86Instruction::Lea(
            X86Operand::Reg(dest_reg),
            X86Operand::Mem(X86Register::Rbp, var_offset),
        ));

        self.store_if_spilled(dest, dest_reg);
    }

    /// Lower `Load dest, ptr_addr`.
    ///
    /// Reads a value from the memory address held in ptr_addr:
    /// ```text
    ///   mov scratch, ptr_addr    ; get the pointer value into a register
    ///   mov dest_reg, [scratch]  ; dereference
    /// ```
    fn lower_load(&mut self, instr: &TACInstruction) {
        let dest = instr.dest.as_ref().unwrap();
        let addr = instr.arg1.as_ref().unwrap();

        // Resolve the pointer address into a register.
        let addr_op = self.resolve(addr, SCRATCH2);
        let addr_op = self.ensure_reg(addr_op, SCRATCH2);

        let addr_reg = match addr_op {
            X86Operand::Reg(r) => r,
            _ => unreachable!(),
        };

        // Dereference: dest_reg = [addr_reg]
        let dest_reg = self.dest_reg(dest);
        self.emit(X86Instruction::Mov(
            X86Operand::Reg(dest_reg),
            X86Operand::Mem(addr_reg, 0),
        ));

        self.store_if_spilled(dest, dest_reg);
    }

    /// Lower `Store addr, value`.
    ///
    /// Writes a value to the memory address:
    /// ```text
    ///   mov scratch2, addr       ; get the pointer into a register
    ///   mov scratch1, value      ; get the value into a register
    ///   mov [scratch2], scratch1 ; write through the pointer
    /// ```
    fn lower_store(&mut self, instr: &TACInstruction) {
        let addr = instr.arg1.as_ref().unwrap();
        let value = instr.arg2.as_ref().unwrap();

        // Resolve the pointer address into SCRATCH2.
        let addr_op = self.resolve(addr, SCRATCH2);
        if addr_op != X86Operand::Reg(SCRATCH2) {
            self.emit(X86Instruction::Mov(X86Operand::Reg(SCRATCH2), addr_op));
        }

        // Resolve the value into SCRATCH1.
        let val_op = self.resolve(value, SCRATCH1);
        let val_op = self.ensure_reg(val_op, SCRATCH1);

        // Write through the pointer: [SCRATCH2] = val_reg
        self.emit(X86Instruction::Mov(X86Operand::Mem(SCRATCH2, 0), val_op));
    }

    /// When a `Mov` targets an address-taken variable, sync the value to the
    /// dedicated stack slot so that subsequent `AddrOf` + `Load` can see it.
    fn sync_addr_taken_if_needed(&mut self, dest: &Operand, value_reg: X86Register) {
        if let Operand::Var(id) = dest {
            if let Some(&offset) = self.addr_taken_offsets.get(id) {
                self.emit(X86Instruction::Mov(
                    X86Operand::Mem(X86Register::Rbp, offset),
                    X86Operand::Reg(value_reg),
                ));
            }
        }
    }
}

/// Drop every move whose destination is written again later in the group,
/// keeping only the last write to each register.
///
/// Two parameters can be handed the same register: an unused parameter's
/// live interval collapses to its definition, so the allocator is free to
/// reuse its register for a later one.  The earlier copy is dead on arrival
/// -- nothing between the two writes reads it -- and leaving it in would
/// make the destination's final contents ambiguous, silently destroying the
/// live parameter.
fn keep_last_write_per_register(moves: &mut Vec<(X86Register, X86Operand)>) {
    // Walk backwards so the *first* sighting of a register is its last write.
    // `HashSet::insert` returns false for a register already seen.
    let mut written: HashSet<X86Register> = HashSet::with_capacity(moves.len());
    let mut survives: Vec<bool> = moves
        .iter()
        .rev()
        .map(|(dest, _)| written.insert(*dest))
        .collect();
    survives.reverse();

    // `retain` visits elements front to back, in step with `survives`.
    let mut survivor = survives.into_iter();
    moves.retain(|_| survivor.next().unwrap_or(false));
}

/// Returns `true` if any pending parallel move still reads `reg` as its source.
fn reads_register(pending: &[(X86Register, X86Operand)], reg: X86Register) -> bool {
    pending.iter().any(|(_, src)| *src == X86Operand::Reg(reg))
}

/// Internal helper enum to distinguish binary operations during lowering.
enum BinKind {
    Add,
    Sub,
    Mul,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A lowering context with an empty allocation — enough to exercise the
    /// helpers that only emit instructions.
    fn test_context() -> LoweringContext {
        LoweringContext {
            out: Vec::new(),
            alloc: AllocationResult {
                mapping: HashMap::new(),
                stack_slots: 0,
                callee_saved_used: Vec::new(),
            },
            num_callee_pushes: 0,
            epilogue_label: ".test_epilogue".to_string(),
            param_buffer: Vec::new(),
            array_offsets: HashMap::new(),
            addr_taken_offsets: HashMap::new(),
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
                X86Instruction::Mov(X86Operand::Reg(dest), X86Operand::Reg(src)) => {
                    // A register not yet written still holds its own marker.
                    let value = *state.get(src).unwrap_or(src);
                    state.insert(*dest, value);
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
        let mut ctx = test_context();
        ctx.emit_parallel_moves(moves);
        let state = simulate(&ctx.out);

        for (dest, src) in moves {
            let expected = match src {
                X86Operand::Reg(reg) => *reg,
                other => panic!("This helper only checks register sources, got {:?}", other),
            };
            assert_eq!(
                state.get(dest).copied().unwrap_or(*dest),
                expected,
                "{:?} should hold the original value of {:?}; emitted: {:?}",
                dest,
                expected,
                ctx.out
            );
        }
    }

    #[test]
    fn parallel_moves_drop_self_moves() {
        // Arrange
        let moves = [
            (X86Register::Rdi, X86Operand::Reg(X86Register::Rdi)),
            (X86Register::Rsi, X86Operand::Reg(X86Register::Rsi)),
        ];
        let mut ctx = test_context();

        // Act
        ctx.emit_parallel_moves(&moves);

        // Assert
        assert!(ctx.out.is_empty(), "no-op moves must not be emitted");
    }

    #[test]
    fn parallel_moves_order_chains_before_overwriting_sources() {
        // `rcx <- rdi` must precede `rdi <- rdx`, or RDI's value is lost.
        assert_moves_are_parallel(&[
            (X86Register::Rcx, X86Operand::Reg(X86Register::Rdi)),
            (X86Register::Rdi, X86Operand::Reg(X86Register::Rdx)),
        ]);
    }

    #[test]
    fn parallel_moves_handle_a_two_register_swap() {
        // A pure cycle: no ordering works, so it must be broken with a
        // scratch register.
        assert_moves_are_parallel(&[
            (X86Register::Rdi, X86Operand::Reg(X86Register::Rsi)),
            (X86Register::Rsi, X86Operand::Reg(X86Register::Rdi)),
        ]);
    }

    #[test]
    fn parallel_moves_handle_a_cycle_with_a_dangling_chain() {
        // A three-register rotation plus a move that feeds off it.
        assert_moves_are_parallel(&[
            (X86Register::Rdi, X86Operand::Reg(X86Register::Rsi)),
            (X86Register::Rsi, X86Operand::Reg(X86Register::Rdx)),
            (X86Register::Rdx, X86Operand::Reg(X86Register::Rdi)),
            (X86Register::R8, X86Operand::Reg(X86Register::Rdx)),
        ]);
    }

    #[test]
    fn parallel_moves_handle_the_full_argument_shuffle() {
        // The System V argument registers rotated by one — the worst case a
        // six-parameter function can hand the allocator.
        assert_moves_are_parallel(&[
            (X86Register::Rdi, X86Operand::Reg(X86Register::R9)),
            (X86Register::Rsi, X86Operand::Reg(X86Register::Rdi)),
            (X86Register::Rdx, X86Operand::Reg(X86Register::Rsi)),
            (X86Register::Rcx, X86Operand::Reg(X86Register::Rdx)),
            (X86Register::R8, X86Operand::Reg(X86Register::Rcx)),
            (X86Register::R9, X86Operand::Reg(X86Register::R8)),
        ]);
    }

    #[test]
    fn a_register_written_twice_keeps_only_the_last_write() {
        // Arrange: an unused parameter shares R8 with a live one, because
        // its live interval collapsed to its definition.
        let mut moves = vec![
            (X86Register::R8, X86Operand::Reg(X86Register::Rcx)),
            (X86Register::Rcx, X86Operand::Reg(X86Register::Rdi)),
            (X86Register::R8, X86Operand::Reg(X86Register::R8)),
        ];

        // Act
        keep_last_write_per_register(&mut moves);

        // Assert: the dead copy is gone, the rest keeps its order.
        assert_eq!(
            moves,
            vec![
                (X86Register::Rcx, X86Operand::Reg(X86Register::Rdi)),
                (X86Register::R8, X86Operand::Reg(X86Register::R8)),
            ]
        );
    }

    #[test]
    fn distinct_destinations_survive_deduplication() {
        // Arrange
        let original = vec![
            (X86Register::Rdi, X86Operand::Reg(X86Register::Rsi)),
            (X86Register::Rsi, X86Operand::Reg(X86Register::Rdx)),
        ];
        let mut moves = original.clone();

        // Act
        keep_last_write_per_register(&mut moves);

        // Assert
        assert_eq!(moves, original);
    }

    #[test]
    fn incoming_params_use_registers_then_the_caller_frame() {
        // Arrange / Act / Assert — first six in ABI registers ...
        assert_eq!(
            LoweringContext::incoming_param_location(0),
            X86Operand::Reg(X86Register::Rdi)
        );
        assert_eq!(
            LoweringContext::incoming_param_location(5),
            X86Operand::Reg(X86Register::R9)
        );
        // ... the rest just above the saved return address.
        assert_eq!(
            LoweringContext::incoming_param_location(6),
            X86Operand::Mem(X86Register::Rbp, 16)
        );
        assert_eq!(
            LoweringContext::incoming_param_location(7),
            X86Operand::Mem(X86Register::Rbp, 24)
        );
    }
}
