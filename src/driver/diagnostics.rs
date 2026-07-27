//! Compiler Diagnostics/Error messages

use std::io::{IsTerminal, Write, stderr};

use crate::frontend::lexer::Span;

// TODO: Add Support for line snippet and fancy arrows etc.

/// ANSI escape sequence that switches the terminal to red text.
const ANSI_RED: &str = "\x1b[31m";

/// ANSI escape sequence that restores the terminal's default styling.
const ANSI_RESET: &str = "\x1b[0m";

/// Common behaviour of Compiler Errors for later reporting
pub trait CompilerError {
    fn get_span(&self) -> Span;
    fn get_message(&self) -> String;
    fn error_prefix(&self) -> String;
}

pub struct Diagnostics {
    comp_errors: Vec<Box<dyn CompilerError>>,
}

impl Diagnostics {
    pub fn new() -> Self {
        Diagnostics {
            comp_errors: Vec::new(),
        }
    }

    /// Report a single compiler error.
    pub fn report<T: CompilerError + 'static>(&mut self, comp_error: T) -> () {
        self.comp_errors.push(Box::new(comp_error));
    }

    /// Report a batch of compiler errors at once.
    pub fn report_all<T: CompilerError + 'static>(&mut self, errors: Vec<T>) -> () {
        for e in errors {
            self.comp_errors.push(Box::new(e));
        }
    }

    /// Check if a Compiler has accured and if the compilation proccess should be stoped.
    pub fn panic(&self) -> bool {
        !self.comp_errors.is_empty()
    }

    /// Return the number of errors reported so far.
    pub fn error_count(&self) -> usize {
        self.comp_errors.len()
    }

    /// Print Compilation errors to stderr.
    ///
    /// Diagnostics go to stderr rather than stdout so that a compiled
    /// program's output and the compiler's own output never interleave, and
    /// so test harnesses can snapshot them independently.
    pub fn print(&self) -> () {
        let (red, reset) = if color_enabled() {
            (ANSI_RED, ANSI_RESET)
        } else {
            ("", "")
        };

        // Locking stderr once keeps the whole report contiguous even when
        // another thread is writing concurrently.
        let mut out = stderr().lock();

        let mut output: Vec<String> = Vec::with_capacity(self.comp_errors.len());
        for err in &self.comp_errors {
            let span = err.get_span();
            let message = format!(
                "{}{}{} {}:{} {}",
                red,
                err.error_prefix(),
                reset,
                span.line,
                span.column,
                err.get_message()
            );

            output.push(message);
        }

        let _ = writeln!(out, "{}", output.join("\n\n"));

        // Summary line
        let count = self.comp_errors.len();
        if count == 1 {
            let _ = writeln!(out, "\n{}1 error generated.{}", red, reset);
        } else {
            let _ = writeln!(out, "\n{}{} errors generated.{}", red, count, reset);
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
