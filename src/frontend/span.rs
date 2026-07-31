//! Source locations shared by every frontend phase.
//!
//! Keeping [`Span`] in its own module means the parser, the semantic analyzer
//! and the diagnostics printer all depend on the location type rather than on
//! the lexer that happens to produce it.

use std::fmt;

/// The location of a piece of source text.
///
/// A span carries both a *human* position (`line`, `column`, 1-based, as every
/// C compiler prints them) and a *machine* one (`start`, `length`, byte offsets
/// into the source). The first is what a diagnostic header shows, the second is
/// what lets two spans be joined into the range that encloses them.
///
/// Columns and lengths are measured in bytes rather than characters, matching
/// the byte-oriented scanner. Since every token delimiter in C is ASCII, the
/// two only differ inside string and character literals.
///
/// The fields are `u32` rather than `usize`: every AST node carries a span, so
/// halving the struct is worth capping the compilable file at 4 GiB.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    /// 1-based line number of the first byte.
    pub line: u32,
    /// 1-based column number of the first byte.
    pub column: u32,
    /// Byte offset of the first byte within the source.
    pub start: u32,
    /// Length of the covered text, in bytes.
    pub length: u32,
}

impl Span {
    /// Span covering `length` bytes from `start`, which sits at `line`:`column`.
    pub const fn new(line: u32, column: u32, start: u32, length: u32) -> Self {
        Self {
            line,
            column,
            start,
            length,
        }
    }

    /// Byte offset one past the last byte covered.
    pub const fn end(&self) -> u32 {
        self.start + self.length
    }

    /// The smallest span enclosing both `self` and `other`.
    ///
    /// The parser builds a node's span this way: it remembers where the node
    /// started and joins that with the last token the node consumed, so the
    /// span covers the whole construct instead of just its first token.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// // `a` at 1:1 joined with `b` at 1:5 covers `a + b`.
    /// let joined = Span::new(1, 1, 0, 1).to(Span::new(1, 5, 4, 1));
    /// assert_eq!(joined, Span::new(1, 1, 0, 5));
    /// ```
    pub fn to(self, other: Span) -> Span {
        // The earlier of the two decides where the span begins, so joining is
        // order-independent -- callers never have to sort the operands.
        let (first, second) = if self.start <= other.start {
            (self, other)
        } else {
            (other, self)
        };

        Span {
            line: first.line,
            column: first.column,
            start: first.start,
            length: second.end().saturating_sub(first.start),
        }
    }

    /// A one-byte span sitting immediately after this one.
    ///
    /// Used to blame the *gap* after a token rather than the token itself: a
    /// missing `;` is reported where the `;` should have been written, which is
    /// where the reader will insert it.
    pub const fn after(&self) -> Span {
        Span {
            line: self.line,
            column: self.column + self.length,
            start: self.end(),
            length: 1,
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
        assert_eq!(Span::new(3, 14, 40, 2).to_string(), "3:14");
    }

    #[test]
    fn joins_spans_in_either_order() {
        let first = Span::new(1, 1, 0, 1);
        let last = Span::new(1, 5, 4, 1);
        let joined = Span::new(1, 1, 0, 5);

        assert_eq!(first.to(last), joined);
        assert_eq!(last.to(first), joined);
    }

    #[test]
    fn joins_across_lines_keeping_the_earlier_position() {
        let opening = Span::new(1, 1, 0, 3);
        let closing = Span::new(4, 1, 30, 1);

        assert_eq!(opening.to(closing), Span::new(1, 1, 0, 31));
    }

    #[test]
    fn points_just_past_the_end() {
        assert_eq!(Span::new(2, 5, 10, 4).after(), Span::new(2, 9, 14, 1));
    }
}
