//! CFG linearization.
//!
//! Liveness analysis and linear-scan allocation both need a single total
//! order over a function's instructions, with one global index per
//! instruction.  This module flattens the control-flow graph into that order
//! once, and every later stage refers to instructions by index.

use std::collections::HashSet;
use std::ops::Range;

use crate::middle::ir::{CFG, TACInstruction};

/// One basic block's slice of a [`LinearizedCfg`].
#[derive(Debug, Clone)]
pub struct LinearBlock {
    /// The block's label in the IR, without any target-specific prefix.
    pub label: String,
    /// Half-open range of indices into the linearized instruction sequence.
    /// Empty for a block that carries no instructions.
    pub range: Range<usize>,
}

/// A function's instructions in a single total order.
#[derive(Debug, Clone)]
pub struct LinearizedCfg {
    instructions: Vec<TACInstruction>,
    blocks: Vec<LinearBlock>,
}

impl LinearizedCfg {
    /// Every instruction of the function; the index into this slice is the
    /// instruction's global position, the unit live intervals are measured in.
    pub fn instructions(&self) -> &[TACInstruction] {
        &self.instructions
    }

    /// The blocks in emission order.  Blocks that carry instructions come
    /// first, in linearization order, and empty ones last -- see
    /// [`linearize_cfg`] for why the order matters.
    pub fn blocks(&self) -> &[LinearBlock] {
        &self.blocks
    }

    /// The instructions belonging to `block`.
    pub fn body(&self, block: &LinearBlock) -> &[TACInstruction] {
        &self.instructions[block.range.clone()]
    }
}

/// Flatten a CFG into a linear instruction sequence.
///
/// Blocks are visited in depth-first preorder from the entry block, which
/// keeps a block near its predecessor and so keeps live intervals short.
/// Blocks unreachable from the entry are appended so that jumps to them
/// still resolve.
///
/// Empty blocks are moved to the very end.  A block with no instructions
/// contributes only its label, and control that reaches the label falls
/// through to whatever follows it; placing such labels last means they fall
/// into the function epilogue, which is exactly what the exit block wants.
pub fn linearize_cfg(cfg: &CFG) -> LinearizedCfg {
    let visit_order = depth_first_order(cfg);

    let mut instructions = Vec::new();
    let mut blocks = Vec::with_capacity(visit_order.len());
    let mut empty_blocks = Vec::new();

    for label in visit_order {
        let body = cfg
            .blocks
            .get(&label)
            .map(|block| block.instructions.as_slice())
            .unwrap_or_default();

        let start = instructions.len();
        instructions.extend(body.iter().cloned());
        let block = LinearBlock {
            label,
            range: start..instructions.len(),
        };

        if block.range.is_empty() {
            empty_blocks.push(block);
        } else {
            blocks.push(block);
        }
    }

    blocks.append(&mut empty_blocks);

    LinearizedCfg {
        instructions,
        blocks,
    }
}

/// The block labels in depth-first preorder from the entry, followed by any
/// block the entry cannot reach.
fn depth_first_order(cfg: &CFG) -> Vec<String> {
    let mut order = Vec::with_capacity(cfg.blocks.len());
    let mut visited = HashSet::with_capacity(cfg.blocks.len());
    let mut stack = vec![cfg.entry.clone()];

    while let Some(label) = stack.pop() {
        // `insert` returns false when the label was already visited, which
        // is how a DFS over a graph avoids looping.
        if !visited.insert(label.clone()) {
            continue;
        }
        order.push(label.clone());

        if let Some(block) = cfg.blocks.get(&label) {
            // Pushed in reverse so the first successor is popped first.
            for successor in block.successors.iter().rev() {
                if !visited.contains(successor) {
                    stack.push(successor.clone());
                }
            }
        }
    }

    order.extend(
        cfg.blocks
            .keys()
            .filter(|label| !visited.contains(*label))
            .cloned(),
    );
    order
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::middle::ir::{BasicBlock, Opcode, Operand};

    /// A CFG shaped like an `if`/`else`: the two arms both fall through to an
    /// empty exit block.
    fn branching_cfg() -> CFG {
        let mut cfg = CFG::new("entry".to_string(), "exit".to_string());
        for label in ["entry", "then", "else", "exit"] {
            cfg.add_block(BasicBlock::new(label.to_string()));
        }
        for (from, to) in [
            ("entry", "then"),
            ("entry", "else"),
            ("then", "exit"),
            ("else", "exit"),
        ] {
            cfg.add_edge(from, to);
        }
        for label in ["entry", "then", "else"] {
            cfg.blocks
                .get_mut(label)
                .expect("block was just added")
                .instructions
                .push(TACInstruction::new(
                    Opcode::Mov,
                    Some(Operand::Var(0)),
                    Some(Operand::ImmInt(1)),
                    None,
                ));
        }
        cfg
    }

    #[test]
    fn instructions_are_indexed_by_their_global_position() {
        // Arrange / Act
        let linear = linearize_cfg(&branching_cfg());

        // Assert: three blocks of one instruction each.
        assert_eq!(linear.instructions().len(), 3);
        for (index, block) in linear.blocks().iter().take(3).enumerate() {
            assert_eq!(block.range, index..index + 1);
            assert_eq!(linear.body(block).len(), 1);
        }
    }

    #[test]
    fn empty_blocks_are_emitted_last_so_they_fall_into_the_epilogue() {
        // Arrange / Act
        let linear = linearize_cfg(&branching_cfg());

        // Assert: `exit` is empty and must not land between the two arms.
        let labels: Vec<&str> = linear
            .blocks()
            .iter()
            .map(|block| block.label.as_str())
            .collect();
        assert_eq!(labels, vec!["entry", "then", "else", "exit"]);
    }

    #[test]
    fn blocks_unreachable_from_the_entry_are_still_linearized() {
        // Arrange: an orphan block, as unreachable-code elimination can leave.
        let mut cfg = branching_cfg();
        let mut orphan = BasicBlock::new("orphan".to_string());
        orphan.instructions.push(TACInstruction::new(
            Opcode::Jump,
            None,
            Some(Operand::Label("exit".to_string())),
            None,
        ));
        cfg.add_block(orphan);

        // Act
        let linear = linearize_cfg(&cfg);

        // Assert
        assert!(
            linear.blocks().iter().any(|block| block.label == "orphan"),
            "an unreachable block still needs its label emitted"
        );
    }
}
