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

    /// An edge from a block that branches into a block with several
    /// predecessors.  Phi nodes on such an edge have nowhere to be lowered to.
    CriticalEdge { from: String, to: String },
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
            SsaError::CriticalEdge { from, to } => write!(
                f,
                "the edge .{from} -> .{to} is critical: .{from} branches and .{to} is entered \
                 from elsewhere too"
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
    check_critical_edges(function, &mut errors);

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
                    site: site_text(function, function.value_def(phi.dest).site),
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
                    site: site_text(function, function.value_def(dest).site),
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

/// No edge may leave a block that branches and arrive at a block that is
/// entered from somewhere else as well.
fn check_critical_edges(function: &Function, errors: &mut Vec<SsaError>) {
    for block in function.block_ids() {
        let successors: Vec<BlockId> = function.block(block).successors().collect();
        if successors.len() < 2 {
            continue;
        }
        for successor in successors {
            if function.block(successor).preds().len() > 1 {
                errors.push(SsaError::CriticalEdge {
                    from: function.block(block).label.clone(),
                    to: function.block(successor).label.clone(),
                });
            }
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

/// The place a value claims to be defined, as a message should describe it.
fn site_text(function: &Function, site: DefSite) -> String {
    match site {
        DefSite::Inst(inst) => format!("`{}`", inst_text(function, inst)),
        DefSite::Phi(block, position) => format!(
            "the phi at position {} of .{}",
            position,
            function.block(block).label
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::middle::ssa::{Op, SlotOrigin, Terminator};

    /// A function with an entry block and nothing in it.
    fn function() -> Function {
        Function::new("f".to_string(), "entry".to_string(), "exit".to_string())
    }

    /// The errors `function` is reported with, failing the test if it is valid.
    ///
    /// The functions below are broken on purpose, by reaching past the API
    /// that normally makes these invariants unbreakable -- which is only
    /// possible from inside this module, and is the point: a pass cannot do
    /// this by accident, but a pass can still corrupt what the API does not
    /// police.
    fn errors(function: &Function) -> Vec<SsaError> {
        verify_ssa(function).expect_err("this function is meant to be broken")
    }

    /// Fail unless one of the reported errors is the expected one.
    macro_rules! assert_reports {
        ($function:expr, $pattern:pat) => {{
            let found = errors($function);
            assert!(
                found.iter().any(|error| matches!(error, $pattern)),
                "expected {} among the reported errors, got {:#?}",
                stringify!($pattern),
                found
            );
        }};
    }

    /// `entry` branches to two blocks that join again: the smallest valid
    /// function with more than one path through it.
    fn diamond() -> (Function, BlockId, BlockId, BlockId, BlockId) {
        let mut function = function();
        let entry = function.entry();
        let left = function.add_block("left".to_string());
        let right = function.add_block("right".to_string());
        let join = function.add_block("join".to_string());

        function.set_terminator(left, Terminator::Jump(join));
        function.set_terminator(right, Terminator::Jump(join));
        function.set_terminator(
            entry,
            Terminator::Branch {
                cond: Operand::Imm(1),
                then_block: left,
                else_block: right,
            },
        );
        function.set_terminator(join, Terminator::Return(None));

        (function, entry, left, right, join)
    }

    #[test]
    fn a_well_formed_function_is_accepted() {
        // Arrange: a diamond whose join reads a phi of the two arms.
        let (mut function, entry, left, right, join) = diamond();
        let slot = function.slot_for(SlotOrigin::Variable(0));
        let from_left = function
            .emit(left, Op::Copy(Operand::Imm(1)))
            .expect("a copy defines a value");
        let from_right = function
            .emit(right, Op::Copy(Operand::Imm(2)))
            .expect("a copy defines a value");

        let merged = function.add_phi(join, slot);
        function.block_mut(join).phis[0].args =
            vec![Operand::Value(from_left), Operand::Value(from_right)];
        function.set_terminator(join, Terminator::Return(Some(Operand::Value(merged))));
        function.emit(entry, Op::Copy(Operand::Imm(0)));

        // Act / Assert
        assert_eq!(verify_ssa(&function), Ok(()));
    }

    #[test]
    fn an_unreachable_block_is_reported() {
        // Arrange: a block nothing branches to, which dominance has no answer
        // for and which construction is supposed to have deleted.
        let mut function = function();
        function.add_block("orphan".to_string());

        // Act / Assert
        assert_reports!(&function, SsaError::UnreachableBlock { .. });
    }

    #[test]
    fn a_branch_back_into_the_entry_block_is_reported() {
        // Arrange: the entry block cannot have predecessors -- a definition
        // there would no longer dominate everything, and it could need phis.
        let mut function = function();
        let entry = function.entry();
        let second = function.add_block("second".to_string());
        function.set_terminator(entry, Terminator::Jump(second));
        function.set_terminator(second, Terminator::Jump(entry));

        // Act / Assert
        assert_reports!(&function, SsaError::EntryHasPredecessors { .. });
    }

    #[test]
    fn a_value_defined_twice_is_reported() {
        // Arrange: two instructions claiming the same result.
        let mut function = function();
        let entry = function.entry();
        let first = function
            .emit(entry, Op::Copy(Operand::Imm(1)))
            .expect("a copy defines a value");
        let second = function
            .emit(entry, Op::Copy(Operand::Imm(2)))
            .expect("a copy defines a value");
        let DefSite::Inst(inst) = function.value_def(second).site else {
            panic!("a copy is defined by an instruction");
        };
        function.inst_mut(inst).dest = Some(first);

        // Act / Assert
        assert_reports!(&function, SsaError::MultipleDefinitions { .. });
    }

    #[test]
    fn a_value_that_is_not_defined_where_it_says_is_reported() {
        // Arrange: the arena and the block disagree about where a value comes
        // from, which is what a pass that moves instructions gets wrong.
        let mut function = function();
        let entry = function.entry();
        let value = function
            .emit(entry, Op::Copy(Operand::Imm(1)))
            .expect("a copy defines a value");
        function.values[value.index()].site = DefSite::Phi(entry, 0);

        // Act / Assert
        assert_reports!(&function, SsaError::MisplacedDefinition { .. });
    }

    #[test]
    fn reading_a_value_whose_definition_was_deleted_is_reported() {
        // Arrange: an instruction removed from its block while something still
        // reads its result -- what an over-eager dead-code pass does.
        let mut function = function();
        let entry = function.entry();
        let value = function
            .emit(entry, Op::Copy(Operand::Imm(1)))
            .expect("a copy defines a value");
        function.emit(entry, Op::Copy(Operand::Value(value)));

        let DefSite::Inst(inst) = function.value_def(value).site else {
            panic!("a copy is defined by an instruction");
        };
        function.remove(entry, inst);

        // Act / Assert
        assert_reports!(&function, SsaError::DanglingUse { .. });
    }

    #[test]
    fn a_use_that_its_definition_does_not_dominate_is_reported() {
        // Arrange: one arm of a diamond reading a value the other arm defines.
        let (mut function, _, left, right, _) = diamond();
        let value = function
            .emit(left, Op::Copy(Operand::Imm(1)))
            .expect("a copy defines a value");
        function.emit(right, Op::Copy(Operand::Value(value)));

        // Act / Assert
        assert_reports!(&function, SsaError::UseNotDominated { .. });
    }

    #[test]
    fn a_use_before_its_definition_in_the_same_block_is_reported() {
        // Arrange: dominance within a block is a question of order.
        let mut function = function();
        let entry = function.entry();
        let later = function
            .emit(entry, Op::Copy(Operand::Imm(1)))
            .expect("a copy defines a value");
        let user = function
            .emit(entry, Op::Copy(Operand::Value(later)))
            .expect("a copy defines a value");

        let (DefSite::Inst(first), DefSite::Inst(second)) = (
            function.value_def(later).site,
            function.value_def(user).site,
        ) else {
            panic!("a copy is defined by an instruction");
        };
        function.block_mut(entry).insts = vec![second, first];

        // Act / Assert
        assert_reports!(&function, SsaError::UseNotDominated { .. });
    }

    #[test]
    fn a_phi_with_the_wrong_number_of_arguments_is_reported() {
        // Arrange: an argument with no predecessor to arrive from.
        let (mut function, _, left, _, join) = diamond();
        let slot = function.slot_for(SlotOrigin::Variable(0));
        let value = function
            .emit(left, Op::Copy(Operand::Imm(1)))
            .expect("a copy defines a value");
        function.add_phi(join, slot);
        function.block_mut(join).phis[0].args = vec![
            Operand::Value(value),
            Operand::Value(value),
            Operand::Imm(0),
        ];

        // Act / Assert
        assert_reports!(&function, SsaError::PhiArity { .. });
    }

    #[test]
    fn a_phi_argument_that_does_not_reach_its_edge_is_reported() {
        // Arrange: the join takes a value from `right` that only `left`
        // defines. The definition dominates neither the phi's own block nor
        // the predecessor the argument arrives from, and it is the second of
        // those that makes it wrong.
        let (mut function, _, left, _, join) = diamond();
        let slot = function.slot_for(SlotOrigin::Variable(0));
        let value = function
            .emit(left, Op::Copy(Operand::Imm(1)))
            .expect("a copy defines a value");
        function.add_phi(join, slot);
        function.block_mut(join).phis[0].args = vec![Operand::Value(value), Operand::Value(value)];

        // Act / Assert
        assert_reports!(&function, SsaError::PhiArgumentNotDominated { .. });
    }

    #[test]
    fn a_phi_argument_is_checked_against_its_predecessor_not_its_own_block() {
        // Arrange: the same shape, but each argument arrives from the arm that
        // defines it. Neither definition dominates the join, so a verifier
        // checking the phi's own block would reject a perfectly good function.
        let (mut function, _, left, right, join) = diamond();
        let slot = function.slot_for(SlotOrigin::Variable(0));
        let from_left = function
            .emit(left, Op::Copy(Operand::Imm(1)))
            .expect("a copy defines a value");
        let from_right = function
            .emit(right, Op::Copy(Operand::Imm(2)))
            .expect("a copy defines a value");
        function.add_phi(join, slot);
        function.block_mut(join).phis[0].args =
            vec![Operand::Value(from_left), Operand::Value(from_right)];

        // Act / Assert
        assert_eq!(verify_ssa(&function), Ok(()));
    }

    #[test]
    fn a_predecessor_list_that_disagrees_with_the_branches_is_reported() {
        // Arrange: a predecessor that does not branch here.
        let (mut function, _, _, _, join) = diamond();
        let stray = function.add_block("stray".to_string());
        function.blocks[join.index()].preds.push(stray);

        // Act / Assert
        assert_reports!(&function, SsaError::EdgesDisagree { .. });
    }

    #[test]
    fn an_incoming_argument_read_out_of_the_prologue_is_reported() {
        // Arrange: the backend lowers the entry block's opening run of
        // `get_param`s as one simultaneous assignment, so one that comes after
        // other work cannot be lowered at all.
        let mut function = function();
        let entry = function.entry();
        function.emit(entry, Op::Copy(Operand::Imm(1)));
        function.emit(entry, Op::GetParam(0));

        // Act / Assert
        assert_reports!(&function, SsaError::ParameterOutOfPrologue { .. });
    }

    #[test]
    fn a_critical_edge_is_reported() {
        // Arrange: `entry` branches, and one arm goes straight to a block that
        // is entered from elsewhere as well.
        let mut function = function();
        let entry = function.entry();
        let other = function.add_block("other".to_string());
        let join = function.add_block("join".to_string());
        function.set_terminator(other, Terminator::Jump(join));
        function.set_terminator(
            entry,
            Terminator::Branch {
                cond: Operand::Imm(1),
                then_block: other,
                else_block: join,
            },
        );
        function.set_terminator(join, Terminator::Return(None));

        // Act / Assert
        assert_reports!(&function, SsaError::CriticalEdge { .. });
    }

    #[test]
    fn a_message_names_the_block_the_instruction_and_the_value() {
        // Arrange: a use its definition does not dominate, which is the error
        // that is hardest to find without being told where to look.
        let (mut function, _, left, right, _) = diamond();
        let value = function
            .emit(left, Op::Copy(Operand::Imm(1)))
            .expect("a copy defines a value");
        function.emit(right, Op::Copy(Operand::Value(value)));

        // Act
        let reported = errors(&function)
            .iter()
            .map(SsaError::to_string)
            .collect::<Vec<_>>()
            .join("\n");

        // Assert: the value, both blocks and the offending instruction are all
        // in the message.
        assert!(reported.contains("%v0"), "{reported}");
        assert!(reported.contains(".left"), "{reported}");
        assert!(reported.contains(".right"), "{reported}");
        assert!(reported.contains("used by: %v1 = %v0"), "{reported}");
    }
}
