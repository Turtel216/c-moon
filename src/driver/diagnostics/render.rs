//! Rendering a [`Diagnostic`] as an annotated source snippet.
//!
//! The layout follows `rustc`'s: a headline, the location, the offending lines
//! quoted with the blamed text underlined, and trailing advice.
//!
//! ```text
//! error[E0203]: the name `a` is defined multiple times
//!  --> shadow.c:5:9
//!   |
//! 4 |     int a = 1;
//!   |         - previous declaration of `a` here
//! 5 |     int a = 2;
//!   |         ^ `a` redeclared here
//!   |
//!   = note: `a` must be declared only once in the same scope
//! ```

use std::borrow::Cow;
use std::fmt::Write as _;

use crate::driver::diagnostics::diagnostic::{Diagnostic, Label};
use crate::driver::diagnostics::source::SourceFile;

/// Columns a tab is displayed as, so underlines line up with the quoted text.
const TAB_WIDTH: usize = 4;

/// Number of unannotated lines that are quoted rather than elided.
///
/// One line of context is cheaper to read than a `...` marker; more than that
/// and the snippet stops being a snippet.
const MAX_CONTEXT_LINES: usize = 1;

/// The ANSI sequences a rendered diagnostic is styled with.
///
/// Every field is a `&'static str` so styling costs nothing when it is off:
/// the empty palette writes empty strings instead of branching at each use.
#[derive(Clone, Copy)]
struct Palette {
    /// The `error[E0201]` headline.
    error: &'static str,
    /// The headline's message and the footer keywords.
    bold: &'static str,
    /// Line numbers, `|` and `-->`.
    gutter: &'static str,
    /// The `^^^` underline and its label.
    primary: &'static str,
    /// The `---` underline of a supporting span and its label.
    secondary: &'static str,
    /// Returns to the terminal's default styling.
    reset: &'static str,
}

impl Palette {
    /// Full colour, matching `rustc`'s scheme.
    const COLOR: Self = Self {
        error: "\x1b[1;31m",
        bold: "\x1b[1m",
        gutter: "\x1b[1;34m",
        primary: "\x1b[1;31m",
        secondary: "\x1b[1;34m",
        reset: "\x1b[0m",
    };

    /// No styling at all, for a redirected stderr or `NO_COLOR`.
    const PLAIN: Self = Self {
        error: "",
        bold: "",
        gutter: "",
        primary: "",
        secondary: "",
        reset: "",
    };

    /// The colour a label and its underline are drawn in.
    const fn label(self, is_primary: bool) -> &'static str {
        if is_primary {
            self.primary
        } else {
            self.secondary
        }
    }
}

/// Draws diagnostics against the file they were reported in.
pub struct Renderer<'a> {
    source: &'a SourceFile<'a>,
    palette: Palette,
}

/// One label, resolved to the columns it occupies on screen.
struct Annotation<'a> {
    /// Where the underline starts, in display columns from the left margin.
    start: usize,
    /// How many display columns it covers; at least one.
    width: usize,
    /// `^` for the span the error is reported at, `-` for a supporting one.
    marker: char,
    /// Caption written next to the underline; may be empty.
    message: &'a str,
    is_primary: bool,
}

impl<'a> Renderer<'a> {
    /// Draws diagnostics about `source`, with colour when `color` is set.
    pub fn new(source: &'a SourceFile<'a>, color: bool) -> Self {
        Self {
            source,
            palette: if color {
                Palette::COLOR
            } else {
                Palette::PLAIN
            },
        }
    }

    /// Renders one diagnostic, newline-terminated.
    pub fn render(&self, diagnostic: &Diagnostic) -> String {
        let Palette {
            error,
            bold,
            gutter,
            reset,
            ..
        } = self.palette;

        // Labels are grouped by line, and the widest line number decides how
        // far the source text is indented.
        let groups = self.group_by_line(diagnostic);
        let width = groups.last().map_or(1, |group| decimal_width(group.number));

        let mut out = String::new();
        let _ = writeln!(
            out,
            "{error}error[{code}]{reset}{bold}: {message}{reset}",
            code = diagnostic.code,
            message = diagnostic.message,
        );

        let span = diagnostic.primary.span;
        let _ = writeln!(
            out,
            "{blank:width$}{gutter}-->{reset} {path}:{line}:{column}",
            blank = "",
            path = self.source.path(),
            line = span.line,
            column = span.column,
        );

        self.write_bar(&mut out, width);

        let mut previous: Option<usize> = None;
        for group in &groups {
            self.write_gap(&mut out, width, previous, group.number);
            self.write_group(&mut out, width, group);
            previous = Some(group.number);
        }

        if !diagnostic.footers.is_empty() {
            self.write_bar(&mut out, width);
        }
        for footer in &diagnostic.footers {
            let _ = writeln!(
                out,
                "{blank:width$} {gutter}={reset} {bold}{keyword}{reset}: {message}",
                blank = "",
                keyword = footer.kind.keyword(),
                message = footer.message,
            );
        }

        out
    }

    /// Collects the diagnostic's labels into one group per source line, in the
    /// order the lines appear in the file.
    fn group_by_line<'d>(&self, diagnostic: &'d Diagnostic) -> Vec<LineGroup<'d>> {
        let mut groups: Vec<LineGroup<'d>> = Vec::new();

        for (label, is_primary) in diagnostic.labels() {
            let line = label.span.line as usize;
            let annotation = self.annotate(label, is_primary);

            match groups.iter_mut().find(|group| group.number == line) {
                Some(group) => group.annotations.push(annotation),
                None => groups.push(LineGroup {
                    number: line,
                    annotations: vec![annotation],
                }),
            }
        }

        groups.sort_by_key(|group| group.number);
        for group in &mut groups {
            group.annotations.sort_by_key(|annotation| annotation.start);
        }
        groups
    }

    /// Resolves a label's byte span to the display columns it underlines.
    ///
    /// A span that runs past the end of its line -- a multi-line construct such
    /// as a function signature -- is underlined up to the line break only.
    fn annotate<'d>(&self, label: &'d Label, is_primary: bool) -> Annotation<'d> {
        let text = self.source.line(label.span.line as usize);
        let start_column = label.span.column as usize;
        let end_column = (start_column + label.span.length as usize).min(text.len() + 1);

        let start = display_offset(text, start_column);
        Annotation {
            start,
            width: display_offset(text, end_column)
                .saturating_sub(start)
                .max(1),
            marker: if is_primary { '^' } else { '-' },
            message: &label.message,
            is_primary,
        }
    }

    /// Writes what separates two annotated lines: nothing when they are
    /// adjacent, the intervening source when there is little of it, `...` when
    /// there is more.
    fn write_gap(&self, out: &mut String, width: usize, previous: Option<usize>, next: usize) {
        let Some(previous) = previous else {
            return;
        };

        let skipped = next.saturating_sub(previous + 1);
        if skipped > MAX_CONTEXT_LINES {
            let _ = writeln!(out, "{}...{}", self.palette.gutter, self.palette.reset);
            return;
        }

        for number in previous + 1..next {
            self.write_source_line(out, width, number);
        }
    }

    /// Writes one annotated line: the source, then the underlines below it.
    fn write_group(&self, out: &mut String, width: usize, group: &LineGroup<'_>) {
        self.write_source_line(out, width, group.number);

        // The rightmost caption is written on the underline row itself; the
        // others need their own rows, connected upwards by `|`.
        let inline = group
            .annotations
            .iter()
            .rposition(|annotation| !annotation.message.is_empty());

        self.write_markers(out, width, group, inline);

        let stacked: Vec<&Annotation<'_>> = group
            .annotations
            .iter()
            .enumerate()
            .filter(|&(index, annotation)| !annotation.message.is_empty() && Some(index) != inline)
            .map(|(_, annotation)| annotation)
            .collect();

        if stacked.is_empty() {
            return;
        }

        self.write_connectors(out, width, &stacked, stacked.len());
        for index in (0..stacked.len()).rev() {
            self.write_connectors(out, width, &stacked, index);
            let annotation = stacked[index];
            let _ = writeln!(
                out,
                "{}{}{}",
                self.palette.label(annotation.is_primary),
                annotation.message,
                self.palette.reset
            );
        }
    }

    /// Writes a quoted source line, with its number in the gutter.
    fn write_source_line(&self, out: &mut String, width: usize, number: usize) {
        let _ = writeln!(
            out,
            "{gutter}{number:>width$} |{reset} {text}",
            gutter = self.palette.gutter,
            reset = self.palette.reset,
            text = expand_tabs(self.source.line(number)),
        );
    }

    /// Writes the row of `^` and `-` underlines below a quoted line.
    ///
    /// `inline` is the index of the annotation whose caption goes on this row,
    /// which is the rightmost one that has a caption.
    fn write_markers(
        &self,
        out: &mut String,
        width: usize,
        group: &LineGroup<'_>,
        inline: Option<usize>,
    ) {
        self.write_gutter(out, width);

        // The row is laid out column by column so that nested spans still show:
        // the primary underline is drawn last and overwrites whatever a
        // supporting span had claimed.
        let end = group
            .annotations
            .iter()
            .map(|annotation| annotation.start + annotation.width)
            .max()
            .unwrap_or_default();

        let mut row: Vec<Option<&Annotation<'_>>> = vec![None; end];
        let supporting = group.annotations.iter().filter(|a| !a.is_primary);
        let blamed = group.annotations.iter().filter(|a| a.is_primary);
        for annotation in supporting.chain(blamed) {
            row[annotation.start..annotation.start + annotation.width].fill(Some(annotation));
        }

        // Consecutive columns claimed by the same annotation are written as one
        // run, so the colour escapes are emitted once per underline.
        let mut column = 0;
        while column < end {
            let Some(annotation) = row[column] else {
                out.push(' ');
                column += 1;
                continue;
            };

            let run = row[column..]
                .iter()
                .take_while(|claim| claim.is_some_and(|other| std::ptr::eq(other, annotation)))
                .count();
            let _ = write!(
                out,
                "{color}{marker}{reset}",
                color = self.palette.label(annotation.is_primary),
                marker = repeat(annotation.marker, run),
                reset = self.palette.reset,
            );
            column += run;
        }

        match inline.map(|index| &group.annotations[index]) {
            Some(annotation) => {
                let _ = writeln!(
                    out,
                    " {}{}{}",
                    self.palette.label(annotation.is_primary),
                    annotation.message,
                    self.palette.reset
                );
            }
            None => out.push('\n'),
        }
    }

    /// Writes a row of `|` connecting the first `count` stacked captions to
    /// their underlines, and leaves the cursor at the caption's column.
    fn write_connectors(
        &self,
        out: &mut String,
        width: usize,
        stacked: &[&Annotation<'_>],
        count: usize,
    ) {
        self.write_gutter(out, width);

        let mut column = 0;
        for annotation in &stacked[..count] {
            let _ = write!(
                out,
                "{blank:gap$}{color}|{reset}",
                blank = "",
                gap = annotation.start.saturating_sub(column),
                color = self.palette.label(annotation.is_primary),
                reset = self.palette.reset,
            );
            column = annotation.start + 1;
        }

        // A row that only connects ends here; one that carries a caption has it
        // appended at the column the loop stopped at.
        if count == stacked.len() {
            out.push('\n');
        } else if let Some(gap) = stacked[count].start.checked_sub(column) {
            let _ = write!(out, "{blank:gap$}", blank = "");
        }
    }

    /// Writes the empty gutter that a non-source row starts with.
    fn write_gutter(&self, out: &mut String, width: usize) {
        let _ = write!(
            out,
            "{gutter}{blank:width$} |{reset} ",
            gutter = self.palette.gutter,
            blank = "",
            reset = self.palette.reset,
        );
    }

    /// Writes the bare `|` row that opens the snippet and precedes the footers.
    fn write_bar(&self, out: &mut String, width: usize) {
        let _ = writeln!(
            out,
            "{gutter}{blank:width$} |{reset}",
            gutter = self.palette.gutter,
            blank = "",
            reset = self.palette.reset,
        );
    }
}

/// The labels that fall on one source line.
struct LineGroup<'a> {
    number: usize,
    annotations: Vec<Annotation<'a>>,
}

/// Display column, counted from the left margin, of a 1-based byte column.
///
/// Tabs count as [`TAB_WIDTH`] columns and every other character as one, which
/// is what keeps an underline under the text it belongs to.
fn display_offset(line: &str, column: usize) -> usize {
    let target = column.saturating_sub(1);
    let mut offset = 0;

    for (index, ch) in line.char_indices() {
        if index >= target {
            return offset;
        }
        offset += char_width(ch);
    }

    // A column past the end of the line -- where a missing token is reported --
    // extends the line by one column per byte.
    offset + target.saturating_sub(line.len())
}

/// How many columns a character occupies when quoted.
const fn char_width(ch: char) -> usize {
    if ch == '\t' { TAB_WIDTH } else { 1 }
}

/// Replaces tabs so quoted text lines up with the underlines below it.
///
/// Returns a [`Cow`] because the common case -- a line with no tab in it --
/// needs no new allocation.
fn expand_tabs(line: &str) -> Cow<'_, str> {
    match line.contains('\t') {
        true => Cow::Owned(line.replace('\t', &repeat(' ', TAB_WIDTH))),
        false => Cow::Borrowed(line),
    }
}

/// A string of `count` copies of `ch`.
fn repeat(ch: char, count: usize) -> String {
    std::iter::repeat_n(ch, count).collect()
}

/// Number of digits `number` is printed with.
fn decimal_width(number: usize) -> usize {
    // `ilog10` is undefined for zero, which is not a valid line number anyway.
    number.checked_ilog10().unwrap_or(0) as usize + 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::driver::diagnostics::diagnostic::codes;
    use crate::frontend::span::Span;

    /// Renders `diagnostic` against `source` without colour.
    fn render(source: &str, diagnostic: &Diagnostic) -> String {
        let file = SourceFile::new("main.c", source);
        Renderer::new(&file, false).render(diagnostic)
    }

    #[test]
    fn underlines_the_blamed_token() {
        let source = "int main() {\n    return value;\n}\n";
        let diagnostic = Diagnostic::error(
            codes::UNDECLARED_VARIABLE,
            "cannot find value `value` in this scope",
            Span::new(2, 12, 24, 5),
        )
        .with_label("not found in this scope");

        assert_eq!(
            render(source, &diagnostic),
            "\
error[E0201]: cannot find value `value` in this scope
 --> main.c:2:12
  |
2 |     return value;
  |            ^^^^^ not found in this scope
"
        );
    }

    #[test]
    fn quotes_both_lines_of_a_two_span_error() {
        let source = "int a = 1;\nint a = 2;\n";
        let diagnostic = Diagnostic::error(
            codes::DUPLICATE_DEFINITION,
            "the name `a` is defined multiple times",
            Span::new(2, 5, 15, 1),
        )
        .with_label("`a` redeclared here")
        .with_secondary(Span::new(1, 5, 4, 1), "previous declaration of `a` here")
        .with_note("`a` must be declared only once in the same scope");

        assert_eq!(
            render(source, &diagnostic),
            "\
error[E0203]: the name `a` is defined multiple times
 --> main.c:2:5
  |
1 | int a = 1;
  |     - previous declaration of `a` here
2 | int a = 2;
  |     ^ `a` redeclared here
  |
  = note: `a` must be declared only once in the same scope
"
        );
    }

    #[test]
    fn stacks_two_labels_on_one_line() {
        let source = "int x = f(1);\n";
        let diagnostic = Diagnostic::error(
            codes::MISMATCHED_TYPES,
            "mismatched types",
            Span::new(1, 9, 8, 5),
        )
        .with_label("expected `int`, found `int*`")
        .with_secondary(Span::new(1, 1, 0, 3), "expected due to this");

        assert_eq!(
            render(source, &diagnostic),
            "\
error[E0204]: mismatched types
 --> main.c:1:9
  |
1 | int x = f(1);
  | ---     ^^^^^ expected `int`, found `int*`
  | |
  | expected due to this
"
        );
    }

    #[test]
    fn elides_the_lines_between_distant_spans() {
        let source = "int f() { return 1; }\nint unrelated;\nint g();\nint f() { return 2; }\n";
        let diagnostic = Diagnostic::error(
            codes::DUPLICATE_DEFINITION,
            "the name `f` is defined multiple times",
            Span::new(4, 5, 46, 1),
        )
        .with_secondary(Span::new(1, 5, 4, 1), "previous definition here");

        assert_eq!(
            render(source, &diagnostic),
            "\
error[E0203]: the name `f` is defined multiple times
 --> main.c:4:5
  |
1 | int f() { return 1; }
  |     - previous definition here
...
4 | int f() { return 2; }
  |     ^
"
        );
    }

    #[test]
    fn keeps_a_single_intervening_line_as_context() {
        let source = "int f();\nint x;\nint f();\n";
        let diagnostic = Diagnostic::error(
            codes::DUPLICATE_DEFINITION,
            "the name `f` is defined multiple times",
            Span::new(3, 5, 20, 1),
        )
        .with_secondary(Span::new(1, 5, 4, 1), "previous definition here");

        assert_eq!(
            render(source, &diagnostic),
            "\
error[E0203]: the name `f` is defined multiple times
 --> main.c:3:5
  |
1 | int f();
  |     - previous definition here
2 | int x;
3 | int f();
  |     ^
"
        );
    }

    #[test]
    fn aligns_underlines_past_a_tab() {
        let source = "int main() {\n\treturn x;\n}\n";
        let diagnostic = Diagnostic::error(
            codes::UNDECLARED_VARIABLE,
            "cannot find value `x` in this scope",
            Span::new(2, 9, 21, 1),
        )
        .with_label("not found in this scope");

        assert_eq!(
            render(source, &diagnostic),
            "\
error[E0201]: cannot find value `x` in this scope
 --> main.c:2:9
  |
2 |     return x;
  |            ^ not found in this scope
"
        );
    }

    #[test]
    fn points_past_the_end_of_a_line() {
        let source = "int main() {\n    int a = 1\n    return a;\n}\n";
        let diagnostic = Diagnostic::error(
            codes::SYNTAX,
            "expected `;`, found `return`",
            Span::new(2, 14, 26, 1),
        )
        .with_label("expected `;` here");

        assert_eq!(
            render(source, &diagnostic),
            "\
error[E0102]: expected `;`, found `return`
 --> main.c:2:14
  |
2 |     int a = 1
  |              ^ expected `;` here
"
        );
    }

    #[test]
    fn widens_the_gutter_for_long_files() {
        let source = "\n".repeat(9) + "int x = y;\n";
        let diagnostic = Diagnostic::error(
            codes::UNDECLARED_VARIABLE,
            "cannot find value `y` in this scope",
            Span::new(10, 9, 17, 1),
        );

        assert_eq!(
            render(&source, &diagnostic),
            "\
error[E0201]: cannot find value `y` in this scope
  --> main.c:10:9
   |
10 | int x = y;
   |         ^
"
        );
    }

    #[test]
    fn colours_the_snippet_when_asked_to() {
        let file = SourceFile::new("main.c", "int x = y;\n");
        let diagnostic = Diagnostic::error(
            codes::UNDECLARED_VARIABLE,
            "cannot find value `y` in this scope",
            Span::new(1, 9, 8, 1),
        );

        let rendered = Renderer::new(&file, true).render(&diagnostic);
        assert!(rendered.starts_with("\x1b[1;31merror[E0201]\x1b[0m"));
        assert!(rendered.contains("\x1b[1;31m^\x1b[0m"));
    }
}
