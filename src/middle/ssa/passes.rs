//! The optimisation passes, and the order they run in.
//!
//! Every pass takes a function, changes it in place, and reports whether it
//! changed anything.  That report is what the fixed point below is built on,
//! so a pass that returns `true` without having changed the function makes the
//! compiler loop for ever -- each pass is tested for saying `false` the second
//! time it runs over the same function.
//!
//! # Def-use chains
//!
//! Passes that need to know where a value is read recompute the chains from
//! scratch with [`DefUse::compute`](super::defuse::DefUse::compute), rather
//! than the function maintaining them as it is edited.  Recomputing is linear
//! and these functions are small; a chain kept up to date by hand is a second
//! copy of the truth, and the verifier cannot see it go stale.

pub mod blocks;
pub mod copyprop;
pub mod dce;
pub mod sccp;
pub mod simplify;

use super::Function;
use super::verify;

/// One optimisation pass: changes `function` in place, and says whether it
/// changed anything.
type Pass = fn(&mut Function) -> bool;

/// The passes, in the order one round runs them.
const PASSES: &[(&str, Pass)] = &[
    ("sparse conditional constant propagation", sccp::run),
    ("algebraic simplification", simplify::run),
    ("copy propagation", copyprop::run),
    ("dead-code elimination", dce::run),
    ("block merging", blocks::run),
];

/// Optimise one function to a fixed point.
///
/// The verifier runs after every pass in a debug build, so a pass that
/// corrupts the IR is blamed where it did the damage rather than in the
/// assembly two stages later.
pub fn optimize(function: &mut Function) {
    let mut changed = true;
    while changed {
        changed = false;
        for (name, pass) in PASSES {
            changed |= pass(function);
            verify::debug_assert_valid(function, name);
        }
    }
}
