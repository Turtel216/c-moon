//! Compiler Diagnostics/Error messages

use crate::frontend::lexer::Span;

// TODO: Add Support for line snippet and fancy arrows etc.

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

    /// Print Compilation errors to stdout
    pub fn print(&self) -> () {
        let mut output: Vec<String> = Vec::new();
        for err in &self.comp_errors {
            let span = err.get_span();
            let message = format!(
                "\x1b[31m{}\x1b[0m {}:{} {}",
                err.error_prefix(),
                span.line,
                span.column,
                err.get_message()
            );

            output.push(message);
        }

        println!("{}", output.join("\n\n"));

        // Summary line
        let count = self.comp_errors.len();
        if count == 1 {
            println!("\n\x1b[31m1 error generated.\x1b[0m");
        } else {
            println!("\n\x1b[31m{} errors generated.\x1b[0m", count);
        }
    }
}
