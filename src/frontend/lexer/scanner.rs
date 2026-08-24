//! Character-level scanning: turns source text into raw tokens.
//!
//! The scanner knows nothing about macros. It hands the preprocessing layer in
//! [`super::Lexer`] a flat token stream plus the directives it recognises on
//! the way.

use crate::frontend::span::Span;

use super::macros::MacroDef;
use super::token::{LexError, Token, TokenKind, char_literal_value};

/// A preprocessor directive recognised at the start of a line.
pub enum Directive<'a> {
    /// `#define NAME replacement` or `#define NAME(params) replacement`.
    Define { name: String, def: MacroDef<'a> },
    /// A directive this compiler does not implement; its line was skipped.
    Ignored,
}

/// Cursor over the source text.
pub struct Scanner<'a> {
    input: &'a str,
    /// Byte view of `input`. C source is scanned byte-wise: every token
    /// delimiter is ASCII, so multi-byte UTF-8 sequences can only appear
    /// inside string and character literals, where they are copied verbatim.
    bytes: &'a [u8],
    pos: usize,
    line: usize,
    column: usize,
}

impl<'a> Scanner<'a> {
    pub fn new(input: &'a str) -> Self {
        Self {
            input,
            bytes: input.as_bytes(),
            pos: 0,
            line: 1,
            column: 1,
        }
    }

    /// Scans the next token, skipping whitespace and comments before it.
    ///
    /// At the end of input this keeps returning [`TokenKind::Eof`].
    pub fn next_token(&mut self) -> Token<'a> {
        self.skip_trivia();

        let start = self.mark();
        let Some(c) = self.advance() else {
            return Token::borrowed(TokenKind::Eof, "", start.span_to(self.pos));
        };

        let kind = match c {
            // Identifiers and keywords
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => {
                self.consume_while(|c| c.is_ascii_alphanumeric() || c == b'_');
                let lexeme = &self.input[start.pos..self.pos];
                TokenKind::keyword(lexeme).unwrap_or(TokenKind::Identifier)
            }

            // Numeric literals
            b'0'..=b'9' => self.scan_number(),

            // Literals delimited by a quote character
            b'"' => self.scan_quoted(b'"', TokenKind::StringLiteral, LexError::UnterminatedString),
            b'\'' => self.scan_char(start.pos),

            // Operators, longest match first (maximal munch)
            b'+' => self.munch(
                &[(b'+', TokenKind::PlusPlus), (b'=', TokenKind::PlusEq)],
                TokenKind::Plus,
            ),
            b'-' => self.munch(
                &[
                    (b'-', TokenKind::MinusMinus),
                    (b'>', TokenKind::Arrow),
                    (b'=', TokenKind::MinusEq),
                ],
                TokenKind::Minus,
            ),
            b'*' => self.munch(&[(b'=', TokenKind::StarEq)], TokenKind::Star),
            b'/' => self.munch(&[(b'=', TokenKind::SlashEq)], TokenKind::Slash),
            b'=' => self.munch(&[(b'=', TokenKind::EqEq)], TokenKind::Eq),
            b'!' => self.munch(&[(b'=', TokenKind::BangEq)], TokenKind::Bang),
            b'^' => self.munch(&[(b'=', TokenKind::CaretEq)], TokenKind::Caret),
            b'&' => self.munch(
                &[(b'&', TokenKind::AmpAmp), (b'=', TokenKind::AmpEq)],
                TokenKind::Ampersand,
            ),
            b'|' => self.munch(
                &[(b'|', TokenKind::PipePipe), (b'=', TokenKind::PipeEq)],
                TokenKind::Pipe,
            ),
            // `%>` and `<%`, `<:`, `:>` are digraphs: alternative spellings of
            // the brace and bracket tokens, kept for standards compliance.
            b'%' => self.munch(
                &[(b'=', TokenKind::PercentEq), (b'>', TokenKind::RBrace)],
                TokenKind::Percent,
            ),
            b':' => self.munch(&[(b'>', TokenKind::RBracket)], TokenKind::Colon),
            b'<' => match self.peek() {
                // `<<` and `<<=` need a second round of munching.
                Some(b'<') => {
                    self.advance();
                    self.munch(&[(b'=', TokenKind::ShlEq)], TokenKind::Shl)
                }
                _ => self.munch(
                    &[
                        (b'=', TokenKind::LessEq),
                        (b'%', TokenKind::LBrace),
                        (b':', TokenKind::LBracket),
                    ],
                    TokenKind::Less,
                ),
            },
            b'>' => match self.peek() {
                Some(b'>') => {
                    self.advance();
                    self.munch(&[(b'=', TokenKind::ShrEq)], TokenKind::Shr)
                }
                _ => self.munch(&[(b'=', TokenKind::GreaterEq)], TokenKind::Greater),
            },

            b'~' => TokenKind::Tilde,
            b'?' => TokenKind::Question,
            b'(' => TokenKind::LParen,
            b')' => TokenKind::RParen,
            b'{' => TokenKind::LBrace,
            b'}' => TokenKind::RBrace,
            b'[' => TokenKind::LBracket,
            b']' => TokenKind::RBracket,
            b';' => TokenKind::Semicolon,
            b',' => TokenKind::Comma,
            b'.' => TokenKind::Dot,

            // A `#` only has meaning at the start of a line, where the
            // preprocessing layer consumes it before the scanner is asked for
            // a token.
            _ => TokenKind::Error(LexError::UnexpectedChar),
        };

        Token::borrowed(
            kind,
            &self.input[start.pos..self.pos],
            start.span_to(self.pos),
        )
    }

    /// Consumes a preprocessor directive if the cursor sits on one.
    ///
    /// # Returns
    ///
    /// `None` when the next thing in the input is not a directive; in that case
    /// the cursor is left untouched and the caller should scan a token instead.
    pub fn scan_directive(&mut self) -> Option<Directive<'a>> {
        // Trivia includes newlines, so this also lands the cursor on a `#`
        // that is preceded by indentation. Skipping it early is harmless when
        // no directive follows: the next token scan would skip it anyway.
        self.skip_trivia();

        if self.peek() != Some(b'#') {
            return None;
        }

        self.advance(); // '#'
        self.skip_inline_whitespace();

        let directive = match self.scan_identifier().as_deref() {
            Some("define") => self.scan_define(),
            _ => None,
        };

        // Whether or not the directive was understood, the rest of its line is
        // not program text.
        self.skip_to_next_line();
        Some(directive.unwrap_or(Directive::Ignored))
    }

    /// Parses the body of a `#define`, with the `define` keyword consumed.
    fn scan_define(&mut self) -> Option<Directive<'a>> {
        self.skip_inline_whitespace();
        let name = self.scan_identifier()?;

        // A macro is function-like only when `(` follows the name with no
        // space in between: `#define F(x) x` takes an argument, while
        // `#define F (x)` expands to the token sequence `( x )`.
        let params = if self.peek() == Some(b'(') {
            self.advance();
            Some(self.scan_macro_params()?)
        } else {
            None
        };

        self.skip_inline_whitespace();
        let replacement = self.scan_replacement_list();

        let def = match params {
            Some(params) => MacroDef::Function {
                params,
                replacement,
            },
            None => MacroDef::Object { replacement },
        };
        Some(Directive::Define { name, def })
    }

    /// Parses a function-like macro's parameter list, with `(` consumed.
    fn scan_macro_params(&mut self) -> Option<Vec<String>> {
        let mut params = Vec::new();
        self.skip_inline_whitespace();

        if self.peek() == Some(b')') {
            self.advance();
            return Some(params);
        }

        loop {
            self.skip_inline_whitespace();
            params.push(self.scan_identifier()?);
            self.skip_inline_whitespace();
            match self.advance() {
                Some(b',') => continue,
                Some(b')') => return Some(params),
                _ => return None,
            }
        }
    }

    /// Tokenizes a macro replacement list, which runs to the end of the line.
    ///
    /// A backslash immediately before a newline splices the following line into
    /// this one, so multi-line macro bodies are supported.
    fn scan_replacement_list(&mut self) -> Vec<Token<'a>> {
        let mut body = String::new();

        loop {
            match self.peek() {
                None | Some(b'\n') => break,
                Some(b'\\') if self.is_line_continuation() => {
                    self.advance(); // '\'
                    if self.peek() == Some(b'\r') {
                        self.advance();
                    }
                    self.advance(); // '\n'
                }
                Some(_) => {
                    let start = self.pos;
                    self.advance();
                    body.push_str(&self.input[start..self.pos]);
                }
            }
        }

        // The spliced body is no longer a slice of the input, so it is scanned
        // separately and the resulting tokens own their text.
        let mut sub_scanner = Scanner::new(&body);
        let mut tokens = Vec::new();
        loop {
            let token = sub_scanner.next_token();
            if token.kind == TokenKind::Eof {
                return tokens;
            }
            tokens.push(token.to_owned_at(token.span));
        }
    }

    /// Whether the backslash under the cursor splices the next line in.
    fn is_line_continuation(&self) -> bool {
        match self.peek_at(1) {
            Some(b'\n') => true,
            Some(b'\r') => self.peek_at(2) == Some(b'\n'),
            _ => false,
        }
    }

    /// Scans an identifier at the cursor, without skipping whitespace first.
    fn scan_identifier(&mut self) -> Option<String> {
        let c = self.peek()?;
        if !c.is_ascii_alphabetic() && c != b'_' {
            return None;
        }
        let start = self.pos;
        self.advance();
        self.consume_while(|c| c.is_ascii_alphanumeric() || c == b'_');
        Some(self.input[start..self.pos].to_owned())
    }

    /// Scans an integer or floating-point literal, with the first digit consumed.
    fn scan_number(&mut self) -> TokenKind {
        self.consume_while(|c| c.is_ascii_digit());

        if self.peek() != Some(b'.') {
            return TokenKind::IntegerLiteral;
        }
        self.advance();
        self.consume_while(|c| c.is_ascii_digit());
        TokenKind::FloatLiteral
    }

    /// Scans a string or character literal, with the opening `quote` consumed.
    ///
    /// A backslash escapes the next character, so `'\''` and `"\""` do not end
    /// the literal early.
    fn scan_quoted(&mut self, quote: u8, kind: TokenKind, unterminated: LexError) -> TokenKind {
        while let Some(c) = self.advance() {
            if c == b'\\' {
                self.advance();
            } else if c == quote {
                return kind;
            }
        }
        TokenKind::Error(unterminated)
    }

    /// Scans a character literal, with the opening `'` consumed.
    ///
    /// The literal is delimited first and only then decoded, so that a
    /// malformed one is reported as what it is -- `''` as an empty literal,
    /// `'ab'` as a multi-character one, `'\\q'` as an unknown escape -- rather
    /// than as a token whose value the parser would have to invent.
    ///
    /// # Arguments
    ///
    /// * `start` - byte position of the opening quote, so the literal can be
    ///   re-read once its extent is known
    fn scan_char(&mut self, start: usize) -> TokenKind {
        match self.scan_quoted(b'\'', TokenKind::CharLiteral, LexError::UnterminatedChar) {
            TokenKind::CharLiteral => match char_literal_value(&self.input[start..self.pos]) {
                Ok(_) => TokenKind::CharLiteral,
                Err(error) => TokenKind::Error(error),
            },
            unterminated => unterminated,
        }
    }

    /// Consumes one follower byte when it matches, implementing maximal munch.
    ///
    /// `alternatives` pairs a byte that may follow the operator just consumed
    /// with the compound token it forms; `fallback` is the single-character
    /// token when none of them follows.
    fn munch(&mut self, alternatives: &[(u8, TokenKind)], fallback: TokenKind) -> TokenKind {
        let Some(next) = self.peek() else {
            return fallback;
        };
        for &(byte, kind) in alternatives {
            if byte == next {
                self.advance();
                return kind;
            }
        }
        fallback
    }

    /// Skips whitespace, line comments and block comments.
    fn skip_trivia(&mut self) {
        loop {
            match self.peek() {
                Some(c) if c.is_ascii_whitespace() => {
                    self.advance();
                }
                Some(b'/') => match self.peek_at(1) {
                    Some(b'/') => self.consume_while(|c| c != b'\n'),
                    Some(b'*') => self.skip_block_comment(),
                    // A lone `/` is the division operator, not trivia.
                    _ => return,
                },
                _ => return,
            }
        }
    }

    /// Skips `/* ... */`, with the cursor on the opening `/`.
    ///
    /// An unterminated block comment runs to the end of input; the scanner
    /// reports no error for it yet.
    fn skip_block_comment(&mut self) {
        self.advance(); // '/'
        self.advance(); // '*'
        while let Some(c) = self.advance() {
            if c == b'*' && self.peek() == Some(b'/') {
                self.advance();
                return;
            }
        }
    }

    /// Skips spaces and tabs but stops at a newline, as directives need.
    fn skip_inline_whitespace(&mut self) {
        self.consume_while(|c| c == b' ' || c == b'\t' || c == b'\r');
    }

    /// Consumes the rest of the current line, including its newline.
    fn skip_to_next_line(&mut self) {
        self.consume_while(|c| c != b'\n');
        self.advance();
    }

    /// Byte under the cursor, without consuming it.
    fn peek(&self) -> Option<u8> {
        self.peek_at(0)
    }

    /// Byte `offset` positions ahead of the cursor, without consuming it.
    fn peek_at(&self, offset: usize) -> Option<u8> {
        self.bytes.get(self.pos + offset).copied()
    }

    /// Consumes one byte, keeping the line and column counters in step.
    fn advance(&mut self) -> Option<u8> {
        let &c = self.bytes.get(self.pos)?;
        self.pos += 1;
        if c == b'\n' {
            self.line += 1;
            self.column = 1;
        } else {
            self.column += 1;
        }
        Some(c)
    }

    /// Consumes bytes for as long as `predicate` holds.
    fn consume_while(&mut self, mut predicate: impl FnMut(u8) -> bool) {
        while self.peek().is_some_and(&mut predicate) {
            self.advance();
        }
    }

    /// Records the current position so a span can be closed once the extent of
    /// the token is known.
    fn mark(&self) -> Mark {
        Mark {
            pos: self.pos,
            line: self.line,
            column: self.column,
        }
    }
}

/// A saved cursor position.
#[derive(Clone, Copy)]
struct Mark {
    pos: usize,
    line: usize,
    column: usize,
}

impl Mark {
    /// Builds the span running from this mark to `end_pos`.
    ///
    /// Positions are narrowed to the `u32` a [`Span`] stores; a file large
    /// enough to overflow one cannot be compiled anyway.
    fn span_to(self, end_pos: usize) -> Span {
        Span::new(
            self.line as u32,
            self.column as u32,
            self.pos as u32,
            end_pos.saturating_sub(self.pos) as u32,
        )
    }
}
