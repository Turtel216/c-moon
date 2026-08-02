//! The compiled file, indexed by line so diagnostics can quote it.

/// A source file the compiler can quote from when reporting an error.
///
/// The file is split into lines once, when the diagnostic printer is created,
/// rather than every time an error is rendered.
pub struct SourceFile<'a> {
    path: &'a str,
    lines: Vec<&'a str>,
}

impl<'a> SourceFile<'a> {
    /// Indexes `source`, which was read from `path`.
    pub fn new(path: &'a str, source: &'a str) -> Self {
        Self {
            path,
            // `lines` splits on `\n` and strips a trailing `\r`, so a file with
            // Windows line endings quotes just as cleanly as a Unix one.
            lines: source.lines().collect(),
        }
    }

    /// The path the file was read from, as written on the command line.
    pub fn path(&self) -> &'a str {
        self.path
    }

    /// The text of a 1-based line number.
    ///
    /// Returns `""` for a line past the end of the file, which is where a span
    /// pointing at the end of input lands.
    pub fn line(&self, number: usize) -> &'a str {
        number
            .checked_sub(1)
            .and_then(|index| self.lines.get(index))
            .copied()
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_lines_by_number() {
        let file = SourceFile::new("main.c", "int main() {\n    return 0;\n}\n");

        assert_eq!(file.line(1), "int main() {");
        assert_eq!(file.line(2), "    return 0;");
        assert_eq!(file.line(3), "}");
    }

    #[test]
    fn reports_an_empty_line_past_the_end() {
        let file = SourceFile::new("main.c", "int x;\n");

        assert_eq!(file.line(0), "");
        assert_eq!(file.line(2), "");
    }
}
