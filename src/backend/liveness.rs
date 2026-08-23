//! Liveness analysis.
//!
//! Computes the live interval of every virtual register: the range of
//! instruction indices over which it holds a value that is still needed.
//! Linear-scan allocation consumes nothing else.
//!
//! The analysis runs in three steps:
//! 1. Per-block USE/KILL sets -- the values a block reads before writing,
//!    and the values it writes.
//! 2. A backward dataflow fixed point over the CFG, giving LIVE_IN/LIVE_OUT
//!    for every block.
//! 3. A reverse walk over the linearized instructions that turns per-block
//!    liveness into intervals (Poletto & Sarkar, 1999).

use std::collections::{BTreeSet, HashMap};

use crate::backend::linear::LinearizedCfg;
use crate::backend::vreg::{VirtualReg, instruction_def, instruction_uses};
use crate::middle::ir::{BasicBlock, CFG, Opcode};

/// The range of instruction indices over which a virtual register is live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveInterval {
    pub vreg: VirtualReg,
    /// First instruction index where the value is live (inclusive).
    pub start: usize,
    /// Last instruction index where the value is live (inclusive).
    pub end: usize,
}

impl LiveInterval {
    /// Returns `true` if this value has to survive `call`, i.e. it is live on
    /// both sides of it.
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

/// A call in the linearized instruction sequence.
///
/// The allocator needs these to know which values are held across a call and
/// must therefore avoid caller-saved registers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallSite {
    /// Index of the call in the linearized instruction sequence.
    pub position: usize,
    /// The virtual register the call writes its return value into, if any.
    pub defines: Option<VirtualReg>,
}

/// Collect every call site of a function, in ascending position order, which
/// the allocator's binary search over call sites relies on.
pub fn find_call_sites(linear: &LinearizedCfg) -> Vec<CallSite> {
    linear
        .instructions()
        .iter()
        .enumerate()
        .filter(|(_, instr)| instr.opcode == Opcode::Call)
        .map(|(position, instr)| CallSite {
            position,
            defines: instruction_def(instr),
        })
        .collect()
}

/// Compute the live interval of every virtual register in a function.
///
/// For each block, in reverse linearization order:
/// 1. Values live out of the block span the whole block.
/// 2. Walking its instructions backwards, a definition moves the interval's
///    start to that index (the value is born there) and a use extends the
///    interval back to the start of the block.
///
/// # Returns
///
/// The intervals sorted by ascending start point, then end point -- the order
/// [`linear_scan`](crate::backend::regalloc::linear_scan) requires.
pub fn compute_live_intervals(cfg: &CFG, linear: &LinearizedCfg) -> Vec<LiveInterval> {
    let live_out = compute_live_out(cfg);

    // Accumulated `(start, end)` per virtual register.
    let mut intervals: HashMap<VirtualReg, (usize, usize)> = HashMap::new();

    /// Widen `vreg`'s interval to cover `[from, to]`, creating it if needed.
    fn extend(
        intervals: &mut HashMap<VirtualReg, (usize, usize)>,
        vreg: &VirtualReg,
        from: usize,
        to: usize,
    ) {
        intervals
            .entry(vreg.clone())
            .and_modify(|(start, end)| {
                *start = (*start).min(from);
                *end = (*end).max(to);
            })
            .or_insert((from, to));
    }

    for block in linear.blocks().iter().rev() {
        if block.range.is_empty() {
            continue;
        }
        let (block_start, block_end) = (block.range.start, block.range.end - 1);

        // A value live out of the block is live across all of it.
        if let Some(live) = live_out.get(&block.label) {
            for vreg in live {
                extend(&mut intervals, vreg, block_start, block_end);
            }
        }

        for index in block.range.clone().rev() {
            let instr = &linear.instructions()[index];

            // A definition is where the value comes into existence, so the
            // interval starts here however far back a later use reached.
            if let Some(defined) = instruction_def(instr) {
                intervals
                    .entry(defined)
                    .and_modify(|(start, _)| *start = index)
                    .or_insert((index, index));
            }

            // A use must be live from the top of the block: the value may
            // have been produced by any predecessor.
            for used in instruction_uses(instr) {
                extend(&mut intervals, &used, block_start, index);
            }
        }
    }

    let mut result: Vec<LiveInterval> = intervals
        .into_iter()
        .map(|(vreg, (start, end))| LiveInterval { vreg, start, end })
        .collect();

    result.sort_unstable_by(|a, b| a.start.cmp(&b.start).then(a.end.cmp(&b.end)));
    result
}

/// The values a block reads before writing them (USE) and the values it
/// writes (KILL).
fn block_use_kill(block: &BasicBlock) -> (BTreeSet<VirtualReg>, BTreeSet<VirtualReg>) {
    let mut used = BTreeSet::new();
    let mut killed = BTreeSet::new();

    for instr in &block.instructions {
        // Only a use that no earlier definition in this block satisfies is
        // exposed to the block's predecessors.
        for vreg in instruction_uses(instr) {
            if !killed.contains(&vreg) {
                used.insert(vreg);
            }
        }
        if let Some(defined) = instruction_def(instr) {
            killed.insert(defined);
        }
    }

    (used, killed)
}

/// Solve the backward liveness dataflow equations to a fixed point:
///
/// ```text
/// LIVE_OUT[B] = union over successors S of LIVE_IN[S]
/// LIVE_IN[B]  = USE[B] union (LIVE_OUT[B] - KILL[B])
/// ```
///
/// # Returns
///
/// LIVE_OUT per block label; LIVE_IN is an implementation detail of the
/// iteration and is not needed by interval construction.
fn compute_live_out(cfg: &CFG) -> HashMap<String, BTreeSet<VirtualReg>> {
    let use_kill: HashMap<&str, (BTreeSet<VirtualReg>, BTreeSet<VirtualReg>)> = cfg
        .blocks
        .iter()
        .map(|(label, block)| (label.as_str(), block_use_kill(block)))
        .collect();

    let mut live_in: HashMap<&str, BTreeSet<VirtualReg>> = HashMap::with_capacity(use_kill.len());
    let mut live_out: HashMap<String, BTreeSet<VirtualReg>> =
        HashMap::with_capacity(use_kill.len());

    // Iterate until nothing changes.  Visiting blocks in reverse label order
    // approximates reverse postorder, which converges in fewer rounds.
    let mut changed = true;
    while changed {
        changed = false;

        for (label, block) in cfg.blocks.iter().rev() {
            let mut new_out = BTreeSet::new();
            for successor in &block.successors {
                if let Some(successor_in) = live_in.get(successor.as_str()) {
                    new_out.extend(successor_in.iter().cloned());
                }
            }

            let (used, killed) = &use_kill[label.as_str()];
            let mut new_in = used.clone();
            new_in.extend(new_out.difference(killed).cloned());

            if live_in.get(label.as_str()) != Some(&new_in) {
                changed = true;
                live_in.insert(label.as_str(), new_in);
            }
            if live_out.get(label) != Some(&new_out) {
                changed = true;
                live_out.insert(label.clone(), new_out);
            }
        }
    }

    live_out
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::backend::linear::linearize_cfg;
    use crate::middle::ir::{BasicBlock, Operand, TACInstruction, Width};

    /// A straight-line function: `t1 = 1; x = t1; ret x`.
    fn straight_line_cfg() -> CFG {
        let mut cfg = CFG::new("entry".to_string(), "exit".to_string());
        let mut entry = BasicBlock::new("entry".to_string());
        entry.instructions = vec![
            TACInstruction::new(
                Opcode::Mov,
                Width::Bits64,
                Some(Operand::Temp("t1".to_string())),
                Some(Operand::ImmInt(1)),
                None,
            ),
            TACInstruction::new(
                Opcode::Mov,
                Width::Bits64,
                Some(Operand::Var(0)),
                Some(Operand::Temp("t1".to_string())),
                None,
            ),
            TACInstruction::new(
                Opcode::Ret,
                Width::Bits64,
                None,
                Some(Operand::Var(0)),
                None,
            ),
        ];
        cfg.add_block(entry);
        cfg.add_block(BasicBlock::new("exit".to_string()));
        cfg
    }

    fn interval_of(intervals: &[LiveInterval], vreg: VirtualReg) -> &LiveInterval {
        intervals
            .iter()
            .find(|interval| interval.vreg == vreg)
            .expect("value has no live interval")
    }

    #[test]
    fn an_interval_starts_at_the_definition_and_ends_at_the_last_use() {
        // Arrange
        let cfg = straight_line_cfg();
        let linear = linearize_cfg(&cfg);

        // Act
        let intervals = compute_live_intervals(&cfg, &linear);

        // Assert
        let temp = interval_of(&intervals, VirtualReg::Temp("t1".to_string()));
        assert_eq!((temp.start, temp.end), (0, 1));
        let var = interval_of(&intervals, VirtualReg::Var(0));
        assert_eq!((var.start, var.end), (1, 2));
    }

    #[test]
    fn intervals_come_out_sorted_by_start_point() {
        // Arrange / Act
        let cfg = straight_line_cfg();
        let intervals = compute_live_intervals(&cfg, &linearize_cfg(&cfg));

        // Assert
        assert!(
            intervals
                .windows(2)
                .all(|pair| pair[0].start <= pair[1].start),
            "linear scan requires intervals sorted by start point"
        );
    }

    #[test]
    fn a_value_live_across_a_loop_back_edge_spans_the_whole_loop() {
        // Arrange: `x` is defined before the loop and used inside it, so the
        // back edge keeps it live over the body.
        let mut cfg = CFG::new("entry".to_string(), "exit".to_string());
        let mut entry = BasicBlock::new("entry".to_string());
        entry.instructions = vec![TACInstruction::new(
            Opcode::Mov,
            Width::Bits64,
            Some(Operand::Var(0)),
            Some(Operand::ImmInt(0)),
            None,
        )];
        let mut body = BasicBlock::new("body".to_string());
        body.instructions = vec![
            TACInstruction::new(
                Opcode::Add,
                Width::Bits64,
                Some(Operand::Var(0)),
                Some(Operand::Var(0)),
                Some(Operand::ImmInt(1)),
            ),
            TACInstruction::new(
                Opcode::Jump,
                Width::Bits64,
                None,
                Some(Operand::Label("body".to_string())),
                None,
            ),
        ];
        cfg.add_block(entry);
        cfg.add_block(body);
        cfg.add_block(BasicBlock::new("exit".to_string()));
        cfg.add_edge("entry", "body");
        cfg.add_edge("body", "body");

        // Act
        let linear = linearize_cfg(&cfg);
        let intervals = compute_live_intervals(&cfg, &linear);

        // Assert: live from its definition to the end of the loop body.
        let var = interval_of(&intervals, VirtualReg::Var(0));
        assert_eq!((var.start, var.end), (0, 2));
    }

    #[test]
    fn call_sites_are_found_in_ascending_order_with_their_destination() {
        // Arrange
        let mut cfg = CFG::new("entry".to_string(), "exit".to_string());
        let mut entry = BasicBlock::new("entry".to_string());
        entry.instructions = vec![
            TACInstruction::new(
                Opcode::Call,
                Width::Bits64,
                Some(Operand::Temp("t1".to_string())),
                Some(Operand::Label("f".to_string())),
                Some(Operand::ImmInt(0)),
            ),
            TACInstruction::new(
                Opcode::Call,
                Width::Bits64,
                None,
                Some(Operand::Label("g".to_string())),
                Some(Operand::ImmInt(0)),
            ),
        ];
        cfg.add_block(entry);
        cfg.add_block(BasicBlock::new("exit".to_string()));

        // Act
        let calls = find_call_sites(&linearize_cfg(&cfg));

        // Assert
        assert_eq!(
            calls,
            vec![
                CallSite {
                    position: 0,
                    defines: Some(VirtualReg::Temp("t1".to_string())),
                },
                CallSite {
                    position: 1,
                    defines: None,
                },
            ]
        );
    }
}
