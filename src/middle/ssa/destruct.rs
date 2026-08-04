//! Leaving SSA form: back to the TAC control-flow graph the backend consumes.
//!
//! Values become TAC temporaries and slots become the variables they came
//! from, which is a straight rewrite.  Phi nodes are the part that needs care:
//! they describe a simultaneous assignment on each incoming edge, and turning
//! that into a sequence of moves is where naive implementations produce wrong
//! code.
//!
//! # Phi lowering
//!
//! Each phi group is lowered on the edge it belongs to in two passes: every
//! argument is copied into a fresh temporary first, and only then is each
//! temporary copied into the phi's destination.  Reading everything before
//! writing anything is what makes the classic hazards impossible --
//! `(a, b) := (b, a)` cannot lose a value to the copy that precedes it, and a
//! phi argument that is still live after the copy is never overwritten.
//!
//! It costs two moves per phi per edge where one is usually enough.  Phase 4
//! of the SSA migration replaces this with the parallel-copy sequentialisation
//! from Boissinot et al., *Revisiting Out-of-SSA Translation*, which uses one
//! move per phi plus one temporary per cycle; the fixtures that pin down the
//! difference belong to that phase.  Until then this stands, because it is
//! obviously correct and that is what the round trip has to be first.

use crate::middle::ir::{BasicBlock, CFG, Opcode, Operand as TacOperand, TACInstruction};

use super::{BinOp, BlockId, Function, Op, Operand, SlotOrigin, Terminator};

/// Translate a function in SSA form back into a TAC control-flow graph.
///
/// # Panics
///
/// Panics if a block with phi nodes has a predecessor that branches, which
/// means a critical edge survived and there is nowhere unambiguous to put the
/// copies the phis lower to.
pub fn to_cfg(function: &Function) -> CFG {
    let entry_label = function.block(function.entry()).label.clone();
    let mut cfg = CFG::new(entry_label, function.exit_label().to_string());

    for block in function.block_ids() {
        cfg.add_block(BasicBlock::new(function.block(block).label.clone()));
    }

    // Every `return` jumps to the exit block, so it has to exist even when the
    // SSA form had no block of its own for it.
    let exit = function.exit_label().to_string();
    if !cfg.blocks.contains_key(&exit) {
        cfg.add_block(BasicBlock::new(exit));
    }

    let mut lowering = Lowering {
        function,
        copies: 0,
    };
    for block in function.block_ids() {
        lowering.lower_block(&mut cfg, block);
    }

    cfg
}

/// The state of one function's translation back to TAC.
struct Lowering<'a> {
    function: &'a Function,
    /// Counter for the temporaries phi lowering introduces.
    copies: usize,
}

impl Lowering<'_> {
    /// Emit one block's instructions, the copies its outgoing edges owe to phi
    /// nodes, and its transfer.
    fn lower_block(&mut self, cfg: &mut CFG, block: BlockId) {
        // Rust note: `self.function` is a shared reference, so copying it into
        // a local detaches the borrow from `self` and lets the loop below call
        // methods that need `&mut self`.
        let function = self.function;
        let label = function.block(block).label.clone();

        let mut body = Vec::new();
        for &inst in &function.block(block).insts {
            self.lower_inst(&mut body, inst);
        }
        self.lower_outgoing_phis(&mut body, block);
        let edges = self.lower_terminator(&mut body, block);

        let target = cfg
            .blocks
            .get_mut(&label)
            .expect("Compiler Bug: block was just added");
        target.instructions = body;

        for successor in edges {
            cfg.add_edge(&label, &successor);
        }
    }

    /// Emit one instruction.
    fn lower_inst(&self, out: &mut Vec<TACInstruction>, inst: super::InstId) {
        let inst = self.function.inst(inst);
        let dest = inst.dest.map(|value| self.value(value));

        match &inst.op {
            Op::Binary(operator, lhs, rhs) => out.push(TACInstruction::new(
                binary_opcode(*operator),
                dest,
                Some(self.operand(*lhs)),
                Some(self.operand(*rhs)),
            )),

            Op::Copy(source) => out.push(TACInstruction::new(
                Opcode::Mov,
                dest,
                Some(self.operand(*source)),
                None,
            )),

            Op::Call { callee, args } => {
                for argument in args {
                    out.push(TACInstruction::new(
                        Opcode::Param,
                        None,
                        Some(self.operand(*argument)),
                        None,
                    ));
                }
                out.push(TACInstruction::new(
                    Opcode::Call,
                    dest,
                    Some(TacOperand::Label(callee.clone())),
                    Some(TacOperand::ImmInt(args.len() as i64)),
                ));
            }

            Op::GetParam(index) => out.push(TACInstruction::new(
                Opcode::GetParam,
                dest,
                Some(TacOperand::ImmInt(*index as i64)),
                None,
            )),

            // An undefined value is materialised as zero: the program is
            // reading a variable it never wrote, so any value is as correct as
            // any other, and a fixed one keeps the output reproducible.
            Op::Undef => out.push(TACInstruction::new(
                Opcode::Mov,
                dest,
                Some(TacOperand::ImmInt(0)),
                None,
            )),

            Op::SlotLoad { slot } => out.push(TACInstruction::new(
                Opcode::Mov,
                dest,
                Some(self.slot(*slot)),
                None,
            )),

            Op::SlotStore { slot, value } => out.push(TACInstruction::new(
                Opcode::Mov,
                Some(self.slot(*slot)),
                Some(self.operand(*value)),
                None,
            )),

            Op::ArrayLoad { base, index } => out.push(TACInstruction::new(
                Opcode::ArrayLoad,
                dest,
                Some(self.slot(*base)),
                Some(self.operand(*index)),
            )),

            Op::ArrayStore { base, index, value } => out.push(TACInstruction::new(
                Opcode::ArrayStore,
                Some(self.slot(*base)),
                Some(self.operand(*index)),
                Some(self.operand(*value)),
            )),

            Op::Load { address } => out.push(TACInstruction::new(
                Opcode::Load,
                dest,
                Some(self.operand(*address)),
                None,
            )),

            Op::Store { address, value } => out.push(TACInstruction::new(
                Opcode::Store,
                None,
                Some(self.operand(*address)),
                Some(self.operand(*value)),
            )),

            Op::AddrOf { slot } => out.push(TACInstruction::new(
                Opcode::AddrOf,
                dest,
                Some(self.slot(*slot)),
                None,
            )),
        }
    }

    /// Emit the copies this block owes to the phi nodes of its successors.
    ///
    /// Every argument is read into a temporary before any destination is
    /// written, which is what makes a group of phis behave as the simultaneous
    /// assignment it is meant to be.
    fn lower_outgoing_phis(&mut self, out: &mut Vec<TACInstruction>, block: BlockId) {
        let function = self.function;

        for successor in function.block(block).successors() {
            let phis = &function.block(successor).phis;
            if phis.is_empty() {
                continue;
            }

            assert_eq!(
                function.block(block).successors().count(),
                1,
                "Compiler Bug: critical edge from .{} into .{}, which has phi nodes",
                function.block(block).label,
                function.block(successor).label
            );

            let position = self.argument_position(successor, block);

            // Every argument is read first ...
            let mut temporaries = Vec::with_capacity(phis.len());
            for phi in phis {
                let temporary = self.fresh_copy();
                let source = self.operand(phi.args[position]);
                out.push(TACInstruction::new(
                    Opcode::Mov,
                    Some(temporary.clone()),
                    Some(source),
                    None,
                ));
                temporaries.push(temporary);
            }

            // ... and only then is any destination written.
            for (phi, temporary) in phis.iter().zip(temporaries) {
                let destination = self.value(phi.dest);
                out.push(TACInstruction::new(
                    Opcode::Mov,
                    Some(destination),
                    Some(temporary),
                    None,
                ));
            }
        }
    }

    /// Emit the block's transfer.
    ///
    /// # Returns
    ///
    /// The labels of the blocks it transfers to, in the order the edges are to
    /// be recorded.
    fn lower_terminator(&self, out: &mut Vec<TACInstruction>, block: BlockId) -> Vec<String> {
        match self.function.block(block).terminator() {
            Terminator::Jump(target) => {
                let label = self.label(*target);
                out.push(TACInstruction::new(
                    Opcode::Jump,
                    None,
                    Some(TacOperand::Label(label.clone())),
                    None,
                ));
                vec![label]
            }

            Terminator::Branch {
                cond,
                then_block,
                else_block,
            } => {
                let taken = self.label(*then_block);
                let untaken = self.label(*else_block);
                out.push(TACInstruction::new(
                    Opcode::BranchIf,
                    None,
                    Some(self.operand(*cond)),
                    Some(TacOperand::Label(taken.clone())),
                ));
                out.push(TACInstruction::new(
                    Opcode::Jump,
                    None,
                    Some(TacOperand::Label(untaken.clone())),
                    None,
                ));
                vec![taken, untaken]
            }

            Terminator::Return(value) => {
                out.push(TACInstruction::new(
                    Opcode::Ret,
                    None,
                    value.map(|value| self.operand(value)),
                    None,
                ));
                vec![self.function.exit_label().to_string()]
            }
        }
    }

    // ### Names ###

    /// Which argument of `successor`'s phi nodes arrives from `block`.
    fn argument_position(&self, successor: BlockId, block: BlockId) -> usize {
        self.function
            .block(successor)
            .preds()
            .iter()
            .position(|&pred| pred == block)
            .expect("Compiler Bug: phi arguments are aligned with the predecessor list")
    }

    /// The TAC operand a value is held in.
    fn value(&self, value: super::ValueId) -> TacOperand {
        TacOperand::Temp(format!("v{}", value.index()))
    }

    /// The TAC operand a slot lives in.
    fn slot(&self, slot: super::SlotId) -> TacOperand {
        match &self.function.slot(slot).origin {
            SlotOrigin::Variable(id) => TacOperand::Var(*id),
            SlotOrigin::Temporary(name) => TacOperand::Temp(name.clone()),
        }
    }

    /// The TAC operand an SSA operand reads.
    fn operand(&self, operand: Operand) -> TacOperand {
        match operand {
            Operand::Value(value) => self.value(value),
            Operand::Imm(constant) => TacOperand::ImmInt(constant),
        }
    }

    /// A block's label.
    fn label(&self, block: BlockId) -> String {
        self.function.block(block).label.clone()
    }

    /// A temporary for one phi copy.  The prefix keeps these apart from both
    /// the lowering's `t` temporaries and the `v` values.
    fn fresh_copy(&mut self) -> TacOperand {
        self.copies += 1;
        TacOperand::Temp(format!("phi{}", self.copies))
    }
}

/// The TAC opcode a binary operator lowers to.
fn binary_opcode(operator: BinOp) -> Opcode {
    match operator {
        BinOp::Add => Opcode::Add,
        BinOp::Sub => Opcode::Sub,
        BinOp::Mul => Opcode::Mul,
        BinOp::Div => Opcode::Div,
        BinOp::Eq => Opcode::Eq,
        BinOp::Neq => Opcode::Neq,
        BinOp::Lt => Opcode::Lt,
        BinOp::Lte => Opcode::Lte,
        BinOp::Gt => Opcode::Gt,
        BinOp::Gte => Opcode::Gte,
    }
}
