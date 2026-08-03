//! Static single assignment form.
//!
//! The migration to SSA is staged: this module currently holds the dominance
//! machinery that SSA construction is built on, and the block identifier the
//! rest of it will share.  Nothing in the compiler pipeline consumes any of it
//! yet.

pub mod dom;

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
