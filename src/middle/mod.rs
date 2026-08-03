//!  Compiler Middle-end

pub mod desuger;
pub mod ir;
// Phase 1 of the SSA migration: dominance is implemented and tested, but no
// pass consumes it yet. The attribute goes when construction is wired up.
#[allow(dead_code)]
pub mod ssa;
