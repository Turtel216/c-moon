//! Def-use chains: every place a value is read.
//!
//! A value is written once, so its definition needs no index; what a pass
//! needs is the other direction, and that is what this builds.
//!
//! # Recomputed, never maintained
//!
//! Chains are built from scratch by [`DefUse::compute`] whenever a pass wants
//! them, rather than kept up to date as the function is edited.  Building them
//! is one walk over the function, and these functions are small.  A chain the
//! passes maintain by hand would be a second copy of the truth, and it is a
//! copy the verifier cannot check: a stale chain does not make the IR invalid,
//! it just makes the next pass quietly wrong.

use std::collections::BTreeMap;

use super::{BlockId, Function, InstId, Operand, ValueId};

/// Where in a block a value is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsePosition {
    /// Argument `argument` of the phi at position `phi`.
    ///
    /// A phi argument is not really read in the phi's own block: it is read at
    /// the end of the predecessor it arrives from, which is the predecessor at
    /// the same index.
    Phi { phi: usize, argument: usize },
    /// An operand of an instruction, `order` places into the block.
    Inst { inst: InstId, order: usize },
    /// An operand of the block's terminator, which comes after everything
    /// else in the block.
    Terminator { order: usize },
}

/// One place a value is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Use {
    /// The block the read happens in.
    pub block: BlockId,
    /// Where within that block.
    pub position: UsePosition,
}

/// Every read of every value in a function.
#[derive(Debug, Clone, Default)]
pub struct DefUse {
    /// Reads of each value, in the order the walk found them.  A `BTreeMap`
    /// rather than a hash map so that a pass iterating over it -- and any
    /// message it produces -- comes out the same way every run.
    uses: BTreeMap<ValueId, Vec<Use>>,
}

impl DefUse {
    /// Walk `function` and record every read.
    pub fn compute(function: &Function) -> Self {
        let mut chains = Self::default();

        for block in function.block_ids() {
            let body = function.block(block);

            for (phi, node) in body.phis.iter().enumerate() {
                for (argument, &operand) in node.args.iter().enumerate() {
                    chains.record(operand, block, UsePosition::Phi { phi, argument });
                }
            }

            for (order, &inst) in body.insts.iter().enumerate() {
                for operand in function.inst(inst).op.operands() {
                    chains.record(
                        operand,
                        block,
                        UsePosition::Inst {
                            inst,
                            order: order + 1,
                        },
                    );
                }
            }

            let order = body.insts.len() + 1;
            for operand in body.terminator().operands() {
                chains.record(operand, block, UsePosition::Terminator { order });
            }
        }

        chains
    }

    /// Note that `operand` is read at this place, if it reads a value at all.
    fn record(&mut self, operand: Operand, block: BlockId, position: UsePosition) {
        if let Operand::Value(value) = operand {
            self.uses
                .entry(value)
                .or_default()
                .push(Use { block, position });
        }
    }

    /// Where `value` is read.
    pub fn uses_of(&self, value: ValueId) -> &[Use] {
        self.uses.get(&value).map_or(&[], Vec::as_slice)
    }

    /// Every value that is read anywhere, with its reads, in value order.
    pub fn all(&self) -> impl Iterator<Item = (ValueId, &[Use])> {
        self.uses
            .iter()
            .map(|(&value, uses)| (value, uses.as_slice()))
    }
}
