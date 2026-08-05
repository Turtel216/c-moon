//! Static single assignment form.
//!
//! A [`Function`] is the middle-end's own representation of one function:
//! basic blocks in an arena, referring to each other by [`BlockId`], holding
//! instructions that produce [`ValueId`]s.  Every value is written exactly
//! once, which is what makes a dataflow analysis a walk over definitions
//! instead of a fixed point over the whole graph.
//!
//! # Values and slots
//!
//! Not every variable can become an SSA value.  One whose address is taken, or
//! that is really an array, has to keep a memory location that a pointer can
//! refer to.  Those stay [`Slot`]s and are reached exclusively through
//! [`Op::SlotLoad`] and [`Op::SlotStore`]; the two never mix, and the type of
//! each field decides which one it is.  Which variables qualify is decided by
//! [`promote`], and nothing else in this module makes that judgement.
//!
//! # Edges and phi arguments
//!
//! A block's successors are derived from its terminator, so the two can never
//! disagree.  Its predecessors are stored, because phi arguments are aligned
//! with them by position -- argument `i` is the value flowing in from
//! predecessor `i`.  That alignment is what makes the predecessor list private:
//! every operation that changes it goes through a method here that fixes up the
//! block's phi nodes in the same call.

pub mod build;
pub mod destruct;
pub mod dom;
pub mod mem2reg;
pub mod promote;
pub mod verify;

use std::collections::HashMap;

use crate::frontend::renamer::VarId;

/// Identifies one basic block within a function.
///
/// Blocks live in an arena and refer to each other by index.  A Python
/// developer would reach for object references here; in Rust an index-based
/// graph avoids both the reference cycles that `Rc<RefCell<_>>` would need and
/// the aliasing rules that make such a graph painful to mutate.  The cost is
/// that an index is only meaningful together with the function it indexes
/// into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlockId(u32);

impl BlockId {
    /// The block at position `index` of its function's block arena.
    ///
    /// # Panics
    ///
    /// Panics if `index` does not fit in a `u32`, which would mean a single
    /// function with more than four billion basic blocks.
    pub fn from_index(index: usize) -> Self {
        Self(u32::try_from(index).expect("Compiler Bug: more basic blocks than a u32 can index"))
    }

    /// This block's position in its function's block arena.
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// Identifies one SSA value: the result of exactly one instruction or phi.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ValueId(u32);

impl ValueId {
    /// This value's position in its function's value arena.
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// Identifies one instruction within a function.
///
/// Instructions live in a function-wide arena and blocks hold their ids in
/// order, so removing an instruction from a block leaves every other id valid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InstId(u32);

impl InstId {
    /// This instruction's position in its function's instruction arena.
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// Identifies one memory location: a variable that could not become a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SlotId(u32);

impl SlotId {
    /// The slot at position `index` of its function's slot arena.
    ///
    /// # Panics
    ///
    /// Panics if `index` does not fit in a `u32`.
    pub fn from_index(index: usize) -> Self {
        Self(u32::try_from(index).expect("Compiler Bug: more slots than a u32 can index"))
    }

    /// This slot's position in its function's slot arena.
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// What a slot was before the middle-end took it apart.
///
/// Slots are how a value gets back to the TAC operand it came from, so the two
/// kinds of TAC storage each get a variant.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SlotOrigin {
    /// A source-level variable, by its renamer id.
    Variable(VarId),
    /// A compiler-generated TAC temporary, by its name.
    Temporary(String),
}

/// One memory location of a function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slot {
    /// The TAC operand this slot stands for.
    pub origin: SlotOrigin,
}

/// Where a value is defined, and what it was called in the source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValueDef {
    /// The instruction or phi that produces this value.
    pub site: DefSite,
    /// The source variable this value is a version of, when it is one.
    ///
    /// Metadata only: two values sharing a `source` are still two different
    /// values, and no pass may conclude anything from the fact that they came
    /// from the same variable.
    pub source: Option<SourceName>,
}

/// The instruction or phi node that defines a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefSite {
    /// Defined by an instruction.
    Inst(InstId),
    /// Defined by the phi at the given position of the given block.
    Phi(BlockId, usize),
}

/// A value's source-level identity, for debug output and diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceName {
    /// The variable this value is a version of.
    pub variable: VarId,
    /// Which version, counting from zero in renaming order.
    pub version: u32,
}

/// An instruction's input.
///
/// There is deliberately no variant for a label or a slot: an operand is
/// always a value, which is what lets the verifier check every use against its
/// definition without a per-opcode table of which fields are really inputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operand {
    /// The result of another instruction or phi.
    Value(ValueId),
    /// An integer literal.
    Imm(i64),
}

impl Operand {
    /// The value this operand reads, if it reads one.
    pub fn value(self) -> Option<ValueId> {
        match self {
            Operand::Value(value) => Some(value),
            Operand::Imm(_) => None,
        }
    }
}

/// A two-operand arithmetic or relational operation.
///
/// The set matches the TAC opcodes it is built from; relational operations
/// produce 0 or 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Eq,
    Neq,
    Lt,
    Lte,
    Gt,
    Gte,
}

/// What an instruction does.
#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    /// `dest = lhs <op> rhs`
    Binary(BinOp, Operand, Operand),
    /// `dest = source`
    Copy(Operand),
    /// `dest = callee(args...)`
    Call { callee: String, args: Vec<Operand> },
    /// `dest = ` the incoming argument at this position.
    GetParam(usize),
    /// `dest = ` a value that was never defined on this path.
    ///
    /// Reading an uninitialised variable is undefined behaviour in C; this
    /// makes the undefinedness explicit rather than leaving a use with no
    /// definition, which SSA cannot represent.
    Undef,

    /// `dest = slot`
    SlotLoad { slot: SlotId },
    /// `slot = value`
    SlotStore { slot: SlotId, value: Operand },
    /// `dest = base[index]`
    ArrayLoad { base: SlotId, index: Operand },
    /// `base[index] = value`
    ArrayStore {
        base: SlotId,
        index: Operand,
        value: Operand,
    },
    /// `dest = *address`
    Load { address: Operand },
    /// `*address = value`
    Store { address: Operand, value: Operand },
    /// `dest = &slot`
    AddrOf { slot: SlotId },
}

impl Op {
    /// Does this operation produce a value?
    ///
    /// The three stores do not; everything else does, including a call to a
    /// function returning `void`, whose result is simply never used.
    pub fn defines_value(&self) -> bool {
        !matches!(
            self,
            Op::SlotStore { .. } | Op::ArrayStore { .. } | Op::Store { .. }
        )
    }

    /// Can this operation be removed when its result is unused?
    ///
    /// A store or a call may be doing something the value it returns does not
    /// account for, so neither is dead merely because nothing reads it.
    pub fn is_pure(&self) -> bool {
        !matches!(
            self,
            Op::SlotStore { .. }
                | Op::ArrayStore { .. }
                | Op::Store { .. }
                | Op::Call { .. }
                | Op::GetParam(_)
        )
    }

    /// The operands this operation reads, in order.
    pub fn operands(&self) -> Vec<Operand> {
        match self {
            Op::Binary(_, lhs, rhs) => vec![*lhs, *rhs],
            Op::Copy(source) => vec![*source],
            Op::Call { args, .. } => args.clone(),
            Op::GetParam(_) | Op::Undef | Op::SlotLoad { .. } | Op::AddrOf { .. } => Vec::new(),
            Op::SlotStore { value, .. } => vec![*value],
            Op::ArrayLoad { index, .. } => vec![*index],
            Op::ArrayStore { index, value, .. } => vec![*index, *value],
            Op::Load { address } => vec![*address],
            Op::Store { address, value } => vec![*address, *value],
        }
    }

    /// Every operand of this operation, mutably, in the same order as
    /// [`Op::operands`].
    ///
    /// Rust note: returning `&mut` references to fields of an enum needs the
    /// match to bind them mutably, which is why this cannot be expressed as a
    /// map over [`Op::operands`].
    pub fn operands_mut(&mut self) -> Vec<&mut Operand> {
        match self {
            Op::Binary(_, lhs, rhs) => vec![lhs, rhs],
            Op::Copy(source) => vec![source],
            Op::Call { args, .. } => args.iter_mut().collect(),
            Op::GetParam(_) | Op::Undef | Op::SlotLoad { .. } | Op::AddrOf { .. } => Vec::new(),
            Op::SlotStore { value, .. } => vec![value],
            Op::ArrayLoad { index, .. } => vec![index],
            Op::ArrayStore { index, value, .. } => vec![index, value],
            Op::Load { address } => vec![address],
            Op::Store { address, value } => vec![address, value],
        }
    }
}

/// One instruction: an operation and the value it produces.
#[derive(Debug, Clone, PartialEq)]
pub struct Inst {
    /// The value this instruction defines, absent for the stores.
    pub dest: Option<ValueId>,
    /// What it does.
    pub op: Op,
}

/// A phi node: the value of a variable at a join, one argument per incoming
/// edge.
///
/// `args` is aligned with the block's predecessor list by position, and the
/// two are only ever changed together -- see [`Function::remove_edge`].
#[derive(Debug, Clone, PartialEq)]
pub struct Phi {
    /// The value this phi defines.
    pub dest: ValueId,
    /// The value arriving from each predecessor, in predecessor order.
    pub args: Vec<Operand>,
    /// The slot this phi was created for, kept so renaming can find it again.
    pub slot: SlotId,
}

/// How a block ends.
#[derive(Debug, Clone, PartialEq)]
pub enum Terminator {
    /// Continue at another block.
    Jump(BlockId),
    /// Continue at `then_block` when `cond` is non-zero, `else_block`
    /// otherwise.
    Branch {
        cond: Operand,
        then_block: BlockId,
        else_block: BlockId,
    },
    /// Leave the function, with a value when it returns one.
    Return(Option<Operand>),
}

impl Terminator {
    /// The blocks this terminator can transfer control to, in order.
    ///
    /// A branch whose arms are the same block yields it twice: the two edges
    /// are distinct, and each has its own phi argument.
    pub fn successors(&self) -> impl Iterator<Item = BlockId> + '_ {
        // Rust note: an array of `Option` flattened into an iterator gives a
        // zero-, one- or two-element iterator without allocating.
        match self {
            Terminator::Jump(target) => [Some(*target), None],
            Terminator::Branch {
                then_block,
                else_block,
                ..
            } => [Some(*then_block), Some(*else_block)],
            Terminator::Return(_) => [None, None],
        }
        .into_iter()
        .flatten()
    }

    /// The operands this terminator reads.
    pub fn operands(&self) -> Vec<Operand> {
        match self {
            Terminator::Jump(_) => Vec::new(),
            Terminator::Branch { cond, .. } => vec![*cond],
            Terminator::Return(value) => value.iter().copied().collect(),
        }
    }

    /// Every operand of this terminator, mutably.
    pub fn operands_mut(&mut self) -> Vec<&mut Operand> {
        match self {
            Terminator::Jump(_) => Vec::new(),
            Terminator::Branch { cond, .. } => vec![cond],
            Terminator::Return(value) => value.iter_mut().collect(),
        }
    }
}

/// One basic block.
#[derive(Debug, Clone)]
pub struct Block {
    /// The label this block had in the TAC CFG, kept so the assembly and the
    /// IR dumps stay recognisable across the round trip.
    pub label: String,
    /// Phi nodes, which are always at the top of the block.
    pub phis: Vec<Phi>,
    /// The block's instructions, in order.
    pub insts: Vec<InstId>,
    /// How the block ends.  A block starts out returning, i.e. with no
    /// successors, until its real terminator is set.
    term: Terminator,
    /// Incoming edges.  Private: phi arguments are aligned with this list, so
    /// nothing may change it without changing them.
    preds: Vec<BlockId>,
}

impl Block {
    /// The blocks this one can transfer control to.
    pub fn successors(&self) -> impl Iterator<Item = BlockId> + '_ {
        self.term.successors()
    }

    /// The blocks that can transfer control to this one.  Phi argument `i`
    /// belongs to predecessor `i`.
    pub fn preds(&self) -> &[BlockId] {
        &self.preds
    }

    /// How this block ends.
    pub fn terminator(&self) -> &Terminator {
        &self.term
    }

    /// How this block ends, mutably.
    ///
    /// Only for changing a terminator's *operands*.  Changing which blocks it
    /// jumps to would leave the predecessor lists and the phi arguments stale;
    /// use [`Function::set_terminator`] for that.
    pub fn terminator_operands_mut(&mut self) -> Vec<&mut Operand> {
        self.term.operands_mut()
    }
}

/// One function in SSA form.
#[derive(Debug, Clone)]
pub struct Function {
    /// The function's symbol name.
    pub name: String,
    /// The label the TAC exit block had, so the round trip can put it back.
    exit_label: String,
    blocks: Vec<Block>,
    insts: Vec<Inst>,
    values: Vec<ValueDef>,
    slots: Vec<Slot>,
    /// Reverse lookup for slot creation, so one variable gets one slot.
    slot_of_origin: HashMap<SlotOrigin, SlotId>,
    entry: BlockId,
}

impl Function {
    /// Start an empty function with just an entry block.
    ///
    /// # Arguments
    ///
    /// * `name` - the function's symbol name
    /// * `entry_label` - label of the block execution starts at
    /// * `exit_label` - label the TAC exit block is to be given again
    pub fn new(name: String, entry_label: String, exit_label: String) -> Self {
        let entry = Block {
            label: entry_label,
            phis: Vec::new(),
            insts: Vec::new(),
            term: Terminator::Return(None),
            preds: Vec::new(),
        };
        Self {
            name,
            exit_label,
            blocks: vec![entry],
            insts: Vec::new(),
            values: Vec::new(),
            slots: Vec::new(),
            slot_of_origin: HashMap::new(),
            entry: BlockId::from_index(0),
        }
    }

    // ### Reading ###

    /// The block execution starts at.
    pub fn entry(&self) -> BlockId {
        self.entry
    }

    /// The label the TAC exit block is to be given again.
    pub fn exit_label(&self) -> &str {
        &self.exit_label
    }

    /// How many blocks the function has.
    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }

    /// Every block of the function, in arena order.
    pub fn block_ids(&self) -> impl Iterator<Item = BlockId> {
        (0..self.blocks.len()).map(BlockId::from_index)
    }

    /// One block.
    pub fn block(&self, block: BlockId) -> &Block {
        &self.blocks[block.index()]
    }

    /// One block, mutably.
    ///
    /// The predecessor list and the terminator stay unreachable through this:
    /// both are private to the block, so instructions and phi *arguments* are
    /// all that can be changed here.
    pub fn block_mut(&mut self, block: BlockId) -> &mut Block {
        &mut self.blocks[block.index()]
    }

    /// One instruction.
    pub fn inst(&self, inst: InstId) -> &Inst {
        &self.insts[inst.index()]
    }

    /// One instruction, mutably.
    pub fn inst_mut(&mut self, inst: InstId) -> &mut Inst {
        &mut self.insts[inst.index()]
    }

    /// How many values the function has defined.
    pub fn value_count(&self) -> usize {
        self.values.len()
    }

    /// Where a value is defined and what it is a version of.
    pub fn value_def(&self, value: ValueId) -> &ValueDef {
        &self.values[value.index()]
    }

    /// Every slot of the function, in arena order.
    pub fn slot_ids(&self) -> impl Iterator<Item = SlotId> {
        (0..self.slots.len()).map(SlotId::from_index)
    }

    /// One memory location.
    pub fn slot(&self, slot: SlotId) -> &Slot {
        &self.slots[slot.index()]
    }

    // ### Building ###

    /// Add a block with no instructions, which returns until told otherwise.
    pub fn add_block(&mut self, label: String) -> BlockId {
        self.blocks.push(Block {
            label,
            phis: Vec::new(),
            insts: Vec::new(),
            term: Terminator::Return(None),
            preds: Vec::new(),
        });
        BlockId::from_index(self.blocks.len() - 1)
    }

    /// The slot standing for `origin`, creating it the first time it is asked
    /// for.
    pub fn slot_for(&mut self, origin: SlotOrigin) -> SlotId {
        if let Some(&slot) = self.slot_of_origin.get(&origin) {
            return slot;
        }
        let slot = SlotId::from_index(self.slots.len());
        self.slots.push(Slot {
            origin: origin.clone(),
        });
        self.slot_of_origin.insert(origin, slot);
        slot
    }

    /// Append an instruction to a block.
    ///
    /// # Returns
    ///
    /// The value it defines, or `None` for the operations that define none.
    /// The value is created here rather than by the caller, so a value cannot
    /// exist without the instruction that defines it.
    pub fn emit(&mut self, block: BlockId, op: Op) -> Option<ValueId> {
        let inst = InstId(self.insts.len() as u32);
        let dest = op.defines_value().then(|| {
            self.values.push(ValueDef {
                site: DefSite::Inst(inst),
                source: None,
            });
            ValueId((self.values.len() - 1) as u32)
        });

        self.insts.push(Inst { dest, op });
        self.blocks[block.index()].insts.push(inst);
        dest
    }

    /// Record which source variable a value is a version of.
    pub fn name_value(&mut self, value: ValueId, source: SourceName) {
        self.values[value.index()].source = Some(source);
    }

    /// Add a phi for `slot` at the top of `block`, with one argument per
    /// predecessor.
    ///
    /// The arguments start out reading the phi's own result.  Renaming
    /// overwrites every one of them -- it visits each predecessor exactly
    /// once, and writes that predecessor's argument in every phi of every
    /// successor -- so the placeholder never survives.  It is not a marker the
    /// verifier can check: a phi whose argument really is its own result is
    /// legitimate SSA, and is what a loop produces for a variable the loop
    /// does not assign.
    ///
    /// # Returns
    ///
    /// The value the phi defines.
    pub fn add_phi(&mut self, block: BlockId, slot: SlotId) -> ValueId {
        let position = self.blocks[block.index()].phis.len();
        self.values.push(ValueDef {
            site: DefSite::Phi(block, position),
            source: None,
        });
        let dest = ValueId((self.values.len() - 1) as u32);

        let arity = self.blocks[block.index()].preds.len();
        self.blocks[block.index()].phis.push(Phi {
            dest,
            args: vec![Operand::Value(dest); arity],
            slot,
        });
        dest
    }

    // ### Edges ###

    /// Change how a block ends, keeping predecessor lists and phi arguments
    /// consistent with it.
    ///
    /// Edges the new terminator keeps are left alone, so the phi arguments
    /// belonging to them survive.  Edges it drops are removed with
    /// [`Function::remove_edge`].
    ///
    /// # Panics
    ///
    /// Panics if the new terminator adds an edge into a block that already has
    /// phi nodes: there would be no argument to give them.  Retargeting an
    /// edge into a block with phis is [`Function::split_edge`]'s job, or needs
    /// an operation that says what flows along the new edge.
    pub fn set_terminator(&mut self, block: BlockId, term: Terminator) {
        let old: Vec<BlockId> = self.blocks[block.index()].term.successors().collect();
        let mut added: Vec<BlockId> = term.successors().collect();

        // Match each old edge against the new ones; what is left over in
        // `added` is genuinely new, and what fails to match is genuinely gone.
        for successor in old {
            match added.iter().position(|&target| target == successor) {
                Some(position) => {
                    added.remove(position);
                }
                None => self.remove_edge(block, successor),
            }
        }

        for successor in added {
            assert!(
                self.blocks[successor.index()].phis.is_empty(),
                "Compiler Bug: new edge into .{} would leave its phi nodes an argument short",
                self.blocks[successor.index()].label
            );
            self.blocks[successor.index()].preds.push(block);
        }

        self.blocks[block.index()].term = term;
    }

    /// Remove one edge `from -> to`, together with the phi arguments that
    /// belonged to it.
    ///
    /// Only the predecessor entry is removed; the terminator is the caller's
    /// business, and [`Function::set_terminator`] is what normally calls this.
    ///
    /// # Panics
    ///
    /// Panics if there is no such edge.
    fn remove_edge(&mut self, from: BlockId, to: BlockId) {
        let target = &mut self.blocks[to.index()];
        let position = target
            .preds
            .iter()
            .position(|&pred| pred == from)
            .expect("Compiler Bug: removing an edge that is not in the predecessor list");

        // With two edges from the same block, either argument will do: both
        // carry whatever that block computed, so they are the same value.
        target.preds.remove(position);
        for phi in &mut target.phis {
            phi.args.remove(position);
        }
    }

    /// Put a new block on the edge `from -> to`, and return it.
    ///
    /// The new block jumps straight to `to`.  Phi arguments in `to` keep their
    /// positions -- the edge still arrives at the same index, just from
    /// somewhere else -- which is what makes this the safe way to break up an
    /// edge.
    ///
    /// # Panics
    ///
    /// Panics if `from` does not branch to `to`.
    pub fn split_edge(&mut self, from: BlockId, to: BlockId) -> BlockId {
        let label = self.fresh_label(&format!(
            "{}_to_{}",
            self.blocks[from.index()].label,
            self.blocks[to.index()].label
        ));
        let split = self.add_block(label);

        // The split block takes the old edge's place, so `to` keeps exactly
        // the predecessors it had, in the same order.
        let mut retargeted = false;
        for pred in &mut self.blocks[to.index()].preds {
            if *pred == from {
                *pred = split;
                retargeted = true;
                break;
            }
        }
        assert!(
            retargeted,
            "Compiler Bug: splitting an edge that is not in the predecessor list"
        );

        self.blocks[split.index()].term = Terminator::Jump(to);
        self.blocks[split.index()].preds.push(from);
        self.blocks[from.index()].term.retarget(to, split);

        split
    }

    /// A label starting with `prefix` that no block has yet.
    fn fresh_label(&self, prefix: &str) -> String {
        let mut candidate = prefix.to_string();
        let mut suffix = 0;
        while self.blocks.iter().any(|block| block.label == candidate) {
            suffix += 1;
            candidate = format!("{}_{}", prefix, suffix);
        }
        candidate
    }

    /// The graph shape of this function, as the adjacency lists [`dom`] works
    /// on.
    ///
    /// # Returns
    ///
    /// The predecessors and the successors of every block, by block index.
    /// Predecessors come out in the order phi arguments are aligned with.
    pub fn adjacency(&self) -> (Vec<Vec<BlockId>>, Vec<Vec<BlockId>>) {
        let predecessors = self
            .blocks
            .iter()
            .map(|block| block.preds.clone())
            .collect();
        let successors = self
            .blocks
            .iter()
            .map(|block| block.term.successors().collect())
            .collect();
        (predecessors, successors)
    }

    /// Delete every block the entry cannot reach, renumbering the rest.
    ///
    /// Dominance is undefined for unreachable blocks, so this runs before any
    /// of it -- see [`dom`].  Deleting a block also deletes its outgoing
    /// edges, which is why the phi arguments of surviving blocks have to be
    /// dropped along with the predecessors they belonged to.
    ///
    /// **Every [`BlockId`] held across this call is invalidated**: the arena is
    /// compacted so that block indices stay dense, which is what dominance
    /// needs.
    pub fn retain_reachable(&mut self) {
        let mut reachable = vec![false; self.blocks.len()];
        let mut stack = vec![self.entry];
        reachable[self.entry.index()] = true;
        while let Some(block) = stack.pop() {
            for successor in self.blocks[block.index()].term.successors() {
                if !reachable[successor.index()] {
                    reachable[successor.index()] = true;
                    stack.push(successor);
                }
            }
        }

        if reachable.iter().all(|&kept| kept) {
            return;
        }

        // New index of every surviving block, in their old relative order.
        let mut renumbered = vec![None; self.blocks.len()];
        let mut next = 0;
        for (index, &kept) in reachable.iter().enumerate() {
            if kept {
                renumbered[index] = Some(BlockId::from_index(next));
                next += 1;
            }
        }

        // Drop the predecessors that are gone, and with them their phi
        // arguments, before anything is renumbered.  Walking backwards keeps
        // the positions not yet examined from shifting under the removal.
        for (index, block) in self.blocks.iter_mut().enumerate() {
            if !reachable[index] {
                continue;
            }
            for position in (0..block.preds.len()).rev() {
                if !reachable[block.preds[position].index()] {
                    block.preds.remove(position);
                    for phi in &mut block.phis {
                        phi.args.remove(position);
                    }
                }
            }
        }

        // Rust note: `mem::take` moves the vector out and leaves an empty one
        // behind, so the surviving blocks can be moved into a fresh arena
        // without cloning any of them.
        let mut kept = Vec::with_capacity(next);
        for (index, block) in std::mem::take(&mut self.blocks).into_iter().enumerate() {
            if reachable[index] {
                kept.push(block);
            }
        }
        self.blocks = kept;

        let remap = |block: &mut BlockId| {
            *block = renumbered[block.index()].expect("Compiler Bug: edge into a deleted block");
        };
        for block in &mut self.blocks {
            for pred in &mut block.preds {
                remap(pred);
            }
            block.term.remap_blocks(&renumbered);
        }
        for value in &mut self.values {
            if let DefSite::Phi(block, _) = &mut value.site {
                remap(block);
            }
        }
        self.entry =
            renumbered[self.entry.index()].expect("Compiler Bug: entry became unreachable");
    }
}

impl Terminator {
    /// Send the single edge that goes to `from` to `to` instead.
    fn retarget(&mut self, from: BlockId, to: BlockId) {
        match self {
            Terminator::Jump(target) => {
                if *target == from {
                    *target = to;
                }
            }
            Terminator::Branch {
                then_block,
                else_block,
                ..
            } => {
                // Only one arm is moved even when both point at `from`: the
                // two edges are separate, and a split breaks up one of them.
                if *then_block == from {
                    *then_block = to;
                } else if *else_block == from {
                    *else_block = to;
                }
            }
            Terminator::Return(_) => {}
        }
    }

    /// Renumber the blocks this terminator names.
    fn remap_blocks(&mut self, renumbered: &[Option<BlockId>]) {
        let remap = |block: &mut BlockId| {
            *block = renumbered[block.index()].expect("Compiler Bug: edge into a deleted block");
        };
        match self {
            Terminator::Jump(target) => remap(target),
            Terminator::Branch {
                then_block,
                else_block,
                ..
            } => {
                remap(then_block);
                remap(else_block);
            }
            Terminator::Return(_) => {}
        }
    }
}
