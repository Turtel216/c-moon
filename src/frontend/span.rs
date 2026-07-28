//! Source locations shared by every frontend phase.
//!
//! Keeping [`Span`] in its own module means the parser, the semantic analyzer
//! and the diagnostics printer all depend on the location type rather than on
//! the lexer that happens to produce it.

use std::fmt;

/// The location of a piece of source text.
///
/// Positions are 1-based, matching the convention every C compiler uses when
/// printing diagnostics. `length` is measured in bytes of the original source,
/// which is what a future "underline the offending token" renderer needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    /// 1-based line number of the first character.
    pub line: usize,
    /// 1-based column number of the first character.
    pub column: usize,
    /// Length of the covered text, in bytes.
    pub length: usize,
}

impl Span {
    /// Span covering `length` bytes starting at `line`:`column`.
    pub const fn new(line: usize, column: usize, length: usize) -> Self {
        Self {
            line,
            column,
            length,
        }
    }
}

impl fmt::Display for Span {
    /// Formats as `line:column`, the prefix used by compiler diagnostics.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.line, self.column)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn displays_as_line_and_column() {
        assert_eq!(Span::new(3, 14, 2).to_string(), "3:14");
    }
}
