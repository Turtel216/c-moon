//! The SSA verifier.
//!
//! Everything the rest of the middle-end is allowed to assume about a
//! [`Function`] is checked here, on the principle that an invariant nothing
//! checks is an invariant that is already broken somewhere.  It runs after
//! every pass in a debug build and in every test, so a pass that corrupts the
//! IR is caught where it did the damage rather than in the assembly two stages
//! later.
//!
//! Some invariants cannot be broken at all, because the types do not allow it,
//! and so do not appear below: a block has exactly one terminator because it
//! has one `Terminator` field; phis are at the top of a block because they are
//! in a list of their own; a slot cannot appear where a value belongs because
//! the two have different types.  What is left is what a pass can get wrong.

use std::collections::HashMap;
use std::fmt;

use crate::printer::ir_printer::{InstText, ValueText};

use super::dom::{DomTree, Graph};
use super::{BlockId, DefSite, Function, InstId, Op, Operand, ValueId};

/// One broken invariant.
///
/// Every variant carries the text of whatever it is complaining about, not
/// just its index, so that the message can be read without the function in
/// front of you.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SsaError {
    /// A block nothing can reach.  Dominance is undefined for these, so they
    /// have to be deleted rather than tolerated.
    UnreachableBlock { block: String },

    /// Something branches into the entry block.
    EntryHasPredecessors { block: String, preds: Vec<String> },

    /// Two instructions or phis claim to define the same value.
    MultipleDefinitions {
        value: String,
        first: String,
        second: String,
    },

    /// A value's recorded definition site is not where it is defined.
    MisplacedDefinition { value: String, site: String },

    /// A use of a value whose defining instruction is no longer in any block.
    DanglingUse { value: String, used_in: String },

    /// A use that the definition does not dominate.
    UseNotDominated {
        value: String,
        defined_in: String,
        used_in: String,
        at: String,
    },

    /// A phi with the wrong number of arguments.
    PhiArity {
        block: String,
        phi: String,
        args: usize,
        preds: usize,
    },

    /// A phi argument whose definition does not reach the end of the
    /// predecessor it arrives from.
    PhiArgumentNotDominated {
        block: String,
        phi: String,
        value: String,
        predecessor: String,
    },

    /// A block's predecessor list disagrees with the terminators that name it.
    EdgesDisagree {
        block: String,
        recorded: Vec<String>,
        actual: Vec<String>,
    },

    /// An incoming argument read outside the entry block's opening run of
    /// them, which the backend lowers as a single simultaneous assignment.
    ParameterOutOfPrologue { block: String, inst: String },
}

impl fmt::Display for SsaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SsaError::UnreachableBlock { block } => {
                write!(f, "block .{block} is unreachable and was not deleted")
            }
            SsaError::EntryHasPredecessors { block, preds } => write!(
                f,
                "entry block .{block} is branched into, from {}",
                labels(preds)
            ),
            SsaError::MultipleDefinitions {
                value,
                first,
                second,
            } => write!(
                f,
                "{value} is defined twice, by `{first}` and by `{second}`"
            ),
            SsaError::MisplacedDefinition { value, site } => {
                write!(
                    f,
                    "{value} is recorded as defined by {site}, which does not define it"
                )
            }
            SsaError::DanglingUse { value, used_in } => write!(
                f,
                "`{used_in}` reads {value}, whose defining instruction is no longer in the function"
            ),
            SsaError::UseNotDominated {
                value,
                defined_in,
                used_in,
                at,
            } => write!(
                f,
                "use of {value} in .{used_in} is not dominated by its definition in \
                 .{defined_in}\n  used by: {at}"
            ),
            SsaError::PhiArity {
                block,
                phi,
                args,
                preds,
            } => write!(
                f,
                "phi `{phi}` in .{block} has {args} argument(s) but the block has {preds} \
                 predecessor(s)"
            ),
            SsaError::PhiArgumentNotDominated {
                block,
                phi,
                value,
                predecessor,
            } => write!(
                f,
                "phi `{phi}` in .{block} takes {value} from .{predecessor}, which its definition \
                 does not reach"
            ),
            SsaError::EdgesDisagree {
                block,
                recorded,
                actual,
            } => write!(
                f,
                "block .{block} records predecessors {} but is branched into from {}",
                labels(recorded),
                labels(actual)
            ),
            SsaError::ParameterOutOfPrologue { block, inst } => write!(
                f,
                "`{inst}` in .{block} reads an incoming argument outside the entry block's \
                 opening run of them"
            ),
        }
    }
}

impl std::error::Error for SsaError {}

/// Format a list of block labels for a message.
fn labels(blocks: &[String]) -> String {
    if blocks.is_empty() {
        return "nothing".to_string();
    }
    blocks
        .iter()
        .map(|label| format!(".{label}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Check every invariant the middle-end relies on.
///
/// # Returns
///
/// Every broken invariant found, so one run reports as much as it can rather
/// than stopping at the first problem.  Checks that need a well-formed graph --
/// anything involving dominance -- are skipped when the structural checks have
/// already failed, since their answers would be meaningless.
pub fn verify_ssa(function: &Function) -> Result<(), Vec<SsaError>> {
    let mut errors = Vec::new();

    check_reachability(function, &mut errors);
    check_edges(function, &mut errors);
    check_definitions(function, &mut errors);
    check_parameter_prologue(function, &mut errors);

    if errors.is_empty() {
        check_dominance(function, &mut errors);
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Check `function` in a debug build, and do nothing in a release build.
///
/// This is what the pipeline calls after each pass.  `cfg!` rather than
/// `#[cfg]` so that the check is always compiled -- a verifier that only
/// builds in debug mode is a verifier that stops compiling without anyone
/// noticing.
pub fn debug_assert_valid(function: &Function, after: &str) {
    if cfg!(debug_assertions) {
        assert_valid(function, after);
    }
}

/// Panic if `function` breaks any invariant.
///
/// This is what passes call: a broken invariant is a compiler bug, so there is
/// nothing to recover from and the message is the whole point.
///
/// # Panics
///
/// Panics with every broken invariant listed, one per line.
pub fn assert_valid(function: &Function, after: &str) {
    if let Err(errors) = verify_ssa(function) {
        let listed: Vec<String> = errors.iter().map(|error| format!("  {error}")).collect();
        panic!(
            "Compiler Bug: SSA verification failed after {} in function {}:\n{}\n\n{}",
            after,
            function.name,
            listed.join("\n"),
            function
        );
    }
}

/// Every block must be reachable, and nothing may branch into the entry.
fn check_reachability(function: &Function, errors: &mut Vec<SsaError>) {
    let mut reachable = vec![false; function.block_count()];
    let mut stack = vec![function.entry()];
    reachable[function.entry().index()] = true;
    while let Some(block) = stack.pop() {
        for successor in function.block(block).successors() {
            if !reachable[successor.index()] {
                reachable[successor.index()] = true;
                stack.push(successor);
            }
        }
    }

    for block in function.block_ids() {
        if !reachable[block.index()] {
            errors.push(SsaError::UnreachableBlock {
                block: function.block(block).label.clone(),
            });
        }
    }

    let entry = function.block(function.entry());
    if !entry.preds().is_empty() {
        errors.push(SsaError::EntryHasPredecessors {
            block: entry.label.clone(),
            preds: entry
                .preds()
                .iter()
                .map(|&pred| function.block(pred).label.clone())
                .collect(),
        });
    }
}

/// Predecessor lists must agree with the terminators, and every phi must have
/// one argument per predecessor.
fn check_edges(function: &Function, errors: &mut Vec<SsaError>) {
    let mut actual: Vec<Vec<BlockId>> = vec![Vec::new(); function.block_count()];
    for block in function.block_ids() {
        for successor in function.block(block).successors() {
            actual[successor.index()].push(block);
        }
    }

    for block in function.block_ids() {
        let recorded = function.block(block).preds();

        // The order of the two lists is not required to match -- only the
        // multiset is, since a phi argument is aligned with whatever order the
        // block records.
        let mut left: Vec<BlockId> = recorded.to_vec();
        let mut right = actual[block.index()].clone();
        left.sort_unstable();
        right.sort_unstable();
        if left != right {
            errors.push(SsaError::EdgesDisagree {
                block: function.block(block).label.clone(),
                recorded: recorded
                    .iter()
                    .map(|&pred| function.block(pred).label.clone())
                    .collect(),
                actual: actual[block.index()]
                    .iter()
                    .map(|&pred| function.block(pred).label.clone())
                    .collect(),
            });
        }

        for phi in &function.block(block).phis {
            if phi.args.len() != recorded.len() {
                errors.push(SsaError::PhiArity {
                    block: function.block(block).label.clone(),
                    phi: value_text(function, phi.dest),
                    args: phi.args.len(),
                    preds: recorded.len(),
                });
            }
        }
    }
}

/// Every value must be defined exactly once, where it says it is, and every
/// use must read a definition that is still in the function.
fn check_definitions(function: &Function, errors: &mut Vec<SsaError>) {
    // Where each value is actually defined, discovered by walking the blocks
    // rather than by trusting what the value arena records.
    let mut defined: HashMap<ValueId, String> = HashMap::new();

    let mut record = |value: ValueId, text: String, errors: &mut Vec<SsaError>| {
        if let Some(first) = defined.get(&value) {
            errors.push(SsaError::MultipleDefinitions {
                value: value_text(function, value),
                first: first.clone(),
                second: text.clone(),
            });
            return;
        }
        defined.insert(value, text);
    };

    for block in function.block_ids() {
        for (position, phi) in function.block(block).phis.iter().enumerate() {
            let text = format!(
                "phi {} in .{}",
                value_text(function, phi.dest),
                function.block(block).label
            );
            record(phi.dest, text, errors);

            if function.value_def(phi.dest).site != DefSite::Phi(block, position) {
                errors.push(SsaError::MisplacedDefinition {
                    value: value_text(function, phi.dest),
                    site: format!("{:?}", function.value_def(phi.dest).site),
                });
            }
        }

        for &inst in &function.block(block).insts {
            let Some(dest) = function.inst(inst).dest else {
                continue;
            };
            record(dest, inst_text(function, inst), errors);

            if function.value_def(dest).site != DefSite::Inst(inst) {
                errors.push(SsaError::MisplacedDefinition {
                    value: value_text(function, dest),
                    site: format!("{:?}", function.value_def(dest).site),
                });
            }
        }
    }

    for (block, use_site) in uses(function) {
        if let Operand::Value(value) = use_site.operand
            && !defined.contains_key(&value)
        {
            errors.push(SsaError::DanglingUse {
                value: value_text(function, value),
                used_in: use_site.describe(function, block),
            });
        }
    }
}

/// Incoming arguments may only be read by an unbroken run at the top of the
/// entry block, which is how the backend lowers them.
fn check_parameter_prologue(function: &Function, errors: &mut Vec<SsaError>) {
    for block in function.block_ids() {
        let is_entry = block == function.entry();
        let mut in_prologue = is_entry;

        for &inst in &function.block(block).insts {
            let is_parameter = matches!(function.inst(inst).op, Op::GetParam(_));
            if is_parameter && !in_prologue {
                errors.push(SsaError::ParameterOutOfPrologue {
                    block: function.block(block).label.clone(),
                    inst: inst_text(function, inst),
                });
            }
            in_prologue &= is_parameter;
        }
    }
}

/// Every use must be dominated by its definition.
fn check_dominance(function: &Function, errors: &mut Vec<SsaError>) {
    let (predecessors, successors) = function.adjacency();
    let graph = Graph::new(function.entry(), &predecessors, &successors);
    let tree = DomTree::build(graph);

    // Where each value is defined, and how far into that block: a phi counts
    // as position zero, so it precedes every instruction of its block.
    let mut site: HashMap<ValueId, (BlockId, usize)> = HashMap::new();
    for block in function.block_ids() {
        for phi in &function.block(block).phis {
            site.insert(phi.dest, (block, 0));
        }
        for (position, &inst) in function.block(block).insts.iter().enumerate() {
            if let Some(dest) = function.inst(inst).dest {
                site.insert(dest, (block, position + 1));
            }
        }
    }

    for (block, use_site) in uses(function) {
        let Operand::Value(value) = use_site.operand else {
            continue;
        };
        let Some(&(defined_in, defined_at)) = site.get(&value) else {
            continue; // already reported as a dangling use
        };

        match use_site.position {
            // A phi argument is not used in the phi's own block: it is used at
            // the end of the predecessor it arrives from, which is the block
            // the definition has to reach.
            UsePosition::PhiArgument { phi, predecessor } => {
                if !tree.dominates(defined_in, predecessor) {
                    errors.push(SsaError::PhiArgumentNotDominated {
                        block: function.block(block).label.clone(),
                        phi: value_text(function, phi),
                        value: value_text(function, value),
                        predecessor: function.block(predecessor).label.clone(),
                    });
                }
            }

            UsePosition::Inst { order, .. } | UsePosition::Terminator { order } => {
                let dominated = if defined_in == block {
                    defined_at < order
                } else {
                    tree.dominates(defined_in, block)
                };
                if !dominated {
                    errors.push(SsaError::UseNotDominated {
                        value: value_text(function, value),
                        defined_in: function.block(defined_in).label.clone(),
                        used_in: function.block(block).label.clone(),
                        at: use_site.describe(function, block),
                    });
                }
            }
        }
    }
}

/// Where in a block an operand is read.
#[derive(Debug, Clone, Copy)]
enum UsePosition {
    /// Argument of a phi, arriving from `predecessor`.
    PhiArgument { phi: ValueId, predecessor: BlockId },
    /// Operand of an instruction, `order` places into the block.
    Inst { inst: InstId, order: usize },
    /// Operand of the block's terminator, which is last.
    Terminator { order: usize },
}

/// One place a value is read.
#[derive(Debug, Clone, Copy)]
struct Use {
    operand: Operand,
    position: UsePosition,
}

impl Use {
    /// The text a message should quote for this use.
    fn describe(&self, function: &Function, block: BlockId) -> String {
        match self.position {
            UsePosition::PhiArgument { phi, .. } => {
                format!("phi {}", value_text(function, phi))
            }
            UsePosition::Inst { inst, .. } => inst_text(function, inst),
            UsePosition::Terminator { .. } => {
                format!("the transfer ending .{}", function.block(block).label)
            }
        }
    }
}

/// Every read of an operand in the function, with the block it happens in.
fn uses(function: &Function) -> Vec<(BlockId, Use)> {
    let mut found = Vec::new();

    for block in function.block_ids() {
        let body = function.block(block);

        for phi in &body.phis {
            for (position, &argument) in phi.args.iter().enumerate() {
                // A phi argument with no matching predecessor is reported by
                // the arity check; there is no edge to blame it on here.
                let Some(&predecessor) = body.preds().get(position) else {
                    continue;
                };
                found.push((
                    block,
                    Use {
                        operand: argument,
                        position: UsePosition::PhiArgument {
                            phi: phi.dest,
                            predecessor,
                        },
                    },
                ));
            }
        }

        for (position, &inst) in body.insts.iter().enumerate() {
            for operand in function.inst(inst).op.operands() {
                found.push((
                    block,
                    Use {
                        operand,
                        position: UsePosition::Inst {
                            inst,
                            order: position + 1,
                        },
                    },
                ));
            }
        }

        let order = body.insts.len() + 1;
        for operand in body.terminator().operands() {
            found.push((
                block,
                Use {
                    operand,
                    position: UsePosition::Terminator { order },
                },
            ));
        }
    }

    found
}

/// One value, as a message should refer to it.
fn value_text(function: &Function, value: ValueId) -> String {
    ValueText { function, value }.to_string()
}

/// One instruction, as a message should quote it.
fn inst_text(function: &Function, inst: InstId) -> String {
    InstText { function, inst }.to_string()
}
