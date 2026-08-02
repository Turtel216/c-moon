//! The data model of a diagnostic: what is wrong, where, and what to do.
//!
//! A [`Diagnostic`] is deliberately free of formatting. Deciding *what* to say
//! belongs to the compiler phase that found the problem; deciding how it looks
//! on a terminal belongs to [`super::render`].

use crate::frontend::span::Span;

/// A stable identifier for a class of error, shown as `error[E0201]`.
///
/// Codes are grouped by the phase that reports them: `E01xx` for the lexer and
/// parser, `E02xx` for semantic analysis. They give the reader something
/// searchable that survives rewording of the message itself.
pub mod codes {
    /// Input that could not be turned into a token at all.
    pub const LEXICAL: &str = "E0101";
    /// A token that the grammar does not allow here.
    pub const SYNTAX: &str = "E0102";

    /// A variable used without a visible declaration.
    pub const UNDECLARED_VARIABLE: &str = "E0201";
    /// A function called without a visible declaration.
    pub const UNDECLARED_FUNCTION: &str = "E0202";
    /// A name declared twice in one scope.
    pub const DUPLICATE_DEFINITION: &str = "E0203";
    /// A value whose type is not the one required here.
    pub const MISMATCHED_TYPES: &str = "E0204";
    /// A call whose argument count differs from the declaration's.
    pub const WRONG_ARGUMENT_COUNT: &str = "E0205";
    /// An assignment to something that is not an object.
    pub const INVALID_ASSIGNMENT: &str = "E0206";
    /// Valid C that this compiler does not implement yet.
    pub const UNSUPPORTED: &str = "E0207";
}

/// A span with something to say about it.
///
/// The message may be empty, which renders as a bare underline: enough to point
/// at the offending text when the header already says everything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Label {
    pub span: Span,
    pub message: String,
}

/// Whether a trailing line explains a rule or proposes a fix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FooterKind {
    /// Background the reader needs to understand the error.
    Note,
    /// A concrete suggestion for making the error go away.
    Help,
}

impl FooterKind {
    /// The word printed before the footer's text.
    pub const fn keyword(self) -> &'static str {
        match self {
            FooterKind::Note => "note",
            FooterKind::Help => "help",
        }
    }
}

/// A line printed under the source snippet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Footer {
    pub kind: FooterKind,
    pub message: String,
}

/// One complete compiler error: a headline, the code it blames, and advice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// Searchable identifier, e.g. `E0201`.
    pub code: &'static str,
    /// The headline, phrased as the problem rather than the fix.
    pub message: String,
    /// The span the error is reported at; underlined with `^`.
    pub primary: Label,
    /// Related spans that explain the error; underlined with `-`.
    pub secondary: Vec<Label>,
    /// Trailing `note`/`help` lines.
    pub footers: Vec<Footer>,
}

impl Diagnostic {
    /// Starts a diagnostic blaming `span`, with no label on it yet.
    ///
    /// # Arguments
    ///
    /// * `code` - one of the constants in [`codes`]
    /// * `message` - the headline, e.g. `` cannot find value `x` in this scope ``
    /// * `span` - the source text the error is reported at
    pub fn error(code: &'static str, message: impl Into<String>, span: Span) -> Self {
        Self {
            code,
            message: message.into(),
            primary: Label {
                span,
                message: String::new(),
            },
            secondary: Vec::new(),
            footers: Vec::new(),
        }
    }

    /// Sets the text written next to the primary underline.
    ///
    /// It should read as a caption for the underlined text, not repeat the
    /// headline: `not found in this scope`, not `undeclared variable`.
    #[must_use]
    pub fn with_label(mut self, message: impl Into<String>) -> Self {
        self.primary.message = message.into();
        self
    }

    /// Adds a related span, such as where a name was declared before.
    #[must_use]
    pub fn with_secondary(mut self, span: Span, message: impl Into<String>) -> Self {
        self.secondary.push(Label {
            span,
            message: message.into(),
        });
        self
    }

    /// Adds a trailing `= note:` line.
    #[must_use]
    pub fn with_note(mut self, message: impl Into<String>) -> Self {
        self.footers.push(Footer {
            kind: FooterKind::Note,
            message: message.into(),
        });
        self
    }

    /// Adds a trailing `= help:` line.
    #[must_use]
    pub fn with_help(mut self, message: impl Into<String>) -> Self {
        self.footers.push(Footer {
            kind: FooterKind::Help,
            message: message.into(),
        });
        self
    }

    /// Adds a `= help:` line only when there is something to suggest.
    ///
    /// Saves every caller that computes an optional suggestion -- a
    /// similarly-spelled name, for instance -- from repeating the same `if let`.
    #[must_use]
    pub fn with_optional_help(self, message: Option<String>) -> Self {
        match message {
            Some(message) => self.with_help(message),
            None => self,
        }
    }

    /// Every label of the diagnostic paired with whether it is the primary
    /// one, the primary first.
    pub fn labels(&self) -> impl Iterator<Item = (&Label, bool)> {
        std::iter::once((&self.primary, true)).chain(self.secondary.iter().map(|l| (l, false)))
    }
}
