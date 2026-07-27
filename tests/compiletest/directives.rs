//! Parsing of the `//@` header directives carried by every test fixture.
//!
//! The syntax follows `rustc`'s own compiletest suite: a fixture declares how
//! it should be run in comments at the top of the file, so the C source and
//! its expectations live together in a single reviewable file.
//!
//! ```c
//! //@ exit-code: 42
//! //@ compile-flags: --opt
//! //@ only: opt
//! ```
//!
//! An unrecognised directive is a hard error rather than a silent no-op, so a
//! typo cannot quietly disable a test.

use std::fmt;

/// Marker that introduces a directive line.
const DIRECTIVE_PREFIX: &str = "//@";

/// Which optimisation variants a fixture should be run under.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VariantPolicy {
    /// Run the fixture both with and without `--opt` (the default).
    ///
    /// Every behavioural test is expected to produce identical results at
    /// both levels, so requiring an opt-out rather than an opt-in means new
    /// fixtures test the optimiser by default.
    #[default]
    Both,
    /// Run only without `--opt`.
    NoOptOnly,
    /// Run only with `--opt`.
    OptOnly,
}

/// The expectations declared by a fixture's header.
///
/// The derived [`Default`] is the "nothing declared" state: exit successfully,
/// no extra flags, both optimisation variants, not ignored.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Directives {
    /// Exit status the compiled program must produce (`run-pass` suite).
    /// `None` means the fixture did not declare one.
    pub exit_code: Option<i32>,
    /// Extra flags passed to the compiler in every variant.
    pub compile_flags: Vec<String>,
    /// Optimisation variants to generate for this fixture.
    pub variants: VariantPolicy,
    /// When set, the fixture is reported as ignored with this reason.
    pub ignore: Option<String>,
}

/// A malformed or unknown directive, reported with its source line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectiveError {
    /// One-based line number of the offending directive.
    pub line: usize,
    /// What was wrong with it.
    pub message: String,
}

impl fmt::Display for DirectiveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

/// Parse every `//@` directive in a fixture.
///
/// # Arguments
///
/// * `source` - Full text of the C fixture.
///
/// # Returns
///
/// The declared expectations, with defaults for anything left unspecified.
///
/// # Errors
///
/// Returns a [`DirectiveError`] for an unknown directive name, a directive
/// that is missing its argument, or an `exit-code` outside `0..=255`.
///
/// # Examples
///
/// ```
/// let directives = parse("//@ exit-code: 42\nint main() { return 42; }")?;
/// assert_eq!(directives.exit_code, Some(42));
/// ```
pub fn parse(source: &str) -> Result<Directives, DirectiveError> {
    let mut directives = Directives::default();

    // `enumerate` gives the zero-based index; directives are reported with
    // one-based line numbers to match what an editor shows.
    for (index, line) in source.lines().enumerate() {
        let line_number = index + 1;
        let Some(body) = line.trim_start().strip_prefix(DIRECTIVE_PREFIX) else {
            continue;
        };

        // `split_once` separates `name: value` without allocating; a
        // directive without a colon (such as `//@ ignore`) keeps an empty
        // argument.
        let (name, argument) = match body.split_once(':') {
            Some((name, argument)) => (name.trim(), argument.trim()),
            None => (body.trim(), ""),
        };

        let error = |message: String| DirectiveError {
            line: line_number,
            message,
        };

        match name {
            "exit-code" => {
                let code: i32 = argument
                    .parse()
                    .map_err(|_| error(format!("'{}' is not an exit status", argument)))?;
                // A Unix wait status only carries the low 8 bits, so any
                // other value could never be observed.
                if !(0..=255).contains(&code) {
                    return Err(error(format!("exit-code {} is outside 0..=255", code)));
                }
                directives.exit_code = Some(code);
            }
            "compile-flags" => {
                if argument.is_empty() {
                    return Err(error(String::from("compile-flags needs at least one flag")));
                }
                directives
                    .compile_flags
                    .extend(argument.split_whitespace().map(String::from));
            }
            "only" => {
                directives.variants = match argument {
                    "opt" => VariantPolicy::OptOnly,
                    "no-opt" => VariantPolicy::NoOptOnly,
                    other => {
                        return Err(error(format!(
                            "unknown variant '{}', expected 'opt' or 'no-opt'",
                            other
                        )));
                    }
                };
            }
            "ignore" => {
                directives.ignore = Some(if argument.is_empty() {
                    String::from("ignored by directive")
                } else {
                    argument.to_owned()
                });
            }
            other => {
                return Err(error(format!(
                    "unknown directive '{}'; known directives are \
                     exit-code, compile-flags, only, ignore",
                    other
                )));
            }
        }
    }

    Ok(directives)
}
