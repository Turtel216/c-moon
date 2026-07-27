//! A small `FileCheck` implementation for the `codegen` suite.
//!
//! `rustc`'s `tests/codegen` suite pins down generated code by annotating the
//! source with `CHECK` directives and matching them against the compiler's
//! output. The same idea is used here to assert what the optimiser and the
//! instruction selector emit, which is far more precise than searching the
//! whole listing for a substring.
//!
//! Supported directives, matched in the order they appear:
//!
//! | Directive     | Meaning                                                    |
//! |---------------|------------------------------------------------------------|
//! | `CHECK`       | A later line contains the pattern.                          |
//! | `CHECK-NEXT`  | The immediately following line contains the pattern.         |
//! | `CHECK-NOT`   | No line between the previous and next match has the pattern. |
//! | `CHECK-LABEL` | Like `CHECK`, used to anchor a region (e.g. a function).      |
//!
//! Matching is by substring on whitespace-normalised lines, so the assembler's
//! indentation is irrelevant to a pattern.

use std::fmt::Write as _;

/// Marker that introduces a check directive.
const CHECK_PREFIX: &str = "// CHECK";

/// How many lines of context to show around a failure.
const CONTEXT_LINES: usize = 4;

/// The kind of a single check directive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckKind {
    /// Matches any later line.
    Check,
    /// Matches the line immediately after the previous match.
    Next,
    /// Must not match anywhere in the current region.
    Not,
    /// Anchors a region; matches like [`CheckKind::Check`].
    Label,
}

impl CheckKind {
    /// The directive's spelling, used in failure messages.
    fn as_str(self) -> &'static str {
        match self {
            CheckKind::Check => "CHECK",
            CheckKind::Next => "CHECK-NEXT",
            CheckKind::Not => "CHECK-NOT",
            CheckKind::Label => "CHECK-LABEL",
        }
    }
}

/// One parsed check directive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    kind: CheckKind,
    /// Whitespace-normalised pattern to search for.
    pattern: String,
    /// One-based line number of the directive in the fixture.
    line: usize,
}

/// Collapse runs of whitespace so patterns are insensitive to formatting.
///
/// This mirrors `FileCheck`'s canonicalisation: `"mov   rax,42"` and
/// `"mov rax, 42"` differ only in spacing, which is not something a test
/// should depend on.
fn normalize(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Parse all `// CHECK` directives out of a fixture.
///
/// # Arguments
///
/// * `source` - Full text of the C fixture.
///
/// # Returns
///
/// The directives in source order.
///
/// # Errors
///
/// Returns a message for an unknown `CHECK-*` suffix, a directive missing its
/// colon or pattern, or a fixture that declares no directives at all -- the
/// last of which would otherwise pass vacuously.
pub fn parse(source: &str) -> Result<Vec<Check>, String> {
    let mut checks = Vec::new();

    for (index, line) in source.lines().enumerate() {
        let line_number = index + 1;
        let Some(rest) = line.trim_start().strip_prefix(CHECK_PREFIX) else {
            continue;
        };

        let (suffix, pattern) = rest
            .split_once(':')
            .ok_or_else(|| format!("line {}: check directive is missing its ':'", line_number))?;

        let kind = match suffix {
            "" => CheckKind::Check,
            "-NEXT" => CheckKind::Next,
            "-NOT" => CheckKind::Not,
            "-LABEL" => CheckKind::Label,
            other => {
                return Err(format!(
                    "line {}: unknown directive 'CHECK{}'",
                    line_number, other
                ));
            }
        };

        let pattern = normalize(pattern);
        if pattern.is_empty() {
            return Err(format!(
                "line {}: CHECK{} has an empty pattern",
                line_number, suffix
            ));
        }

        checks.push(Check {
            kind,
            pattern,
            line: line_number,
        });
    }

    if checks.is_empty() {
        return Err(String::from(
            "fixture declares no CHECK directives, so it would pass vacuously",
        ));
    }

    Ok(checks)
}

/// Match a fixture's directives against the generated assembly.
///
/// # Arguments
///
/// * `checks` - Directives in source order, from [`parse`].
/// * `subject` - The assembly listing to match against.
///
/// # Errors
///
/// Returns a rendered failure report naming the directive that did not hold
/// and showing the relevant part of the listing.
pub fn run(checks: &[Check], subject: &str) -> Result<(), String> {
    let lines: Vec<String> = subject.lines().map(normalize).collect();

    // Index of the first line still available to match against.
    let mut cursor = 0usize;
    // `CHECK-NOT` directives apply to the gap before the next positive match,
    // so they are held here until that match (or the end of input) fixes the
    // region they cover.
    let mut pending_not: Vec<&Check> = Vec::new();

    for check in checks {
        match check.kind {
            CheckKind::Not => pending_not.push(check),
            CheckKind::Check | CheckKind::Label => {
                let found = lines
                    .iter()
                    .enumerate()
                    .skip(cursor)
                    .find(|(_, line)| line.contains(&check.pattern))
                    .map(|(index, _)| index)
                    .ok_or_else(|| report(check, &lines, cursor, "no matching line"))?;

                verify_absent(&pending_not, &lines, cursor, found)?;
                pending_not.clear();
                cursor = found + 1;
            }
            CheckKind::Next => {
                let line = lines
                    .get(cursor)
                    .ok_or_else(|| report(check, &lines, cursor, "input ended"))?;
                if !line.contains(&check.pattern) {
                    return Err(report(check, &lines, cursor, "next line does not match"));
                }
                verify_absent(&pending_not, &lines, cursor, cursor)?;
                pending_not.clear();
                cursor += 1;
            }
        }
    }

    // Trailing `CHECK-NOT`s cover everything left in the listing.
    verify_absent(&pending_not, &lines, cursor, lines.len())
}

/// Assert that no pending `CHECK-NOT` pattern occurs in `lines[start..end]`.
fn verify_absent(
    pending: &[&Check],
    lines: &[String],
    start: usize,
    end: usize,
) -> Result<(), String> {
    for check in pending {
        if let Some((index, _)) = lines[start..end]
            .iter()
            .enumerate()
            .find(|(_, line)| line.contains(&check.pattern))
        {
            return Err(report(
                check,
                lines,
                start + index,
                "pattern was found but must not appear",
            ));
        }
    }

    Ok(())
}

/// Render a failure with the offending directive and nearby output.
fn report(check: &Check, lines: &[String], position: usize, reason: &str) -> String {
    let mut message = String::new();
    let _ = writeln!(
        message,
        "{} failed at fixture line {}: {}\n  pattern: {}",
        check.kind.as_str(),
        check.line,
        reason,
        check.pattern
    );

    // `saturating_sub` keeps the window inside the listing near line 0.
    let start = position.saturating_sub(CONTEXT_LINES);
    let end = (position + CONTEXT_LINES + 1).min(lines.len());

    let _ = writeln!(message, "  generated assembly:");
    for (index, line) in lines.iter().enumerate().take(end).skip(start) {
        let marker = if index == position { ">" } else { " " };
        let _ = writeln!(message, "  {} {:>4} | {}", marker, index + 1, line);
    }

    message
}
