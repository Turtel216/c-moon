//! Liveness Analysis
//!
//! Computes live intervals for all virtual registers across a CFG.
//!
//! The algorithm proceeds in four phases:
//! 1. **Linearize** the CFG into a flat instruction sequence with global indices.
//! 2. **Compute block-level GEN/KILL** sets (upward-exposed uses / definitions).
//! 3. **Iterative backward dataflow** to compute LIVE_IN/LIVE_OUT per block.
//! 4. **Build live intervals** from per-block liveness (Poletto & Sarkar method).

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

use crate::middle::ir::{BasicBlock, CFG, Opcode, Operand, TACInstruction};

// ### Virtual Register - unified representation for the backend ###

/// A unified representation of TAC variables (`Var(usize)`) and temporaries
/// (`Temp(String)`).  The register allocator and liveness analysis work
/// exclusively over `VirtualReg` values rather than raw `Operand`s.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum VirtualReg {
    /// A compiler-generated temporary, e.g. `t1`, `t2`.
    Temp(String),
    /// A renamed source-level variable, e.g. var #0, var #1.
    Var(usize),
}

impl std::fmt::Display for VirtualReg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VirtualReg::Temp(name) => write!(f, "%{}", name),
            VirtualReg::Var(id) => write!(f, "%r{}", id),
        }
    }
}

/// Try to extract a `VirtualReg` from a TAC `Operand`.
/// Returns `None` for immediates and labels — they are not virtual registers.
pub fn operand_to_vreg(op: &Operand) -> Option<VirtualReg> {
    match op {
        Operand::Var(id) => Some(VirtualReg::Var(*id)),
        Operand::Temp(name) => Some(VirtualReg::Temp(name.clone())),
        Operand::ImmInt(_) | Operand::Label(_) => None,
    }
}

// ### Linearized CFG ###

/// A flattened view of the CFG where every instruction has a unique
/// global index.  This is consumed by the register allocator.
pub struct LinearizedCfg {
    /// All instructions in linearized order, paired with their source
    /// block label (for label emission during lowering).
    pub instructions: Vec<(TACInstruction, String)>,
    /// Map from block label → `(start_index, end_index)` — both inclusive.
    /// Empty blocks are omitted.
    pub block_ranges: HashMap<String, (usize, usize)>,
    /// The blocks in the order they were linearized (DFS preorder from entry).
    pub block_order: Vec<String>,
}

/// A live interval for a single virtual register.
#[derive(Debug, Clone)]
pub struct LiveInterval {
    pub vreg: VirtualReg,
    /// First instruction index where this vreg is live (inclusive).
    pub start: usize,
    /// Last instruction index where this vreg is live (inclusive).
    pub end: usize,
}

impl LiveInterval {
    /// Returns `true` if this value has to survive `call`, i.e. it is live
    /// on both sides of it.
    ///
    /// Such a value cannot be kept in a caller-saved register, because the
    /// callee is free to overwrite every one of them.
    pub fn crosses_call(&self, call: &CallSite) -> bool {
        if self.start > call.position || self.end < call.position {
            return false;
        }
        // A value produced *by* the call is only born once the callee has
        // returned, so it has nothing to survive.
        !(self.start == call.position && call.defines.as_ref() == Some(&self.vreg))
    }
}

// ### Call sites ###

/// A `Call` instruction in the linearized instruction sequence.
///
/// The register allocator needs these to know which values are held across
/// a call and must therefore avoid caller-saved registers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallSite {
    /// Index of the `Call` in the linearized instruction sequence.
    pub position: usize,
    /// The virtual register the call writes its return value into, if any.
    pub defines: Option<VirtualReg>,
}

/// Collect every call site in a linearized function, in ascending position
/// order (which the allocator relies on for binary search).
pub fn find_call_sites(linear: &LinearizedCfg) -> Vec<CallSite> {
    linear
        .instructions
        .iter()
        .enumerate()
        .filter(|(_, (instr, _))| instr.opcode == Opcode::Call)
        .map(|(position, (instr, _))| CallSite {
            position,
            defines: instr.dest.as_ref().and_then(operand_to_vreg),
        })
        .collect()
}

// ### Linearize the CFG ###

/// Flatten the CFG into a linear instruction sequence using DFS preorder
/// traversal from the entry block.
pub fn linearize_cfg(cfg: &CFG) -> LinearizedCfg {
    let mut order = Vec::new();
    let mut visited = HashSet::new();
    let mut stack = VecDeque::new();

    // DFS preorder from entry
    stack.push_back(cfg.entry.clone());
    while let Some(label) = stack.pop_front() {
        if !visited.insert(label.clone()) {
            continue;
        }
        order.push(label.clone());

        if let Some(block) = cfg.blocks.get(&label) {
            // Push successors in reverse so the first successor is visited first.
            for succ in block.successors.iter().rev() {
                if !visited.contains(succ) {
                    stack.push_front(succ.clone());
                }
            }
        }
    }

    // Also include any blocks not reachable from entry (unreachable-after-ret blocks).
    for label in cfg.blocks.keys() {
        if !visited.contains(label) {
            order.push(label.clone());
        }
    }

    let mut instructions = Vec::new();
    let mut block_ranges = HashMap::new();

    for label in &order {
        if let Some(block) = cfg.blocks.get(label) {
            if block.instructions.is_empty() {
                continue;
            }
            let start = instructions.len();
            for instr in &block.instructions {
                instructions.push((instr.clone(), label.clone()));
            }
            let end = instructions.len() - 1;
            block_ranges.insert(label.clone(), (start, end));
        }
    }

    LinearizedCfg {
        instructions,
        block_ranges,
        block_order: order,
    }
}

// ### Phase 2: DEF / USE extraction per instruction ###

/// Extract the set of virtual registers USED (read) by a single instruction.
pub fn instruction_uses(instr: &TACInstruction) -> Vec<VirtualReg> {
    let mut uses = Vec::new();

    match instr.opcode {
        // Binary arithmetic/comparison: reads arg1 and arg2
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
            if let Some(ref op) = instr.arg1 {
                if let Some(v) = operand_to_vreg(op) {
                    uses.push(v);
                }
            }
            if let Some(ref op) = instr.arg2 {
                if let Some(v) = operand_to_vreg(op) {
                    uses.push(v);
                }
            }
        }
        // Mov: reads arg1
        Opcode::Mov => {
            if let Some(ref op) = instr.arg1 {
                if let Some(v) = operand_to_vreg(op) {
                    uses.push(v);
                }
            }
        }
        // Jump: arg1 is a label — no vreg use
        Opcode::Jump => {}
        // Branch: arg1 is the condition vreg, arg2 is a label
        Opcode::BranchIf | Opcode::BranchIfNot => {
            if let Some(ref op) = instr.arg1 {
                if let Some(v) = operand_to_vreg(op) {
                    uses.push(v);
                }
            }
        }
        // Param: reads arg1
        Opcode::Param => {
            if let Some(ref op) = instr.arg1 {
                if let Some(v) = operand_to_vreg(op) {
                    uses.push(v);
                }
            }
        }
        // Call: arg1 is a label, arg2 is the arg count — no vreg use here
        Opcode::Call => {}
        // Ret: reads the return value (arg1)
        Opcode::Ret => {
            if let Some(ref op) = instr.arg1 {
                if let Some(v) = operand_to_vreg(op) {
                    uses.push(v);
                }
            }
        }
        // GetParam: arg1 is an immediate index — no vreg use
        Opcode::GetParam => {}

        // ArrayStore: reads base (dest field), index (arg1), and value (arg2).
        // Note: dest is used as the *base address*, not a true definition.
        Opcode::ArrayStore => {
            if let Some(ref op) = instr.dest {
                if let Some(v) = operand_to_vreg(op) {
                    uses.push(v);
                }
            }
            if let Some(ref op) = instr.arg1 {
                if let Some(v) = operand_to_vreg(op) {
                    uses.push(v);
                }
            }
            if let Some(ref op) = instr.arg2 {
                if let Some(v) = operand_to_vreg(op) {
                    uses.push(v);
                }
            }
        }

        // ArrayLoad: reads base (arg1) and index (arg2).
        Opcode::ArrayLoad => {
            if let Some(ref op) = instr.arg1 {
                if let Some(v) = operand_to_vreg(op) {
                    uses.push(v);
                }
            }
            if let Some(ref op) = instr.arg2 {
                if let Some(v) = operand_to_vreg(op) {
                    uses.push(v);
                }
            }
        }

        // Load: reads the pointer address (arg1).
        Opcode::Load => {
            if let Some(ref op) = instr.arg1 {
                if let Some(v) = operand_to_vreg(op) {
                    uses.push(v);
                }
            }
        }

        // Store: reads address (arg1) and value (arg2). No def.
        Opcode::Store => {
            if let Some(ref op) = instr.arg1 {
                if let Some(v) = operand_to_vreg(op) {
                    uses.push(v);
                }
            }
            if let Some(ref op) = instr.arg2 {
                if let Some(v) = operand_to_vreg(op) {
                    uses.push(v);
                }
            }
        }

        // AddrOf: arg1 is a Var whose address is taken. Its value is not
        // read, so it is not a use in the dataflow sense.
        Opcode::AddrOf => {}
    }

    uses
}

/// Extract the virtual register DEFINED (written) by a single instruction.
pub fn instruction_def(instr: &TACInstruction) -> Option<VirtualReg> {
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
        | Opcode::Gte
        | Opcode::Mov
        | Opcode::Call
        | Opcode::GetParam
        | Opcode::ArrayLoad
        | Opcode::Load
        | Opcode::AddrOf => instr.dest.as_ref().and_then(operand_to_vreg),

        // ArrayStore and Store do NOT define their dest -- they write to
        // memory, not to a register.
        Opcode::Jump
        | Opcode::BranchIf
        | Opcode::BranchIfNot
        | Opcode::Param
        | Opcode::Ret
        | Opcode::ArrayStore
        | Opcode::Store => None,
    }
}

// ### Iterative backward dataflow ###

/// Compute the upward-exposed uses and KILL (definitions) sets
/// for a basic block.
fn block_gen_kill(block: &BasicBlock) -> (BTreeSet<VirtualReg>, BTreeSet<VirtualReg>) {
    let mut upward_exposed = BTreeSet::new();
    let mut kill = BTreeSet::new();

    for instr in &block.instructions {
        // Uses that haven't been killed yet are upward-exposed.
        for vreg in instruction_uses(instr) {
            if !kill.contains(&vreg) {
                upward_exposed.insert(vreg);
            }
        }
        // Definitions go into the KILL set.
        if let Some(vreg) = instruction_def(instr) {
            kill.insert(vreg);
        }
    }

    (upward_exposed, kill)
}

/// Perform iterative backward dataflow to compute LIVE_IN and LIVE_OUT
/// for every basic block.
///
/// Equations:
///   LIVE_OUT[B] = ∪ { LIVE_IN[S] : S ∈ successors(B) }
///   LIVE_IN[B]  = GEN[B] ∪ (LIVE_OUT[B] − KILL[B])
fn compute_block_liveness(
    cfg: &CFG,
) -> (
    HashMap<String, BTreeSet<VirtualReg>>, // LIVE_IN
    HashMap<String, BTreeSet<VirtualReg>>, // LIVE_OUT
) {
    // Pre-compute upward-exposed uses and KILL for every block.
    let mut use_map: HashMap<String, BTreeSet<VirtualReg>> = HashMap::new();
    let mut kill_map: HashMap<String, BTreeSet<VirtualReg>> = HashMap::new();

    for (label, block) in &cfg.blocks {
        let (upward_exposed, kill) = block_gen_kill(block);
        use_map.insert(label.clone(), upward_exposed);
        kill_map.insert(label.clone(), kill);
    }

    // Initialize LIVE_IN and LIVE_OUT to empty sets.
    let mut live_in: HashMap<String, BTreeSet<VirtualReg>> = cfg
        .blocks
        .keys()
        .map(|l| (l.clone(), BTreeSet::new()))
        .collect();
    let mut live_out: HashMap<String, BTreeSet<VirtualReg>> = cfg
        .blocks
        .keys()
        .map(|l| (l.clone(), BTreeSet::new()))
        .collect();

    // Iterate until fixed point.  Processing in reverse order
    // (approximating reverse postorder) speeds convergence.
    let mut changed = true;
    while changed {
        changed = false;

        for label in cfg.blocks.keys().rev() {
            let block = &cfg.blocks[label];

            // LIVE_OUT[B] = ∪ LIVE_IN[S]  for all successors S
            let mut new_out = BTreeSet::new();
            for succ in &block.successors {
                if let Some(succ_in) = live_in.get(succ) {
                    new_out.extend(succ_in.iter().cloned());
                }
            }

            // LIVE_IN[B] = USE[B] ∪ (LIVE_OUT[B] − KILL[B])
            let uses = &use_map[label];
            let kill = &kill_map[label];
            let mut new_in = uses.clone();
            for vreg in &new_out {
                if !kill.contains(vreg) {
                    new_in.insert(vreg.clone());
                }
            }

            if new_in != live_in[label] || new_out != live_out[label] {
                changed = true;
                live_in.insert(label.clone(), new_in);
                live_out.insert(label.clone(), new_out);
            }
        }
    }

    (live_in, live_out)
}

// ### Phase 4: Build live intervals  (Poletto & Sarkar algorithm) ###

/// Compute the live interval `[start, end]` for every virtual register.
///
/// Algorithm (from "Linear Scan Register Allocation", Poletto & Sarkar 1999):
///
/// For each block B (in reverse linearized order):
///   1. `live` ← LIVE_OUT[B]
///   2. For each vreg in `live`: extend interval to cover [B.start, B.end].
///   3. Walk instructions from last to first:
///      - For each DEF: set interval start to this index (vreg born here).
///        Remove from `live`.
///      - For each USE: extend interval to [B.start, instr_index].
///        Add to `live`.
///
/// The output is sorted by start point (required by linear scan).
pub fn compute_live_intervals(cfg: &CFG, linear: &LinearizedCfg) -> Vec<LiveInterval> {
    let (_, live_out) = compute_block_liveness(cfg);

    // Cumulative (start, end) per virtual register.
    let mut intervals: HashMap<VirtualReg, (usize, usize)> = HashMap::new();

    // Helper: extend or create an interval to include [from, to].
    fn extend(
        map: &mut HashMap<VirtualReg, (usize, usize)>,
        vreg: &VirtualReg,
        from: usize,
        to: usize,
    ) {
        map.entry(vreg.clone())
            .and_modify(|(s, e)| {
                *s = (*s).min(from);
                *e = (*e).max(to);
            })
            .or_insert((from, to));
    }

    // Process blocks in reverse linearized order.
    for label in linear.block_order.iter().rev() {
        let Some(&(block_start, block_end)) = linear.block_ranges.get(label) else {
            continue; // Empty block.
        };

        // vregs live-out span the entire block.
        if let Some(lo) = live_out.get(label) {
            for vreg in lo {
                extend(&mut intervals, vreg, block_start, block_end);
            }
        }

        //  walk instructions in reverse.
        for idx in (block_start..=block_end).rev() {
            let (ref instr, _) = linear.instructions[idx];

            // DEF: the vreg is born here — tighten start.
            if let Some(ref def_vreg) = instruction_def(instr) {
                intervals
                    .entry(def_vreg.clone())
                    .and_modify(|(s, _)| *s = idx)
                    .or_insert((idx, idx));
            }

            // USE: vreg must be live from block_start to this instruction.
            for use_vreg in instruction_uses(instr) {
                extend(&mut intervals, &use_vreg, block_start, idx);
            }
        }
    }

    // Sort by start point (primary), then end point (secondary).
    let mut result: Vec<LiveInterval> = intervals
        .into_iter()
        .map(|(vreg, (start, end))| LiveInterval { vreg, start, end })
        .collect();

    result.sort_by(|a, b| a.start.cmp(&b.start).then(a.end.cmp(&b.end)));
    result
}
