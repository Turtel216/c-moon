//! Compiler diagnostics: collecting errors and reporting them to the user.
//!
//! A phase that finds a problem hands over its own error type; the trait
//! [`CompilerError`] turns it into a [`Diagnostic`], which [`Diagnostics`]
//! collects and finally renders as an annotated source snippet.

use std::io::{IsTerminal, Write, stderr};

mod diagnostic;
mod render;
mod source;

pub use diagnostic::{Diagnostic, codes};

use render::Renderer;
use source::SourceFile;

/// An error a compiler phase can report.
///
/// Implementors describe *what* is wrong and where; how it looks on screen is
/// entirely up to the renderer.
pub trait CompilerError {
    /// Describes this error as a diagnostic: headline, blamed spans, advice.
    fn into_diagnostic(self) -> Diagnostic;
}

/// The errors found so far, and the file they were found in.
pub struct Diagnostics<'a> {
    source: SourceFile<'a>,
    errors: Vec<Diagnostic>,
    color: bool,
}

impl<'a> Diagnostics<'a> {
    /// Collects diagnostics about `source`, which was read from `path`.
    pub fn new(path: &'a str, source: &'a str) -> Self {
        Self {
            source: SourceFile::new(path, source),
            errors: Vec::new(),
            color: color_enabled(),
        }
    }

    /// Report a single compiler error.
    pub fn report<T: CompilerError>(&mut self, error: T) {
        self.errors.push(error.into_diagnostic());
    }

    /// Report a batch of compiler errors at once.
    pub fn report_all<T: CompilerError>(&mut self, errors: Vec<T>) {
        self.errors
            .extend(errors.into_iter().map(CompilerError::into_diagnostic));
    }

    /// Whether an error was reported and compilation must stop.
    pub fn panic(&self) -> bool {
        !self.errors.is_empty()
    }

    /// Print the collected diagnostics to stderr.
    ///
    /// Diagnostics go to stderr rather than stdout so that a compiled program's
    /// output and the compiler's own output never interleave, and so test
    /// harnesses can snapshot them independently.
    pub fn print(&self) {
        let renderer = Renderer::new(&self.source, self.color);

        // The whole report is rendered before anything is written, so that
        // locking stderr once keeps it contiguous even when another thread is
        // writing concurrently.
        let mut report = String::new();
        for error in &self.errors {
            report.push_str(&renderer.render(error));
            report.push('\n');
        }
        report.push_str(&self.summary());

        let mut out = stderr().lock();
        let _ = write!(out, "{}", report);
    }

    /// The closing line: how many errors ended the compilation.
    fn summary(&self) -> String {
        let (bold, reset) = match self.color {
            true => ("\x1b[1m", "\x1b[0m"),
            false => ("", ""),
        };

        match self.errors.len() {
            1 => format!("{bold}error: aborting due to 1 previous error{reset}\n"),
            count => format!("{bold}error: aborting due to {count} previous errors{reset}\n"),
        }
    }
}

/// Decide whether ANSI colour codes should be emitted.
///
/// Colour is suppressed when stderr is redirected (a pipe or a file, as in a
/// test harness) or when the conventional `NO_COLOR` variable is set, so that
/// captured diagnostics stay free of escape sequences.
fn color_enabled() -> bool {
    stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::span::Span;

    /// An error type standing in for a real phase's, for the tests below.
    struct Dummy;

    impl CompilerError for Dummy {
        fn into_diagnostic(self) -> Diagnostic {
            Diagnostic::error(codes::SYNTAX, "expected `;`", Span::new(1, 1, 0, 1))
        }
    }

    #[test]
    fn starts_out_clean() {
        let diagnostics = Diagnostics::new("main.c", "int x;\n");

        assert!(!diagnostics.panic());
    }

    #[test]
    fn counts_reported_errors() {
        let mut diagnostics = Diagnostics::new("main.c", "int x;\n");
        diagnostics.report(Dummy);
        diagnostics.report_all(vec![Dummy, Dummy]);

        assert!(diagnostics.panic());
        assert_eq!(diagnostics.errors.len(), 3);
    }

    #[test]
    fn pluralises_the_summary() {
        let mut diagnostics = Diagnostics::new("main.c", "int x;\n");
        diagnostics.color = false;

        diagnostics.report(Dummy);
        assert_eq!(
            diagnostics.summary(),
            "error: aborting due to 1 previous error\n"
        );

        diagnostics.report(Dummy);
        assert_eq!(
            diagnostics.summary(),
            "error: aborting due to 2 previous errors\n"
        );
    }
}
