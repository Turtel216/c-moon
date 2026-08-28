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
//! A group of phi nodes describes one *parallel* assignment on each incoming
//! edge: every argument is read, and only then is every destination written.
//! Emitting the copies one after another in the order the phis happen to be
//! written is what produces the classic wrong code.  `(a, b) := (b, a)` turns
//! into `a := b; b := a`, which leaves both holding `b` -- the **swap
//! problem**.  A phi argument that is still live after the copy, overwritten
//! by an earlier copy of the same group, is the **lost-copy problem**.
//!
//! The sequentialisation here is Algorithm 1 of Boissinot et al., *Revisiting
//! Out-of-SSA Translation for Correctness, Code Quality, and Efficiency*: work
//! through the copies whose destination nothing else still needs, and when
//! only cycles are left, break one by saving a single value into a fresh
//! temporary.  That costs one move per phi plus one per cycle, against the two
//! per phi that reading everything into temporaries first would.
//!
//! What is *not* implemented is the rest of that paper: the interference
//! analysis and the coalescing that would let the copies be removed again.
//! Without coalescing every phi destination is a value of its own, and
//! correctness reduces exactly to sequentialising each parallel copy, which is
//! what this does.  Copies are placed at the end of the predecessor, and
//! critical edges are already split, so there is always a block where a copy
//! belongs to one edge alone.
//!
//! Coalescing is deferred deliberately.  It should not be added before the
//! copy count is shown to be a problem, and before checking what the backend's
//! register allocator already does with them.

use std::collections::{HashMap, HashSet};

use crate::middle::ir::{BasicBlock, CFG, Opcode, Operand as TacOperand, TACInstruction};

use super::{BinOp, BlockId, Function, Op, Operand, SlotOrigin, Terminator, ValueId};

/// One assignment of a parallel copy: `dest := source`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Copy {
    dest: ValueId,
    source: Operand,
}

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

    debug_assert_transfers(&cfg);
    cfg
}

/// Every block that carries instructions must end in an explicit transfer.
///
/// The backend emits blocks in an order of its own and has no notion of
/// falling through to the next one, so a block that simply runs off its end
/// continues into whichever block was laid out after it.  An empty block is
/// fine -- it contributes only its label, and control reaching that label
/// falls into the epilogue, which is what the exit block is for.
///
/// # Panics
///
/// Panics in a debug build if any block breaks this.
fn debug_assert_transfers(cfg: &CFG) {
    if !cfg!(debug_assertions) {
        return;
    }

    for (label, block) in &cfg.blocks {
        let Some(last) = block.instructions.last() else {
            continue;
        };
        assert!(
            matches!(
                last.opcode,
                Opcode::Jump | Opcode::BranchIf | Opcode::BranchIfNot | Opcode::Ret
            ),
            "Compiler Bug: block .{} ends in `{}` rather than a transfer, so control would \
             fall into whatever the backend lays out after it",
            label,
            last
        );
    }
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
            Op::Binary(operator, width, lhs, rhs) => out.push(TACInstruction::new(
                binary_opcode(*operator),
                *width,
                dest,
                Some(self.operand(*lhs)),
                Some(self.operand(*rhs)),
            )),

            // A copy moves a value between homes without looking at it, so
            // the whole register goes: see `TACInstruction::width`.
            Op::Copy(source) => out.push(TACInstruction::transfer(
                Opcode::Mov,
                dest,
                Some(self.operand(*source)),
                None,
            )),

            Op::Convert {
                from,
                sign,
                to,
                value,
            } => out.push(TACInstruction::new(
                Opcode::Convert {
                    from: *from,
                    sign: *sign,
                },
                *to,
                dest,
                Some(self.operand(*value)),
                None,
            )),

            Op::Call { callee, args } => {
                for argument in args {
                    out.push(TACInstruction::transfer(
                        Opcode::Param,
                        None,
                        Some(self.operand(*argument)),
                        None,
                    ));
                }
                out.push(TACInstruction::transfer(
                    Opcode::Call,
                    dest,
                    Some(TacOperand::Label(callee.clone())),
                    Some(TacOperand::ImmInt(args.len() as i64)),
                ));
            }

            Op::GetParam(index) => out.push(TACInstruction::transfer(
                Opcode::GetParam,
                dest,
                Some(TacOperand::ImmInt(*index as i64)),
                None,
            )),

            // An undefined value is materialised as zero: the program is
            // reading a variable it never wrote, so any value is as correct as
            // any other, and a fixed one keeps the output reproducible.
            Op::Undef => out.push(TACInstruction::transfer(
                Opcode::Mov,
                dest,
                Some(TacOperand::ImmInt(0)),
                None,
            )),

            Op::SlotLoad { slot, width } => out.push(TACInstruction::new(
                Opcode::Mov,
                *width,
                dest,
                Some(self.slot(*slot)),
                None,
            )),

            Op::SlotStore { slot, value, width } => out.push(TACInstruction::new(
                Opcode::Mov,
                *width,
                Some(self.slot(*slot)),
                Some(self.operand(*value)),
                None,
            )),

            Op::ArrayLoad { base, index, width } => out.push(TACInstruction::new(
                Opcode::ArrayLoad,
                *width,
                dest,
                Some(self.slot(*base)),
                Some(self.operand(*index)),
            )),

            Op::ArrayAddr { base, index, width } => out.push(TACInstruction::new(
                Opcode::ArrayAddr,
                *width,
                dest,
                Some(self.slot(*base)),
                Some(self.operand(*index)),
            )),

            Op::ArrayStore {
                base,
                index,
                value,
                width,
            } => out.push(TACInstruction::new(
                Opcode::ArrayStore,
                *width,
                Some(self.slot(*base)),
                Some(self.operand(*index)),
                Some(self.operand(*value)),
            )),

            Op::Load { address, width } => out.push(TACInstruction::new(
                Opcode::Load,
                *width,
                dest,
                Some(self.operand(*address)),
                None,
            )),

            Op::Store {
                address,
                value,
                width,
            } => out.push(TACInstruction::new(
                Opcode::Store,
                *width,
                None,
                Some(self.operand(*address)),
                Some(self.operand(*value)),
            )),

            // An address is a full machine word whatever it points at.
            Op::AddrOf { slot } => out.push(TACInstruction::transfer(
                Opcode::AddrOf,
                dest,
                Some(self.slot(*slot)),
                None,
            )),
        }
    }

    /// Emit the copies this block owes to the phi nodes of its successors.
    ///
    /// The phis of one successor form a single parallel assignment on this
    /// edge, which [`Lowering::sequentialize`] turns into moves.
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
            let copies: Vec<Copy> = phis
                .iter()
                .map(|phi| Copy {
                    dest: phi.dest,
                    source: phi.args[position],
                })
                // A phi taking its own result along this edge -- what a loop
                // produces for a variable it does not assign -- asks for a
                // move from a value to itself, which is no assignment at all.
                .filter(|copy| copy.source != Operand::Value(copy.dest))
                .collect();

            self.sequentialize(&copies, out);
        }
    }

    /// Turn one parallel assignment into a sequence of moves.
    ///
    /// Boissinot et al., Algorithm 1.  Two facts drive it:
    ///
    /// - A copy may be emitted as soon as nothing else still needs what its
    ///   destination currently holds.  Emitting it frees its *source* to be
    ///   overwritten in turn, so one copy can make another ready.
    /// - When nothing is ready, every remaining copy is on a cycle.  Saving
    ///   one of their destinations into a temporary breaks that cycle and
    ///   makes it ready, and the cycle unwinds from there.
    ///
    /// `location` is what makes the second part work: it records where the
    /// value that started in each place currently lives, so a copy reading a
    /// value that has since been moved reads it from wherever it went.
    fn sequentialize(&mut self, copies: &[Copy], out: &mut Vec<TACInstruction>) {
        let mut source_of: HashMap<ValueId, Operand> = HashMap::new();
        let mut location: HashMap<ValueId, TacOperand> = HashMap::new();
        let mut pending: HashSet<ValueId> = HashSet::new();

        for copy in copies {
            source_of.insert(copy.dest, copy.source);
            pending.insert(copy.dest);
            // A value that is read starts out where it was defined.
            if let Operand::Value(source) = copy.source {
                location.insert(source, self.value(source));
            }
        }

        // A destination nothing reads can be written straight away.
        let mut ready: Vec<ValueId> = copies
            .iter()
            .map(|copy| copy.dest)
            .filter(|dest| !location.contains_key(dest))
            .collect();
        let mut to_do: Vec<ValueId> = copies.iter().map(|copy| copy.dest).collect();

        loop {
            while let Some(dest) = ready.pop() {
                let source = source_of[&dest];
                let held_in = match source {
                    Operand::Value(value) => location[&value].clone(),
                    Operand::Imm(constant) => TacOperand::ImmInt(constant),
                };

                out.push(TACInstruction::transfer(
                    Opcode::Mov,
                    Some(self.value(dest)),
                    Some(held_in.clone()),
                    None,
                ));
                pending.remove(&dest);

                if let Operand::Value(value) = source {
                    let at_home = held_in == self.value(value);
                    location.insert(value, self.value(dest));
                    // The source has been read out of its own place, so
                    // whatever was waiting to overwrite it may now do so.
                    if at_home && pending.contains(&value) {
                        ready.push(value);
                    }
                }
            }

            let Some(dest) = to_do.pop() else { break };
            if !pending.contains(&dest) {
                continue;
            }

            // Nothing is ready and this copy is still outstanding, so it is on
            // a cycle. One temporary breaks it, and the rest of the cycle
            // follows from the ready list.
            let temporary = self.fresh_copy();
            out.push(TACInstruction::transfer(
                Opcode::Mov,
                Some(temporary.clone()),
                Some(self.value(dest)),
                None,
            ));
            location.insert(dest, temporary);
            ready.push(dest);
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
                out.push(TACInstruction::transfer(
                    Opcode::Jump,
                    None,
                    Some(TacOperand::Label(label.clone())),
                    None,
                ));
                vec![label]
            }

            Terminator::Branch {
                cond,
                width,
                then_block,
                else_block,
            } => {
                let taken = self.label(*then_block);
                let untaken = self.label(*else_block);
                out.push(TACInstruction::new(
                    Opcode::BranchIf,
                    *width,
                    None,
                    Some(self.operand(*cond)),
                    Some(TacOperand::Label(taken.clone())),
                ));
                out.push(TACInstruction::transfer(
                    Opcode::Jump,
                    None,
                    Some(TacOperand::Label(untaken.clone())),
                    None,
                ));
                vec![taken, untaken]
            }

            Terminator::Return(value) => {
                out.push(TACInstruction::transfer(
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
        BinOp::Div(sign) => Opcode::Div(sign),
        BinOp::And => Opcode::And,
        BinOp::Or => Opcode::Or,
        BinOp::Xor => Opcode::Xor,
        BinOp::Shl => Opcode::Shl,
        BinOp::Shr(sign) => Opcode::Shr(sign),
        BinOp::Eq => Opcode::Eq,
        BinOp::Neq => Opcode::Neq,
        BinOp::Lt(sign) => Opcode::Lt(sign),
        BinOp::Lte(sign) => Opcode::Lte(sign),
        BinOp::Gt(sign) => Opcode::Gt(sign),
        BinOp::Gte(sign) => Opcode::Gte(sign),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::middle::ir::Width;

    use crate::middle::ssa::verify::verify_ssa;
    use crate::middle::ssa::{BinOp, SlotOrigin};

    /// The instructions of one block of the emitted TAC, as text.
    fn emitted(cfg: &CFG, label: &str) -> Vec<String> {
        cfg.blocks[label]
            .instructions
            .iter()
            .map(TACInstruction::to_string)
            .collect()
    }

    /// A loop whose header carries `phi_count` phi nodes, with the arguments
    /// on the back edge chosen by the caller.
    ///
    /// ```text
    /// entry:  <initial values>            done:  ret <first phi>
    ///         jmp header                  latch: <body>
    /// header: <phis>                             jmp header
    ///         br .latch : .done
    /// ```
    struct Loop {
        function: Function,
        header: BlockId,
        latch: BlockId,
        done: BlockId,
    }

    impl Loop {
        /// A loop with `initial` values computed in the entry block.
        fn new(initial: &[i64]) -> (Self, Vec<ValueId>) {
            let mut function =
                Function::new("f".to_string(), "entry".to_string(), "exit".to_string());
            let entry = function.entry();
            let header = function.add_block("header".to_string());
            let latch = function.add_block("latch".to_string());
            let done = function.add_block("done".to_string());

            let values: Vec<ValueId> = initial
                .iter()
                .map(|&constant| {
                    function
                        .emit(entry, Op::Copy(Operand::Imm(constant)))
                        .expect("a copy defines a value")
                })
                .collect();

            // Every edge is in place before any phi exists, so that each phi
            // is created with one argument per predecessor.
            function.set_terminator(entry, Terminator::Jump(header));
            function.set_terminator(latch, Terminator::Jump(header));
            function.set_terminator(
                header,
                Terminator::Branch {
                    cond: Operand::Imm(1),
                    then_block: latch,
                    else_block: done,
                    width: Width::Bits64,
                },
            );

            (
                Self {
                    function,
                    header,
                    latch,
                    done,
                },
                values,
            )
        }

        /// Add a phi to the header taking `from_entry` on the way in.
        fn phi(&mut self, variable: usize, from_entry: Operand) -> ValueId {
            let slot = self.function.slot_for(SlotOrigin::Variable(variable));
            let dest = self.function.add_phi(self.header, slot);
            let position = self.function.block(self.header).phis.len() - 1;
            self.function.block_mut(self.header).phis[position].args[0] = from_entry;
            dest
        }

        /// Set what the phi at `position` takes along the back edge.
        fn back_edge(&mut self, position: usize, value: Operand) {
            self.function.block_mut(self.header).phis[position].args[1] = value;
        }

        /// Finish the function, returning `result`, and lower it to TAC.
        fn lower(mut self, result: ValueId) -> CFG {
            self.function
                .set_terminator(self.done, Terminator::Return(Some(Operand::Value(result))));
            assert_eq!(verify_ssa(&self.function), Ok(()), "{}", self.function);
            to_cfg(&self.function)
        }
    }

    #[test]
    fn a_pair_of_phis_that_exchange_their_values_needs_a_temporary() {
        // Arrange: `a, b = b, a` on the back edge -- the swap problem. Lowered
        // in the order the phis are written it would be `a := b; b := a`,
        // leaving both holding the old `b`.
        let (mut fixture, initial) = Loop::new(&[1, 2]);
        let first = fixture.phi(0, Operand::Value(initial[0]));
        let second = fixture.phi(1, Operand::Value(initial[1]));
        fixture.back_edge(0, Operand::Value(second));
        fixture.back_edge(1, Operand::Value(first));

        // Act
        let cfg = fixture.lower(first);

        // Assert: one value is saved, the cycle unwinds through it, and the
        // saved value lands in the destination the cycle started from.
        assert_eq!(
            emitted(&cfg, "latch"),
            vec![
                "%phi1 =.64 %v3".to_string(),
                "%v3 =.64 %v2".to_string(),
                "%v2 =.64 %phi1".to_string(),
                "jmp .header".to_string(),
            ]
        );
    }

    #[test]
    fn a_chain_of_copies_is_ordered_so_that_nothing_reads_a_clobbered_value() {
        // Arrange: `(a, c) := (v, a)` -- no cycle, but `c := a` has to happen
        // before `a := v` overwrites what it reads.
        let (mut fixture, initial) = Loop::new(&[1]);
        let first = fixture.phi(0, Operand::Value(initial[0]));
        let second = fixture.phi(1, Operand::Value(initial[0]));
        let stepped = fixture
            .function
            .emit(
                fixture.latch,
                Op::Binary(
                    BinOp::Add,
                    Width::Bits64,
                    Operand::Value(first),
                    Operand::Imm(1),
                ),
            )
            .expect("an addition defines a value");
        fixture.back_edge(0, Operand::Value(stepped));
        fixture.back_edge(1, Operand::Value(first));

        // Act
        let cfg = fixture.lower(second);

        // Assert: the reader goes first, and no temporary is needed.
        assert_eq!(
            emitted(&cfg, "latch"),
            vec![
                "%v3 = %v1 +.64 1".to_string(),
                "%v2 =.64 %v1".to_string(),
                "%v1 =.64 %v3".to_string(),
                "jmp .header".to_string(),
            ]
        );
    }

    #[test]
    fn a_phi_that_takes_its_own_result_costs_nothing() {
        // Arrange: what a loop produces for a variable it never assigns.
        let (mut fixture, initial) = Loop::new(&[7]);
        let carried = fixture.phi(0, Operand::Value(initial[0]));
        fixture.back_edge(0, Operand::Value(carried));

        // Act
        let cfg = fixture.lower(carried);

        // Assert: a move from a value to itself is not an assignment, and
        // sequentialising it would have cost a temporary and two moves.
        assert_eq!(emitted(&cfg, "latch"), vec!["jmp .header".to_string()]);
    }

    #[test]
    fn a_phi_that_is_live_after_the_loop_is_not_overwritten_on_the_way_out() {
        // Arrange: the lost-copy shape. The phi's value is read after the
        // loop, and its argument is computed inside the loop, so a copy placed
        // anywhere but the end of the back edge destroys one or the other.
        let (mut fixture, initial) = Loop::new(&[1]);
        let carried = fixture.phi(0, Operand::Value(initial[0]));
        let stepped = fixture
            .function
            .emit(
                fixture.latch,
                Op::Binary(
                    BinOp::Add,
                    Width::Bits64,
                    Operand::Value(carried),
                    Operand::Imm(1),
                ),
            )
            .expect("an addition defines a value");
        fixture.back_edge(0, Operand::Value(stepped));

        // Act
        let cfg = fixture.lower(carried);

        // Assert: the copy is on the back edge, after everything the block
        // computes ...
        assert_eq!(
            emitted(&cfg, "latch"),
            vec![
                "%v2 = %v1 +.64 1".to_string(),
                "%v1 =.64 %v2".to_string(),
                "jmp .header".to_string(),
            ]
        );

        // ... and the exit path leaves the value alone, which is the whole
        // point: what the loop last computed is what is read afterwards.
        assert_eq!(emitted(&cfg, "done"), vec!["ret %v1".to_string()]);
    }
}
