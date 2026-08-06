//! Building an SSA [`Function`] from the TAC control-flow graph.
//!
//! The translation happens in two steps.  First every variable and temporary
//! becomes a [`Slot`](super::Slot) -- a memory location read and written
//! through explicit loads and stores -- which is a faithful, if verbose,
//! rendering of the TAC.  Promotion then turns the slots that qualify into SSA
//! values.  Splitting it this way means the first step needs no analysis at
//! all and cannot be wrong, and the second is the standard construction
//! algorithm working on loads and stores, which is where it is at its most
//! testable.
//!
//! Edges are taken from the branch instructions, not from the CFG's own
//! successor and predecessor lists: in the TAC those are maintained by hand
//! alongside the instructions and can drift apart from them.  Here the
//! terminator is the only source of truth.

use std::collections::HashMap;

use crate::middle::ir::{self, CFG, Opcode, TACInstruction};

use super::{BinOp, BlockId, Function, Op, Operand, SlotOrigin, Terminator};

/// Translate one TAC function into SSA form.
///
/// # Arguments
///
/// * `name` - the function's symbol name
/// * `cfg` - its control-flow graph
///
/// # Panics
///
/// Panics on TAC the lowering cannot produce -- a block whose tail is not a
/// recognised transfer, a branch to a label that is not a block, a `Param`
/// with no `Call` to consume it.  Each of those means a bug in an earlier
/// pass rather than a bad program.
pub fn build(name: &str, cfg: &CFG) -> Function {
    let mut builder = Builder::new(name, cfg);
    builder.translate(cfg);

    let mut function = builder.function;
    // Dominance is undefined for a block nothing reaches, and every `return`
    // in the source leaves one behind, so they go before anything looks at
    // the graph shape.
    function.retain_reachable();
    split_critical_edges(&mut function);
    function
}

/// Break up every critical edge, so that each one has a block of its own.
///
/// An edge is *critical* when it leaves a block that branches and arrives at a
/// block that is branched into from elsewhere as well.  Such an edge has
/// nowhere to put code that must run on it and on no other path -- which is
/// exactly what a phi node is -- so leaving out a block for it makes phi
/// nodes impossible to lower correctly.
///
/// The blocks this adds contain nothing but a jump.  From here on the
/// control-flow graph is kept free of critical edges, and the verifier checks
/// it.
pub fn split_critical_edges(function: &mut Function) {
    // Collected before anything is split: splitting adds blocks, and a block
    // added for one edge can never be on a critical edge itself.
    let mut critical = Vec::new();
    for block in function.block_ids() {
        let successors: Vec<BlockId> = function.block(block).successors().collect();
        if successors.len() < 2 {
            continue;
        }
        for successor in successors {
            if function.block(successor).preds().len() > 1 {
                critical.push((block, successor));
            }
        }
    }

    for (from, to) in critical {
        function.split_edge(from, to);
    }
}

/// The state of one function's translation.
struct Builder {
    function: Function,
    /// TAC label to the block it became.
    blocks: HashMap<String, BlockId>,
}

impl Builder {
    /// Create every block of `cfg`, so that branches have something to point
    /// at before any instruction is translated.
    fn new(name: &str, cfg: &CFG) -> Self {
        let mut function = Function::new(name.to_string(), cfg.entry.clone(), cfg.exit.clone());
        let mut blocks = HashMap::with_capacity(cfg.blocks.len());
        blocks.insert(cfg.entry.clone(), function.entry());

        for label in cfg.blocks.keys() {
            if *label == cfg.entry {
                continue;
            }
            blocks.insert(label.clone(), function.add_block(label.clone()));
        }

        Self { function, blocks }
    }

    /// Translate every block's instructions and work out how it ends.
    fn translate(&mut self, cfg: &CFG) {
        for (label, block) in &cfg.blocks {
            let id = self.block(label);
            let tail = self.translate_body(id, &block.instructions);
            let terminator = self.translate_terminator(id, tail);
            self.function.set_terminator(id, terminator);
        }
    }

    /// Translate the instructions up to the block's trailing transfer.
    ///
    /// # Returns
    ///
    /// The instructions that make up that transfer, which is what
    /// [`Builder::translate_terminator`] is given.
    fn translate_body<'a>(
        &mut self,
        block: BlockId,
        body: &'a [TACInstruction],
    ) -> &'a [TACInstruction] {
        // Arguments are buffered until the call that consumes them, exactly as
        // the backend does it; fusing the two removes the requirement that
        // they stay adjacent.
        let mut pending: Vec<Operand> = Vec::new();

        let prologue = self.translate_parameter_prologue(block, body);

        for (position, instr) in body.iter().enumerate().skip(prologue) {
            if is_transfer(&instr.opcode) {
                assert!(
                    pending.is_empty(),
                    "Compiler Bug: arguments passed to no call in .{}",
                    self.function.block(block).label
                );
                return &body[position..];
            }
            self.translate_instruction(block, instr, &mut pending);
        }

        assert!(
            pending.is_empty(),
            "Compiler Bug: arguments passed to no call in .{}",
            self.function.block(block).label
        );
        &[]
    }

    /// Translate the opening run of `get_param` instructions as one group.
    ///
    /// The backend lowers such a run as a single simultaneous assignment,
    /// because each of them reads an incoming argument register that a later
    /// one's destination may overwrite.  Emitting all the reads before any of
    /// the stores that place them is what keeps the run unbroken.
    ///
    /// # Returns
    ///
    /// How many instructions were consumed.
    fn translate_parameter_prologue(&mut self, block: BlockId, body: &[TACInstruction]) -> usize {
        let run = body
            .iter()
            .take_while(|instr| instr.opcode == Opcode::GetParam)
            .count();

        let mut placed = Vec::with_capacity(run);
        for instr in &body[..run] {
            let ir::Operand::ImmInt(index) = arg(instr, &instr.arg1) else {
                panic!("Compiler Bug: get_param needs an argument position");
            };
            let value = self
                .function
                .emit(block, Op::GetParam(*index as usize))
                .expect("Compiler Bug: get_param defines a value");
            let slot = self.slot(arg(instr, &instr.dest));
            placed.push((slot, value));
        }

        for (slot, value) in placed {
            self.function.emit(
                block,
                Op::SlotStore {
                    slot,
                    value: Operand::Value(value),
                },
            );
        }

        run
    }

    /// Translate one non-transfer instruction.
    fn translate_instruction(
        &mut self,
        block: BlockId,
        instr: &TACInstruction,
        pending: &mut Vec<Operand>,
    ) {
        match instr.opcode {
            Opcode::Add
            | Opcode::Sub
            | Opcode::Mul
            | Opcode::Div
            | Opcode::Eq
            | Opcode::Neq
            | Opcode::Lt
            | Opcode::Lte
            | Opcode::Gt
            | Opcode::Gte => {
                let operator = binary_operator(&instr.opcode);
                let lhs = self.operand(block, arg(instr, &instr.arg1));
                let rhs = self.operand(block, arg(instr, &instr.arg2));
                self.define(block, instr, Op::Binary(operator, lhs, rhs));
            }

            Opcode::Mov => {
                let source = self.operand(block, arg(instr, &instr.arg1));
                self.define(block, instr, Op::Copy(source));
            }

            Opcode::Param => {
                let argument = self.operand(block, arg(instr, &instr.arg1));
                pending.push(argument);
            }

            Opcode::Call => {
                let ir::Operand::Label(callee) = arg(instr, &instr.arg1) else {
                    panic!("Compiler Bug: a call needs a callee label, got {:?}", instr);
                };
                let args: Vec<Operand> = pending.drain(..).collect();
                if let Some(ir::Operand::ImmInt(count)) = instr.arg2 {
                    assert_eq!(
                        args.len(),
                        count as usize,
                        "Compiler Bug: call to {} passed {} arguments but declared {}",
                        callee,
                        args.len(),
                        count
                    );
                }
                let callee = callee.clone();
                self.define(block, instr, Op::Call { callee, args });
            }

            // Only reachable for a `get_param` that is not part of the
            // block's opening run, which the backend cannot lower.
            Opcode::GetParam => panic!(
                "Compiler Bug: get_param away from the top of a block: {:?}",
                instr
            ),

            Opcode::ArrayLoad => {
                let base = self.slot(arg(instr, &instr.arg1));
                let index = self.operand(block, arg(instr, &instr.arg2));
                self.define(block, instr, Op::ArrayLoad { base, index });
            }

            Opcode::ArrayStore => {
                // `dest` names the array written into, which is an input.
                let base = self.slot(arg(instr, &instr.dest));
                let index = self.operand(block, arg(instr, &instr.arg1));
                let value = self.operand(block, arg(instr, &instr.arg2));
                self.function
                    .emit(block, Op::ArrayStore { base, index, value });
            }

            Opcode::Load => {
                let address = self.operand(block, arg(instr, &instr.arg1));
                self.define(block, instr, Op::Load { address });
            }

            Opcode::Store => {
                let address = self.operand(block, arg(instr, &instr.arg1));
                let value = self.operand(block, arg(instr, &instr.arg2));
                self.function.emit(block, Op::Store { address, value });
            }

            Opcode::AddrOf => {
                let slot = self.slot(arg(instr, &instr.arg1));
                self.define(block, instr, Op::AddrOf { slot });
            }

            Opcode::Jump | Opcode::BranchIf | Opcode::BranchIfNot | Opcode::Ret => {
                unreachable!("transfers are handled by translate_terminator")
            }
        }
    }

    /// Work out how a block ends from the instructions left over after its
    /// body.
    ///
    /// The TAC has no single terminator: a conditional branch is a `br_if`
    /// followed by the `jmp` that is taken when it is not, and a block that
    /// falls off the end of the function has nothing at all.
    fn translate_terminator(&mut self, block: BlockId, tail: &[TACInstruction]) -> Terminator {
        match tail {
            // Only reachable once the optimiser has merged the exit block into
            // its predecessor, which leaves the merged block with no transfer.
            [] => Terminator::Return(None),

            [jump] if jump.opcode == Opcode::Jump => Terminator::Jump(self.target(jump)),

            [ret] if ret.opcode == Opcode::Ret => {
                let value = ret.arg1.as_ref().map(|value| self.operand(block, value));
                Terminator::Return(value)
            }

            [branch, jump]
                if matches!(branch.opcode, Opcode::BranchIf | Opcode::BranchIfNot)
                    && jump.opcode == Opcode::Jump =>
            {
                let cond = self.operand(block, arg(branch, &branch.arg1));
                let branched = self.target_of(branch, &branch.arg2);
                let fallthrough = self.target(jump);

                // `br_if_not` jumps when the condition is false, so the two
                // targets swap round.
                let (then_block, else_block) = match branch.opcode {
                    Opcode::BranchIf => (branched, fallthrough),
                    _ => (fallthrough, branched),
                };

                // Both arms leading to the same block is a jump, and saying so
                // keeps the pair of edges out of the rest of the compiler.
                if then_block == else_block {
                    Terminator::Jump(then_block)
                } else {
                    Terminator::Branch {
                        cond,
                        then_block,
                        else_block,
                    }
                }
            }

            _ => panic!(
                "Compiler Bug: block .{} ends in a transfer this pass does not recognise: {:?}",
                self.function.block(block).label,
                tail.iter().map(|instr| &instr.opcode).collect::<Vec<_>>()
            ),
        }
    }

    // ### Operands ###

    /// Translate an instruction that produces a value, and store the result
    /// into the slot its destination names.
    fn define(&mut self, block: BlockId, instr: &TACInstruction, op: Op) {
        let value = self
            .function
            .emit(block, op)
            .expect("Compiler Bug: an instruction with a destination defines a value");
        let slot = self.slot(arg(instr, &instr.dest));
        self.function.emit(
            block,
            Op::SlotStore {
                slot,
                value: Operand::Value(value),
            },
        );
    }

    /// Translate a TAC operand into something an instruction can read,
    /// emitting the load a variable needs.
    fn operand(&mut self, block: BlockId, operand: &ir::Operand) -> Operand {
        match operand {
            ir::Operand::ImmInt(value) => Operand::Imm(*value),
            ir::Operand::Var(_) | ir::Operand::Temp(_) => {
                let slot = self.slot(operand);
                let value = self
                    .function
                    .emit(block, Op::SlotLoad { slot })
                    .expect("Compiler Bug: a load defines a value");
                Operand::Value(value)
            }
            ir::Operand::Label(label) => {
                panic!(
                    "Compiler Bug: label .{} used where a value is needed",
                    label
                )
            }
        }
    }

    /// The slot a TAC variable or temporary lives in.
    fn slot(&mut self, operand: &ir::Operand) -> super::SlotId {
        let origin = match operand {
            ir::Operand::Var(id) => SlotOrigin::Variable(*id),
            ir::Operand::Temp(name) => SlotOrigin::Temporary(name.clone()),
            other => panic!("Compiler Bug: {:?} does not name a storage location", other),
        };
        self.function.slot_for(origin)
    }

    /// The block a jump's target label names.
    fn target(&self, jump: &TACInstruction) -> BlockId {
        self.target_of(jump, &jump.arg1)
    }

    /// The block the label in `field` names.
    fn target_of(&self, instr: &TACInstruction, field: &Option<ir::Operand>) -> BlockId {
        let ir::Operand::Label(label) = arg(instr, field) else {
            panic!(
                "Compiler Bug: a transfer needs a target label, got {:?}",
                instr
            );
        };
        self.block(label)
    }

    /// The block a label names.
    fn block(&self, label: &str) -> BlockId {
        *self
            .blocks
            .get(label)
            .unwrap_or_else(|| panic!("Compiler Bug: branch to unknown block .{}", label))
    }
}

/// Read an operand field that the opcode requires to be present.
fn arg<'a>(instr: &'a TACInstruction, field: &'a Option<ir::Operand>) -> &'a ir::Operand {
    field
        .as_ref()
        .unwrap_or_else(|| panic!("Compiler Bug: {:?} is missing an operand", instr.opcode))
}

/// Does this opcode end a basic block?
fn is_transfer(opcode: &Opcode) -> bool {
    matches!(
        opcode,
        Opcode::Jump | Opcode::BranchIf | Opcode::BranchIfNot | Opcode::Ret
    )
}

/// The SSA operator a two-operand TAC opcode stands for.
fn binary_operator(opcode: &Opcode) -> BinOp {
    match opcode {
        Opcode::Add => BinOp::Add,
        Opcode::Sub => BinOp::Sub,
        Opcode::Mul => BinOp::Mul,
        Opcode::Div => BinOp::Div,
        Opcode::Eq => BinOp::Eq,
        Opcode::Neq => BinOp::Neq,
        Opcode::Lt => BinOp::Lt,
        Opcode::Lte => BinOp::Lte,
        Opcode::Gt => BinOp::Gt,
        Opcode::Gte => BinOp::Gte,
        other => panic!("Compiler Bug: {:?} is not a binary operation", other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::HashMap;

    use crate::middle::ir::{BasicBlock, Operand as TacOperand};
    use crate::middle::ssa::destruct::to_cfg;
    use crate::middle::ssa::mem2reg::promote_slots;
    use crate::middle::ssa::promote::promotable;
    use crate::middle::ssa::verify::verify_ssa;

    /// A TAC control-flow graph made of the given blocks, in order.
    ///
    /// Edges are deliberately not added: construction takes them from the
    /// branch instructions, and leaving the CFG's own lists empty proves it.
    fn cfg(blocks: &[(&str, Vec<TACInstruction>)]) -> CFG {
        let mut cfg = CFG::new("entry".to_string(), "exit".to_string());
        for (label, instructions) in blocks {
            let mut block = BasicBlock::new((*label).to_string());
            block.instructions = instructions.clone();
            cfg.add_block(block);
        }
        cfg
    }

    fn var(id: usize) -> TacOperand {
        TacOperand::Var(id)
    }

    fn temp(name: &str) -> TacOperand {
        TacOperand::Temp(name.to_string())
    }

    fn label(name: &str) -> TacOperand {
        TacOperand::Label(name.to_string())
    }

    fn instr(
        opcode: Opcode,
        dest: Option<TacOperand>,
        arg1: Option<TacOperand>,
        arg2: Option<TacOperand>,
    ) -> TACInstruction {
        TACInstruction::new(opcode, dest, arg1, arg2)
    }

    fn mov(dest: TacOperand, source: TacOperand) -> TACInstruction {
        instr(Opcode::Mov, Some(dest), Some(source), None)
    }

    fn jump(target: &str) -> TACInstruction {
        instr(Opcode::Jump, None, Some(label(target)), None)
    }

    /// The function built from `blocks`, verified.
    fn build_verified(blocks: &[(&str, Vec<TACInstruction>)]) -> Function {
        let function = build("f", &cfg(blocks));
        verified(function)
    }

    /// The function built from `blocks` with every eligible variable promoted
    /// to SSA values, verified.
    fn promote_verified(
        blocks: &[(&str, Vec<TACInstruction>)],
        array_sizes: &[(usize, usize)],
    ) -> Function {
        let mut function = build_verified(blocks);
        let sizes: HashMap<usize, usize> = array_sizes.iter().copied().collect();
        let eligible = promotable(&function, &sizes);
        promote_slots(&mut function, &eligible);
        verified(function)
    }

    fn verified(function: Function) -> Function {
        if let Err(errors) = verify_ssa(&function) {
            panic!("verification failed: {:?}\n{}", errors, function);
        }
        function
    }

    /// The blocks of an `if`/`else` whose arms both assign to variable 0.
    fn if_else(
        consequent: Vec<TACInstruction>,
        alternative: Vec<TACInstruction>,
    ) -> Vec<(&'static str, Vec<TACInstruction>)> {
        let mut consequent = consequent;
        let mut alternative = alternative;
        consequent.push(jump("join"));
        alternative.push(jump("join"));
        vec![
            (
                "entry",
                vec![
                    mov(temp("t1"), TacOperand::ImmInt(1)),
                    instr(
                        Opcode::BranchIfNot,
                        None,
                        Some(temp("t1")),
                        Some(label("otherwise")),
                    ),
                    jump("consequent"),
                ],
            ),
            ("consequent", consequent),
            ("otherwise", alternative),
            ("join", vec![instr(Opcode::Ret, None, Some(var(0)), None)]),
        ]
    }

    #[test]
    fn every_variable_starts_out_as_a_slot() {
        // Arrange: `r0 = 1; t1 = r0 + 2; return t1`.
        let function = build_verified(&[
            (
                "entry",
                vec![
                    mov(var(0), TacOperand::ImmInt(1)),
                    instr(
                        Opcode::Add,
                        Some(temp("t1")),
                        Some(var(0)),
                        Some(TacOperand::ImmInt(2)),
                    ),
                    instr(Opcode::Ret, None, Some(temp("t1")), None),
                ],
            ),
            ("exit", vec![]),
        ]);

        // Assert: every read is a load and every write a store, which is what
        // makes the first step of construction free of any judgement.
        assert_eq!(
            function.to_string(),
            "\
function f {
.entry:
    %v0 = 1
    store_slot %r0, %v0
    %v1 = load_slot %r0
    %v2 = %v1 + 2
    store_slot %t1, %v2
    %v3 = load_slot %t1
    ret %v3
}
"
        );
    }

    #[test]
    fn a_branch_and_its_fallthrough_become_one_terminator() {
        // Arrange: `br_if_not t1 goto else; jmp then` -- the pair the lowering
        // emits for every `if`.
        let function = build_verified(&[
            (
                "entry",
                vec![
                    mov(temp("t1"), TacOperand::ImmInt(1)),
                    instr(
                        Opcode::BranchIfNot,
                        None,
                        Some(temp("t1")),
                        Some(label("otherwise")),
                    ),
                    jump("consequent"),
                ],
            ),
            ("consequent", vec![jump("join")]),
            ("otherwise", vec![jump("join")]),
            ("join", vec![instr(Opcode::Ret, None, None, None)]),
        ]);

        // Assert: `br_if_not` jumps when the condition is false, so the arms
        // come out the other way round from the instruction's operands.
        let entry = function.block(function.entry());
        let arms: Vec<String> = entry
            .successors()
            .map(|block| function.block(block).label.clone())
            .collect();
        assert_eq!(arms, vec!["consequent", "otherwise"]);
    }

    #[test]
    fn unreachable_blocks_are_deleted_during_construction() {
        // Arrange: the orphan every `return` leaves behind.
        let function = build_verified(&[
            ("entry", vec![instr(Opcode::Ret, None, None, None)]),
            ("unreachable_after_ret", vec![jump("exit")]),
            ("exit", vec![]),
        ]);

        // Assert: only the entry survives -- the exit block is reachable only
        // from the orphan, and a `return` is its own terminator.
        assert_eq!(function.block_count(), 1);
        assert_eq!(function.block(function.entry()).label, "entry");
    }

    #[test]
    fn arguments_are_fused_into_the_call_and_split_out_again() {
        // Arrange: `param 1; param 2; t1 = call g, 2`.
        let blocks = [
            (
                "entry",
                vec![
                    instr(Opcode::Param, None, Some(TacOperand::ImmInt(1)), None),
                    instr(Opcode::Param, None, Some(TacOperand::ImmInt(2)), None),
                    instr(
                        Opcode::Call,
                        Some(temp("t1")),
                        Some(label("g")),
                        Some(TacOperand::ImmInt(2)),
                    ),
                    instr(Opcode::Ret, None, Some(temp("t1")), None),
                ],
            ),
            ("exit", vec![]),
        ];
        let function = build_verified(&blocks);

        // Assert: one instruction in SSA ...
        assert_eq!(
            function.to_string(),
            "\
function f {
.entry:
    %v0 = call g(1, 2)
    store_slot %t1, %v0
    %v1 = load_slot %t1
    ret %v1
}
"
        );

        // ... and the `param` run is back, immediately before the call, in the
        // TAC the backend consumes.
        let rebuilt = to_cfg(&function);
        let opcodes: Vec<Opcode> = rebuilt.blocks["entry"]
            .instructions
            .iter()
            .map(|instr| instr.opcode.clone())
            .collect();
        assert_eq!(
            opcodes,
            vec![
                Opcode::Param,
                Opcode::Param,
                Opcode::Call,
                Opcode::Mov,
                Opcode::Mov,
                Opcode::Ret
            ]
        );
    }

    #[test]
    fn incoming_arguments_stay_in_one_unbroken_run() {
        // Arrange: two parameters, which the backend lowers as a single
        // simultaneous assignment.
        let function = build_verified(&[
            (
                "entry",
                vec![
                    instr(
                        Opcode::GetParam,
                        Some(var(0)),
                        Some(TacOperand::ImmInt(0)),
                        None,
                    ),
                    instr(
                        Opcode::GetParam,
                        Some(var(1)),
                        Some(TacOperand::ImmInt(1)),
                        None,
                    ),
                    instr(Opcode::Ret, None, Some(var(0)), None),
                ],
            ),
            ("exit", vec![]),
        ]);

        // Assert: both reads happen before either is placed, in SSA and in the
        // TAC it lowers back to.
        assert_eq!(
            function.to_string(),
            "\
function f {
.entry:
    %v0 = get_param 0
    %v1 = get_param 1
    store_slot %r0, %v0
    store_slot %r1, %v1
    %v2 = load_slot %r0
    ret %v2
}
"
        );
        let rebuilt = to_cfg(&function);
        let opcodes: Vec<Opcode> = rebuilt.blocks["entry"]
            .instructions
            .iter()
            .map(|instr| instr.opcode.clone())
            .collect();
        assert_eq!(opcodes[..2], [Opcode::GetParam, Opcode::GetParam]);
    }

    #[test]
    fn a_critical_edge_gets_a_block_of_its_own() {
        // Arrange: the branch in `entry` goes straight to `join`, which the
        // other arm also reaches -- a critical edge in the shape an `if` with
        // an empty arm produces.
        let function = build_verified(&[
            (
                "entry",
                vec![
                    instr(
                        Opcode::BranchIf,
                        None,
                        Some(temp("t1")),
                        Some(label("consequent")),
                    ),
                    jump("join"),
                ],
            ),
            ("consequent", vec![jump("join")]),
            ("join", vec![instr(Opcode::Ret, None, None, None)]),
        ]);

        // Assert: a block was interposed, and it holds nothing but the jump.
        let entry = function.entry();
        let arms: Vec<String> = function
            .block(entry)
            .successors()
            .map(|block| function.block(block).label.clone())
            .collect();
        assert_eq!(
            arms,
            vec!["consequent".to_string(), "entry_to_join".to_string()]
        );

        let split = function
            .block_ids()
            .find(|&block| function.block(block).label == "entry_to_join")
            .expect("the critical edge was split");
        assert!(function.block(split).insts.is_empty());
        assert_eq!(
            function.block(split).successors().collect::<Vec<_>>().len(),
            1
        );

        // The join keeps both predecessors, one of them by way of the new
        // block, so any phi argument would still line up.
        let join = function
            .block_ids()
            .find(|&block| function.block(block).label == "join")
            .expect("the join survives");
        assert_eq!(function.block(join).preds().len(), 2);
    }

    #[test]
    fn an_edge_that_is_not_critical_is_left_alone() {
        // Arrange: both arms lead to blocks of their own, so neither edge has
        // anywhere it needs to put code.
        let function = build_verified(&[
            (
                "entry",
                vec![
                    instr(
                        Opcode::BranchIf,
                        None,
                        Some(temp("t1")),
                        Some(label("consequent")),
                    ),
                    jump("otherwise"),
                ],
            ),
            ("consequent", vec![jump("join")]),
            ("otherwise", vec![jump("join")]),
            ("join", vec![instr(Opcode::Ret, None, None, None)]),
        ]);

        // Assert: four blocks, exactly the ones written above.
        assert_eq!(function.block_count(), 4);
    }

    #[test]
    fn a_promoted_variable_leaves_memory_entirely() {
        // Arrange: `r0 = 1; t1 = r0 + 2; return t1`, the same function the
        // memory form above was built from.
        let function = promote_verified(
            &[
                (
                    "entry",
                    vec![
                        mov(var(0), TacOperand::ImmInt(1)),
                        instr(
                            Opcode::Add,
                            Some(temp("t1")),
                            Some(var(0)),
                            Some(TacOperand::ImmInt(2)),
                        ),
                        instr(Opcode::Ret, None, Some(temp("t1")), None),
                    ],
                ),
                ("exit", vec![]),
            ],
            &[],
        );

        // Assert: no load, no store, and the values read each other directly.
        // The first value is printed as a version of the variable it came
        // from, which is what debug output and diagnostics need.
        assert_eq!(
            function.to_string(),
            "\
function f {
.entry:
    %r0.0 = 1
    %v2 = %r0.0 + 2
    ret %v2
}
"
        );
    }

    #[test]
    fn a_variable_assigned_in_both_arms_gets_a_phi_at_the_join() {
        // Arrange: `if (t1) r0 = 1; else r0 = 2; return r0`.
        let function = promote_verified(
            &if_else(
                vec![mov(var(0), TacOperand::ImmInt(1))],
                vec![mov(var(0), TacOperand::ImmInt(2))],
            ),
            &[],
        );

        // Assert: the join reads a phi with one argument per predecessor,
        // each of them the value that predecessor computed.
        let join = function
            .block_ids()
            .find(|&block| function.block(block).label == "join")
            .expect("the join survives");
        let phi = &function.block(join).phis[0];
        assert_eq!(function.block(join).phis.len(), 1);
        assert_eq!(phi.args.len(), function.block(join).preds().len());
        assert_eq!(phi.args.len(), 2);

        // Argument `i` belongs to predecessor `i`, whatever order the block
        // happens to record them in.
        for (position, &pred) in function.block(join).preds().iter().enumerate() {
            let assigned = *function
                .block(pred)
                .insts
                .iter()
                .find_map(|&inst| function.inst(inst).dest.as_ref())
                .expect("each arm computes the value it assigns");
            assert_eq!(phi.args[position], Operand::Value(assigned));
        }
    }

    #[test]
    fn reading_a_variable_that_was_never_written_yields_an_undefined_value() {
        // Arrange: `if (t1) r0 = 1; return r0` -- nothing writes `r0` on the
        // other path, which SSA cannot represent as a use with no definition.
        let function = promote_verified(
            &if_else(vec![mov(var(0), TacOperand::ImmInt(1))], vec![]),
            &[],
        );

        // Assert: the missing definition is an explicit `undef` in the entry
        // block, which dominates every use of it.
        let entry = function.entry();
        let undefined = function
            .block(entry)
            .insts
            .iter()
            .find(|&&inst| matches!(function.inst(inst).op, Op::Undef))
            .map(|&inst| function.inst(inst).dest.expect("undef defines a value"))
            .expect("the unwritten path needs an undefined value");

        let join = function
            .block_ids()
            .find(|&block| function.block(block).label == "join")
            .expect("the join survives");
        assert!(
            function.block(join).phis[0]
                .args
                .contains(&Operand::Value(undefined))
        );
    }

    #[test]
    fn a_function_that_writes_every_path_needs_no_undefined_value() {
        // Arrange: the same shape with both arms assigning.
        let function = promote_verified(
            &if_else(
                vec![mov(var(0), TacOperand::ImmInt(1))],
                vec![mov(var(0), TacOperand::ImmInt(2))],
            ),
            &[],
        );

        // Assert: the placeholder construction creates up front is taken out
        // again when nothing needs it.
        assert!(
            !function
                .block_ids()
                .flat_map(|block| function.block(block).insts.clone())
                .any(|inst| matches!(function.inst(inst).op, Op::Undef))
        );
    }

    #[test]
    fn a_loop_variable_gets_a_phi_at_the_header() {
        // Arrange: `i = 0; while (i < 5) i = i + 1; return i`.
        let function = promote_verified(&loop_blocks(), &[]);

        // Assert: the header's phi takes the initial value along the edge
        // from the entry and the updated one along the back edge.
        let header = function
            .block_ids()
            .find(|&block| function.block(block).label == "cond")
            .expect("the header survives");
        let phi = &function.block(header).phis[0];
        assert_eq!(function.block(header).phis.len(), 1);

        let argument = |from: &str| {
            let position = function
                .block(header)
                .preds()
                .iter()
                .position(|&pred| function.block(pred).label == from)
                .expect("the header is entered from here");
            phi.args[position]
        };
        let initial = *function
            .block(function.entry())
            .insts
            .iter()
            .find_map(|&inst| function.inst(inst).dest.as_ref())
            .expect("the entry computes the initial value");
        assert_eq!(argument("entry"), Operand::Value(initial));
        assert_ne!(argument("body"), Operand::Value(initial));
        assert!(matches!(argument("body"), Operand::Value(_)));
    }

    #[test]
    fn a_variable_whose_address_is_taken_keeps_its_loads_and_stores() {
        // Arrange: `r0 = 1; t1 = &r0; store t1, 5; return r0`.
        let function = promote_verified(
            &[
                (
                    "entry",
                    vec![
                        mov(var(0), TacOperand::ImmInt(1)),
                        instr(Opcode::AddrOf, Some(temp("t1")), Some(var(0)), None),
                        instr(
                            Opcode::Store,
                            None,
                            Some(temp("t1")),
                            Some(TacOperand::ImmInt(5)),
                        ),
                        instr(Opcode::Ret, None, Some(var(0)), None),
                    ],
                ),
                ("exit", vec![]),
            ],
            &[],
        );

        // Assert: the write through the pointer has to be visible to the read
        // by name, so `r0` stays in memory and `t1` -- an ordinary value --
        // does not.
        assert_eq!(
            function.to_string(),
            "\
function f {
.entry:
    %v0 = 1
    store_slot %r0, %v0
    %v1 = addr_of %r0
    store %v1, 5
    %v3 = load_slot %r0
    ret %v3
}
"
        );
    }

    #[test]
    fn an_array_keeps_its_storage_but_its_index_does_not() {
        // Arrange: `a[0] = 1; t1 = a[0]; return t1` with `a` a real array.
        let function = promote_verified(
            &[
                (
                    "entry",
                    vec![
                        instr(
                            Opcode::ArrayStore,
                            Some(var(0)),
                            Some(TacOperand::ImmInt(0)),
                            Some(TacOperand::ImmInt(1)),
                        ),
                        instr(
                            Opcode::ArrayLoad,
                            Some(temp("t1")),
                            Some(var(0)),
                            Some(TacOperand::ImmInt(0)),
                        ),
                        instr(Opcode::Ret, None, Some(temp("t1")), None),
                    ],
                ),
                ("exit", vec![]),
            ],
            &[(0, 4)],
        );

        // Assert
        assert_eq!(
            function.to_string(),
            "\
function f {
.entry:
    array_store %r0[0] = 1
    %v0 = array_load %r0[0]
    ret %v0
}
"
        );
    }

    /// `i = 0; while (i < 5) i = i + 1; return i`.
    fn loop_blocks() -> Vec<(&'static str, Vec<TACInstruction>)> {
        vec![
            (
                "entry",
                vec![mov(var(0), TacOperand::ImmInt(0)), jump("cond")],
            ),
            (
                "cond",
                vec![
                    instr(
                        Opcode::Lt,
                        Some(temp("t1")),
                        Some(var(0)),
                        Some(TacOperand::ImmInt(5)),
                    ),
                    instr(
                        Opcode::BranchIfNot,
                        None,
                        Some(temp("t1")),
                        Some(label("done")),
                    ),
                    jump("body"),
                ],
            ),
            (
                "body",
                vec![
                    instr(
                        Opcode::Add,
                        Some(temp("t2")),
                        Some(var(0)),
                        Some(TacOperand::ImmInt(1)),
                    ),
                    mov(var(0), temp("t2")),
                    jump("cond"),
                ],
            ),
            ("done", vec![instr(Opcode::Ret, None, Some(var(0)), None)]),
            ("exit", vec![]),
        ]
    }

    #[test]
    fn the_round_trip_preserves_the_control_flow_graph() {
        // Arrange: a loop, so the graph has a back edge and a join.
        let blocks = loop_blocks();

        // Act: build, lower back to TAC, and build again.
        let once = build_verified(&blocks);
        let twice = build("f", &to_cfg(&once));

        // Assert: the same blocks, joined the same way, and still valid SSA.
        //
        // Only the shape: the instructions are not identical, because
        // lowering a phi introduces copies that the next trip sees as ordinary
        // instructions. One trip is all the compiler makes, and the copy
        // propagation that would collapse them belongs to the passes rather
        // than to construction.
        assert!(verify_ssa(&twice).is_ok());
        let shape = |function: &Function| -> Vec<(String, Vec<String>)> {
            function
                .block_ids()
                .map(|block| {
                    (
                        function.block(block).label.clone(),
                        function
                            .block(block)
                            .successors()
                            .map(|successor| function.block(successor).label.clone())
                            .collect(),
                    )
                })
                .collect()
        };
        assert_eq!(shape(&once), shape(&twice));
    }
}
