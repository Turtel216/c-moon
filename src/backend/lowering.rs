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

        for (instr, block_label) in instructions {
            // Emit a label when we enter a new basic block.
            if current_block != Some(block_label.as_str()) {
                current_block = Some(block_label.as_str());
                ctx.emit(X86Instruction::Label(format!(".{}", block_label)));
            }
            ctx.lower_instruction(instr);
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
            Opcode::GetParam => self.lower_get_param(instr),

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

        // Stack args go in reverse order (right-to-left).
        let stack_args = if args.len() > 6 { &args[6..] } else { &[] };
        for arg in stack_args.iter().rev() {
            let val = self.resolve(arg, SCRATCH1);
            let val = self.ensure_reg(val, SCRATCH1);
            self.emit(X86Instruction::Push(val));
        }

        // Register args.
        for (i, arg) in args.iter().enumerate().take(6) {
            let target_reg = PARAM_REGS[i];
            let val = self.resolve(arg, SCRATCH1);
            if val != X86Operand::Reg(target_reg) {
                self.emit(X86Instruction::Mov(X86Operand::Reg(target_reg), val));
            }
        }

        // Align stack to 16 bytes if we pushed an odd number of stack args.
        let stack_arg_count = stack_args.len();
        let needs_alignment = stack_arg_count % 2 != 0;
        if needs_alignment {
            self.emit(X86Instruction::Sub(
                X86Operand::Reg(X86Register::Rsp),
                X86Operand::Imm(8),
            ));
        }

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

    fn lower_get_param(&mut self, instr: &TACInstruction) {
        let dest = instr.dest.as_ref().unwrap();
        let index = match instr.arg1.as_ref().unwrap() {
            Operand::ImmInt(i) => *i as usize,
            _ => panic!("GetParam arg1 must be an immediate index"),
        };

        let dest_reg = self.dest_reg(dest);

        if index < PARAM_REGS.len() {
            let param_reg = PARAM_REGS[index];
            if dest_reg != param_reg {
                self.emit(X86Instruction::Mov(
                    X86Operand::Reg(dest_reg),
                    X86Operand::Reg(param_reg),
                ));
            }
        } else {
            // Parameters beyond the 6th are on the stack.
            // Layout: [rbp+16] = arg7, [rbp+24] = arg8, ...
            let stack_offset = 16 + ((index - 6) * 8) as i32;
            self.emit(X86Instruction::Mov(
                X86Operand::Reg(dest_reg),
                X86Operand::Mem(X86Register::Rbp, stack_offset),
            ));
        }

        self.store_if_spilled(dest, dest_reg);
        // If this param is address-taken, sync to its dedicated slot.
        self.sync_addr_taken_if_needed(dest, dest_reg);
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

    /// Resolve a TAC `Operand` into an `X86Operand`.
    /// If the operand is a spilled vreg, a `mov` into `scratch` is emitted
    /// and the scratch register is returned.
    fn resolve(&mut self, op: &Operand, scratch: X86Register) -> X86Operand {
        // For address-taken variables, always load from their dedicated
        // stack slot. This ensures pointer writes are visible when reading
        // the variable by name.
        if let Operand::Var(id) = op {
            if let Some(&offset) = self.addr_taken_offsets.get(id) {
                self.emit(X86Instruction::Mov(
                    X86Operand::Reg(scratch),
                    X86Operand::Mem(X86Register::Rbp, offset),
                ));
                return X86Operand::Reg(scratch);
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
                        let offset = self.spill_offset(*slot);
                        self.emit(X86Instruction::Mov(
                            X86Operand::Reg(scratch),
                            X86Operand::Mem(X86Register::Rbp, offset),
                        ));
                        X86Operand::Reg(scratch)
                    }
                    None => panic!("VirtualReg {} has no allocation", vreg),
                }
            }
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
        self.emit(X86Instruction::Mov(
            X86Operand::Mem(SCRATCH2, 0),
            val_op,
        ));
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

/// Internal helper enum to distinguish binary operations during lowering.
enum BinKind {
    Add,
    Sub,
    Mul,
}
