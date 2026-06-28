//! C lexer implementation with basic built-in macro expansion
//! Supports:
//!   - Object-like macros:   #define X 5
//!   - Function-like macros: #define ADD(a,b) ((a)+(b))
//! Limitations:
//!   - No nested macro expansion in replacement output
//!   - Only #define is handled
//!   - No stringification/token pasting/variadics
//!   - No conditional compilation yet

use std::borrow::Cow;
use std::collections::{HashMap, VecDeque};

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    // Keywords
    //Auto, Unsupported for now
    Break,
    Case,
    Char,
    Const,
    Continue,
    Default,
    Do,
    Double,
    Else,
    Enum,
    Extern,
    Float,
    For,
    Goto,
    If,
    Int,
    Long,
    //Register, Unsupported for now
    Return,
    Short,
    Signed,
    Sizeof,
    Static,
    Struct,
    Switch,
    Typedef,
    Union,
    Unsigned,
    Void,
    Volatile,
    While,

    // Identifier and Literals
    Identifier,
    IntegerLiteral,
    FloatLiteral,
    StringLiteral,
    CharLiteral,

    // Operators and Punctuation
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Eq,
    EqEq,
    Bang,
    BangEq,
    Less,
    LessEq,
    Greater,
    GreaterEq,
    Ampersand,
    AmpAmp,
    Pipe,
    PipePipe,
    Caret,
    Tilde,
    Shl,
    Shr,
    PlusPlus,
    MinusMinus,
    PlusEq,
    MinusEq,
    StarEq,
    SlashEq,
    PercentEq,
    AmpEq,
    PipeEq,
    CaretEq,
    ShlEq,
    ShrEq,

    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Semicolon,
    Colon,
    Dot,
    Arrow,
    Question,

    Eof,
    Error(LexError),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LexError {
    UnexpectedChar,
    UnterminatedString,
    UnterminatedChar,
    //UnterminatedBlockComment, TODO: Throw error in skip_whitespace_and_comments()
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Span {
    pub line: usize,
    pub column: usize,
    pub length: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Token<'a> {
    pub kind: TokenKind,
    pub lexeme: Cow<'a, str>,
    pub span: Span,
}

#[derive(Debug, Clone)]
enum MacroDef<'a> {
    Object {
        replacement: Vec<Token<'a>>,
    },
    Function {
        params: Vec<String>,
        replacement: Vec<Token<'a>>,
    },
}

pub struct Lexer<'a> {
    input: &'a str,
    bytes: &'a [u8],
    pos: usize,
    line: usize,
    column: usize,

    at_line_start: bool,

    macros: HashMap<String, MacroDef<'a>>,
    expanded: VecDeque<Token<'a>>,
    lookahead: VecDeque<Token<'a>>,
}

impl<'a> Lexer<'a> {
    pub fn new(input: &'a str) -> Self {
        Self {
            input,
            bytes: input.as_bytes(),
            pos: 0,
            line: 1,
            column: 1,
            at_line_start: true,
            macros: HashMap::new(),
            expanded: VecDeque::new(),
            lookahead: VecDeque::new(),
        }
    }

    /// Returns the next character without consuming it
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    /// Returns the character after the next without consuming it
    fn peek_next(&self) -> Option<u8> {
        self.bytes.get(self.pos + 1).copied()
    }

    /// Consumes the next character and advances line/column counters
    fn advance(&mut self) -> Option<u8> {
        if let Some(&c) = self.bytes.get(self.pos) {
            self.pos += 1;
            if c == b'\n' {
                self.line += 1;
                self.column = 1;
                self.at_line_start = true;
            } else {
                self.column += 1;
            }
            Some(c)
        } else {
            None
        }
    }

    /// Consumes characters while the predicate is true
    fn consume_while<F>(&mut self, mut predicate: F)
    where
        F: FnMut(u8) -> bool,
    {
        while let Some(c) = self.peek() {
            if predicate(c) {
                self.advance();
            } else {
                break;
            }
        }
    }

    fn skip_whitespace_and_comments(&mut self) {
        loop {
            match self.peek() {
                Some(b' ') | Some(b'\t') | Some(b'\r') => {
                    self.advance();
                }
                Some(b'\n') => {
                    self.advance();
                }
                Some(b'/') => match self.peek_next() {
                    Some(b'/') => {
                        // Line comment
                        self.advance(); // '/'
                        self.advance(); // '/'
                        self.consume_while(|c| c != b'\n');
                    }
                    Some(b'*') => {
                        // Block comment
                        self.advance(); // consume '/'
                        self.advance(); // consume '*'
                        loop {
                            match self.advance() {
                                Some(b'*') if self.peek() == Some(b'/') => {
                                    self.advance(); // consume '/'
                                    break;
                                }
                                Some(_) => continue,
                                None => break, // Handle unterminated gracefully in parser if needed
                            }
                        }
                    }
                    _ => break,
                },
                _ => break,
            }
        }
    }

    fn check_keyword(lexeme: &str) -> Option<TokenKind> {
        match lexeme {
            //"auto" => Some(TokenKind::Auto),
            "break" => Some(TokenKind::Break),
            "case" => Some(TokenKind::Case),
            "char" => Some(TokenKind::Char),
            "const" => Some(TokenKind::Const),
            "continue" => Some(TokenKind::Continue),
            "default" => Some(TokenKind::Default),
            "do" => Some(TokenKind::Do),
            "double" => Some(TokenKind::Double),
            "else" => Some(TokenKind::Else),
            "enum" => Some(TokenKind::Enum),
            "extern" => Some(TokenKind::Extern),
            "float" => Some(TokenKind::Float),
            "for" => Some(TokenKind::For),
            "goto" => Some(TokenKind::Goto),
            "if" => Some(TokenKind::If),
            "int" => Some(TokenKind::Int),
            "long" => Some(TokenKind::Long),
            //"register" => Some(TokenKind::Register),
            "return" => Some(TokenKind::Return),
            "short" => Some(TokenKind::Short),
            "signed" => Some(TokenKind::Signed),
            "sizeof" => Some(TokenKind::Sizeof),
            "static" => Some(TokenKind::Static),
            "struct" => Some(TokenKind::Struct),
            "switch" => Some(TokenKind::Switch),
            "typedef" => Some(TokenKind::Typedef),
            "union" => Some(TokenKind::Union),
            "unsigned" => Some(TokenKind::Unsigned),
            "void" => Some(TokenKind::Void),
            "volatile" => Some(TokenKind::Volatile),
            "while" => Some(TokenKind::While),
            _ => None,
        }
    }

    fn make_token_borrowed(
        &self,
        kind: TokenKind,
        start_pos: usize,
        end_pos: usize,
        line: usize,
        col: usize,
    ) -> Token<'a> {
        Token {
            kind,
            lexeme: Cow::Borrowed(&self.input[start_pos..end_pos]),
            span: Span {
                line,
                column: col,
                length: end_pos.saturating_sub(start_pos),
            },
        }
    }

    fn make_token_owned(kind: TokenKind, lexeme: String, span: Span) -> Token<'a> {
        Token {
            kind,
            lexeme: Cow::Owned(lexeme),
            span,
        }
    }

    fn next_raw_token(&mut self) -> Token<'a> {
        self.skip_whitespace_and_comments();

        let start_pos = self.pos;
        let start_line = self.line;
        let start_col = self.column;

        let Some(c) = self.advance() else {
            return Token {
                kind: TokenKind::Eof,
                lexeme: Cow::Borrowed(""),
                span: Span {
                    line: start_line,
                    column: start_col,
                    length: 0,
                },
            };
        };

        self.at_line_start = false;

        let kind = match c {
            // Identifiers and Keywords
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => {
                self.consume_while(|c| c.is_ascii_alphanumeric() || c == b'_');
                let lexeme = &self.input[start_pos..self.pos];
                Self::check_keyword(lexeme).unwrap_or(TokenKind::Identifier)
            }

            // Numeric Literals
            b'0'..=b'9' => {
                let mut is_float = false;
                self.consume_while(|c| c.is_ascii_digit());

                if self.peek() == Some(b'.') {
                    is_float = true;
                    self.advance();
                    self.consume_while(|c| c.is_ascii_digit());
                }

                if is_float {
                    TokenKind::FloatLiteral
                } else {
                    TokenKind::IntegerLiteral
                }
            }

            // String Literals
            b'"' => {
                let mut closed = false;
                while let Some(c) = self.advance() {
                    if c == b'\\' {
                        self.advance(); // Skip escaped char
                    } else if c == b'"' {
                        closed = true;
                        break;
                    }
                }
                if closed {
                    TokenKind::StringLiteral
                } else {
                    TokenKind::Error(LexError::UnterminatedString)
                }
            }

            // Char Literals
            b'\'' => {
                let mut closed = false;
                while let Some(c) = self.advance() {
                    if c == b'\\' {
                        self.advance();
                    } else if c == b'\'' {
                        closed = true;
                        break;
                    }
                }
                if closed {
                    TokenKind::CharLiteral
                } else {
                    TokenKind::Error(LexError::UnterminatedChar)
                }
            }

            // Operators (Applying Maximal Munch Principle)
            b'+' => {
                if self.peek() == Some(b'+') {
                    self.advance();
                    TokenKind::PlusPlus
                } else if self.peek() == Some(b'=') {
                    self.advance();
                    TokenKind::PlusEq
                } else {
                    TokenKind::Plus
                }
            }
            b'-' => {
                if self.peek() == Some(b'-') {
                    self.advance();
                    TokenKind::MinusMinus
                } else if self.peek() == Some(b'>') {
                    self.advance();
                    TokenKind::Arrow
                } else if self.peek() == Some(b'=') {
                    self.advance();
                    TokenKind::MinusEq
                } else {
                    TokenKind::Minus
                }
            }
            b'=' => {
                if self.peek() == Some(b'=') {
                    self.advance();
                    TokenKind::EqEq
                } else {
                    TokenKind::Eq
                }
            }
            b'!' => {
                if self.peek() == Some(b'=') {
                    self.advance();
                    TokenKind::BangEq
                } else {
                    TokenKind::Bang
                }
            }
            b'<' => {
                if self.peek() == Some(b'=') {
                    self.advance();
                    TokenKind::LessEq
                } else if self.peek() == Some(b'<') {
                    self.advance();
                    if self.peek() == Some(b'=') {
                        self.advance();
                        TokenKind::ShlEq
                    } else {
                        TokenKind::Shl
                    }
                } else if self.peek() == Some(b'%') {
                    self.advance();
                    TokenKind::LBrace
                } else if self.peek() == Some(b':') {
                    self.advance();
                    TokenKind::LBracket
                } else {
                    TokenKind::Less
                }
            }
            b'>' => {
                if self.peek() == Some(b'=') {
                    self.advance();
                    TokenKind::GreaterEq
                } else if self.peek() == Some(b'>') {
                    self.advance();
                    if self.peek() == Some(b'=') {
                        self.advance();
                        TokenKind::ShrEq
                    } else {
                        TokenKind::Shr
                    }
                } else {
                    TokenKind::Greater
                }
            }

            b'*' => {
                if self.peek() == Some(b'=') {
                    self.advance();
                    TokenKind::StarEq
                } else {
                    TokenKind::Star
                }
            }
            b'/' => {
                if self.peek() == Some(b'=') {
                    self.advance();
                    TokenKind::SlashEq
                } else {
                    TokenKind::Slash
                }
            }
            b'%' => {
                if self.peek() == Some(b'=') {
                    self.advance();
                    TokenKind::PercentEq
                } else if self.peek() == Some(b'>') {
                    self.advance();
                    TokenKind::RBrace
                } else {
                    TokenKind::Percent
                }
            }
            b'&' => {
                if self.peek() == Some(b'&') {
                    self.advance();
                    TokenKind::AmpAmp
                } else if self.peek() == Some(b'=') {
                    self.advance();
                    TokenKind::AmpEq
                } else {
                    TokenKind::Ampersand
                }
            }
            b'|' => {
                if self.peek() == Some(b'|') {
                    self.advance();
                    TokenKind::PipePipe
                } else if self.peek() == Some(b'=') {
                    self.advance();
                    TokenKind::PipeEq
                } else {
                    TokenKind::Pipe
                }
            }
            b'^' => {
                if self.peek() == Some(b'=') {
                    self.advance();
                    TokenKind::CaretEq
                } else {
                    TokenKind::Caret
                }
            }
            b'~' => TokenKind::Tilde,
            b'?' => TokenKind::Question,
            b':' => {
                if self.peek() == Some(b'>') {
                    self.advance();
                    TokenKind::RBracket
                } else {
                    TokenKind::Colon
                }
            }

            // Preprocessor marker (handled in wrapper when line-start)
            b'#' => TokenKind::Error(LexError::UnexpectedChar),

            // Basic Punctuation
            b'(' => TokenKind::LParen,
            b')' => TokenKind::RParen,
            b'{' => TokenKind::LBrace,
            b'}' => TokenKind::RBrace,
            b'[' => TokenKind::LBracket,
            b']' => TokenKind::RBracket,
            b';' => TokenKind::Semicolon,
            b',' => TokenKind::Comma,
            b'.' => TokenKind::Dot,

            _ => TokenKind::Error(LexError::UnexpectedChar),
        };

        self.make_token_borrowed(kind, start_pos, self.pos, start_line, start_col)
    }

    fn next_raw_or_buffered(&mut self) -> Token<'a> {
        if let Some(t) = self.lookahead.pop_front() {
            t
        } else {
            self.next_raw_token()
        }
    }

    fn push_front_raw(&mut self, t: Token<'a>) {
        self.lookahead.push_front(t);
    }

    fn skip_inline_ws(&mut self) {
        while matches!(self.peek(), Some(b' ') | Some(b'\t') | Some(b'\r')) {
            self.advance();
        }
    }

    fn parse_identifier_at_pos(&mut self) -> Option<String> {
        let Some(c) = self.peek() else { return None };
        if !(c.is_ascii_alphabetic() || c == b'_') {
            return None;
        }
        let start = self.pos;
        self.advance();
        self.consume_while(|b| b.is_ascii_alphanumeric() || b == b'_');
        Some(self.input[start..self.pos].to_string())
    }

    fn parse_define_directive(&mut self) {
        // We are at '#' already.
        self.advance(); // '#'
        self.skip_inline_ws();

        let Some(dir) = self.parse_identifier_at_pos() else {
            self.consume_until_newline();
            return;
        };

        if dir != "define" {
            self.consume_until_newline();
            return;
        }

        self.skip_inline_ws();

        let Some(name) = self.parse_identifier_at_pos() else {
            self.consume_until_newline();
            return;
        };

        // function-like only when '(' follows immediately after macro name
        let is_function_like = self.peek() == Some(b'(');

        if is_function_like {
            self.advance(); // '('
            let mut params = Vec::<String>::new();
            self.skip_inline_ws();

            if self.peek() != Some(b')') {
                loop {
                    self.skip_inline_ws();
                    let Some(param) = self.parse_identifier_at_pos() else {
                        self.consume_until_newline();
                        return;
                    };
                    params.push(param);
                    self.skip_inline_ws();
                    match self.peek() {
                        Some(b',') => {
                            self.advance();
                        }
                        Some(b')') => break,
                        _ => {
                            self.consume_until_newline();
                            return;
                        }
                    }
                }
            }

            if self.peek() == Some(b')') {
                self.advance(); // ')'
            } else {
                self.consume_until_newline();
                return;
            }

            let replacement = self.read_replacement_tokens_until_eol();
            self.macros.insert(
                name,
                MacroDef::Function {
                    params,
                    replacement,
                },
            );
        } else {
            self.skip_inline_ws();
            let replacement = self.read_replacement_tokens_until_eol();
            self.macros.insert(name, MacroDef::Object { replacement });
        }

        // consume trailing newline if still present
        if self.peek() == Some(b'\n') {
            self.advance();
        }
    }

    fn consume_until_newline(&mut self) {
        while let Some(c) = self.peek() {
            if c == b'\n' {
                break;
            }
            self.advance();
        }
    }

    fn read_replacement_tokens_until_eol(&mut self) -> Vec<Token<'a>> {
        // Collect replacement text, honoring line continuation with backslash-newline.
        let mut buf = String::new();

        loop {
            match self.peek() {
                None => break,
                Some(b'\n') => {
                    // End of macro body on plain newline
                    break;
                }
                Some(b'\\') => {
                    // Possible line continuation
                    if self.peek_next() == Some(b'\n') {
                        // Consume "\" + "\n" and continue (splice lines)
                        self.advance(); // '\'
                        self.advance(); // '\n'
                        continue;
                    } else if self.peek_next() == Some(b'\r') {
                        // Handle "\" + "\r\n"
                        let save_pos = self.pos;
                        self.advance(); // '\'
                        self.advance(); // '\r'
                        if self.peek() == Some(b'\n') {
                            self.advance(); // '\n'
                            continue;
                        } else {
                            // Not actually CRLF continuation, restore and treat '\' as normal char
                            self.pos = save_pos;
                            // rebuild line/column approximately by consuming as normal below
                        }
                    }

                    // Normal backslash character
                    buf.push('\\');
                    self.advance();
                }
                Some(_) => {
                    let start = self.pos;
                    self.advance();
                    buf.push_str(&self.input[start..self.pos]);
                }
            }
        }

        // Tokenize collected replacement text with a sub-lexer.
        let mut sub = Lexer::new(&buf);
        let mut out = Vec::new();
        loop {
            let t = sub.next_raw_token();
            if t.kind == TokenKind::Eof {
                break;
            }
            out.push(Token {
                kind: t.kind,
                lexeme: Cow::Owned(t.lexeme.into_owned()),
                span: t.span,
            });
        }
        out
    }

    fn handle_directive_if_present(&mut self) -> bool {
        self.skip_whitespace_and_comments();

        let save_pos = self.pos;
        let save_line = self.line;
        let save_col = self.column;
        let save_at_line_start = self.at_line_start;

        while matches!(self.peek(), Some(b' ') | Some(b'\t') | Some(b'\r')) {
            self.advance();
        }

        if self.peek() == Some(b'#') {
            self.parse_define_directive();
            true
        } else {
            self.pos = save_pos;
            self.line = save_line;
            self.column = save_col;
            self.at_line_start = save_at_line_start;
            false
        }
    }

    fn try_expand_object_macro(&mut self, ident: &Token<'a>) -> bool {
        let name = ident.lexeme.as_ref();
        let Some(def) = self.macros.get(name) else {
            return false;
        };

        match def {
            MacroDef::Object { replacement } => {
                for t in replacement.iter().cloned() {
                    self.expanded.push_back(Token {
                        kind: t.kind,
                        lexeme: Cow::Owned(t.lexeme.into_owned()),
                        span: ident.span,
                    });
                }
                true
            }
            _ => false,
        }
    }

    fn try_expand_function_macro(&mut self, ident: &Token<'a>) -> bool {
        let name = ident.lexeme.as_ref();
        let Some(def) = self.macros.get(name).cloned() else {
            return false;
        };

        let MacroDef::Function {
            params,
            replacement,
        } = def
        else {
            return false;
        };

        // Need immediate '(' token next (ignoring whitespace because lexer already skipped it)
        let next = self.next_raw_or_buffered();
        if next.kind != TokenKind::LParen {
            self.push_front_raw(next);
            return false;
        }

        // Parse args with balanced parens
        let mut args: Vec<Vec<Token<'a>>> = Vec::new();
        let mut cur: Vec<Token<'a>> = Vec::new();
        let mut depth: i32 = 0;

        loop {
            let t = self.next_raw_or_buffered();
            match t.kind {
                TokenKind::Eof => return false,
                TokenKind::LParen => {
                    depth += 1;
                    cur.push(t);
                }
                TokenKind::RParen => {
                    if depth == 0 {
                        args.push(cur);
                        break;
                    } else {
                        depth -= 1;
                        cur.push(t);
                    }
                }
                TokenKind::Comma if depth == 0 => {
                    args.push(cur);
                    cur = Vec::new();
                }
                _ => cur.push(t),
            }
        }

        // Handle zero-arg call case: MACRO()
        if params.is_empty() && args.len() == 1 && args[0].is_empty() {
            args.clear();
        }

        if args.len() != params.len() {
            // arity mismatch -> do not expand; emit original call tokens as parsed
            self.expanded.push_back(ident.clone());
            self.expanded.push_back(Token::<'a> {
                kind: TokenKind::LParen,
                lexeme: Cow::Owned("(".to_string()),
                span: ident.span,
            });

            let arg_count = args.len();
            for (i, a) in args.iter().enumerate() {
                for t in a.iter().cloned() {
                    self.expanded.push_back(t);
                }
                if i + 1 < arg_count {
                    self.expanded.push_back(Token::<'a> {
                        kind: TokenKind::Comma,
                        lexeme: Cow::Owned(",".to_string()),
                        span: ident.span,
                    });
                }
            }

            self.expanded.push_back(Token::<'a> {
                kind: TokenKind::RParen,
                lexeme: Cow::Owned(")".to_string()),
                span: ident.span,
            });
            return true;
        }

        let mut param_map: HashMap<String, Vec<Token<'a>>> = HashMap::new();
        for (p, a) in params.iter().zip(args.into_iter()) {
            param_map.insert(p.clone(), a);
        }

        // Substitute only parameters; do not recursively expand resulting identifiers.
        for rt in replacement {
            if rt.kind == TokenKind::Identifier {
                let key = rt.lexeme.as_ref();
                if let Some(arg_tokens) = param_map.get(key) {
                    for a in arg_tokens.iter().cloned() {
                        self.expanded.push_back(Token {
                            kind: a.kind,
                            lexeme: Cow::Owned(a.lexeme.into_owned()),
                            span: ident.span,
                        });
                    }
                    continue;
                }
            }

            self.expanded.push_back(Token {
                kind: rt.kind,
                lexeme: Cow::Owned(rt.lexeme.into_owned()),
                span: ident.span,
            });
        }

        true
    }

    pub fn next_token(&mut self) -> Token<'a> {
        loop {
            if let Some(t) = self.expanded.pop_front() {
                return t;
            }

            if self.handle_directive_if_present() {
                continue;
            }

            let t = self.next_raw_or_buffered();

            if t.kind == TokenKind::Identifier {
                if self.try_expand_function_macro(&t) {
                    continue;
                }
                if self.try_expand_object_macro(&t) {
                    continue;
                }
            }

            return t;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper function to collect all tokens from the input until Eof.
    fn lex_all(input: &str) -> Vec<Token> {
        let mut lexer = Lexer::new(input);
        let mut tokens = Vec::new();
        loop {
            let token = lexer.next_token();
            if token.kind == TokenKind::Eof {
                break;
            }
            tokens.push(token);
        }
        tokens
    }

    fn lex_all_kinds(input: &str) -> Vec<TokenKind> {
        let mut lx = Lexer::new(input);
        let mut out = Vec::new();
        loop {
            let t = lx.next_token();
            out.push(t.kind.clone());
            if t.kind == TokenKind::Eof {
                break;
            }
        }
        out
    }

    fn lex_all_lexemes(input: &str) -> Vec<String> {
        let mut lx = Lexer::new(input);
        let mut out = Vec::new();
        loop {
            let t = lx.next_token();
            out.push(t.lexeme.to_string());
            if t.kind == TokenKind::Eof {
                break;
            }
        }
        out
    }

    #[test]
    fn test_keywords_and_identifiers() {
        let input = "int main auto_var";
        let tokens = lex_all(input);

        assert_eq!(tokens.len(), 3);
        assert_eq!(tokens[0].kind, TokenKind::Int);
        assert_eq!(tokens[1].kind, TokenKind::Identifier);
        assert_eq!(tokens[1].lexeme, "main");
        assert_eq!(tokens[2].kind, TokenKind::Identifier);
        assert_eq!(tokens[2].lexeme, "auto_var");
    }

    #[test]
    fn test_numeric_literals() {
        let input = "42 3.14 0";
        let tokens = lex_all(input);

        assert_eq!(tokens[0].kind, TokenKind::IntegerLiteral);
        assert_eq!(tokens[0].lexeme, "42");
        assert_eq!(tokens[1].kind, TokenKind::FloatLiteral);
        assert_eq!(tokens[1].lexeme, "3.14");
        assert_eq!(tokens[2].kind, TokenKind::IntegerLiteral);
        assert_eq!(tokens[2].lexeme, "0");
    }

    #[test]
    fn test_strings_and_chars() {
        let input = r#" "hello, world!" 'c' "#;
        let tokens = lex_all(input);

        assert_eq!(tokens[0].kind, TokenKind::StringLiteral);
        assert_eq!(tokens[0].lexeme, "\"hello, world!\"");
        assert_eq!(tokens[1].kind, TokenKind::CharLiteral);
        assert_eq!(tokens[1].lexeme, "'c'");
    }

    #[test]
    fn test_operators_maximal_munch() {
        // Ensures that ++ is parsed as PlusPlus, not Plus, Plus
        let input = "+ ++ += -> <<=";
        let tokens = lex_all(input);

        assert_eq!(tokens[0].kind, TokenKind::Plus);
        assert_eq!(tokens[1].kind, TokenKind::PlusPlus);
        assert_eq!(tokens[2].kind, TokenKind::PlusEq);
        assert_eq!(tokens[3].kind, TokenKind::Arrow);
        assert_eq!(tokens[4].kind, TokenKind::ShlEq);
    }

    #[test]
    fn test_skipping_comments_and_whitespace() {
        let input = "
            // This is a line comment
            int x = 5; /* 
            Block comment 
            */ 
            return x;
        ";
        let tokens = lex_all(input);

        // Should only see: int, x, =, 5, ;, return, x, ;
        assert_eq!(tokens.len(), 8);
        assert_eq!(tokens[0].kind, TokenKind::Int);
        assert_eq!(tokens[1].kind, TokenKind::Identifier);
        assert_eq!(tokens[5].kind, TokenKind::Return);
    }

    #[test]
    fn test_line_and_column_tracking() {
        let input = "int a;\n  a = 10;";
        let tokens = lex_all(input);

        // 'int'
        assert_eq!(tokens[0].span.line, 1);
        assert_eq!(tokens[0].span.column, 1);

        // 'a'
        assert_eq!(tokens[1].span.line, 1);
        assert_eq!(tokens[1].span.column, 5);

        // 'a' on the second line
        assert_eq!(tokens[3].span.line, 2);
        assert_eq!(tokens[3].span.column, 3);

        // '='
        assert_eq!(tokens[4].span.line, 2);
        assert_eq!(tokens[4].span.column, 5);
    }

    #[test]
    fn test_unterminated_errors() {
        let input = "\"unterminated string";
        let tokens = lex_all(input);

        assert_eq!(
            tokens[0].kind,
            TokenKind::Error(LexError::UnterminatedString)
        );

        let input2 = "'a";
        let tokens2 = lex_all(input2);
        assert_eq!(
            tokens2[0].kind,
            TokenKind::Error(LexError::UnterminatedChar)
        );
    }
    #[test]
    fn object_macro_basic_expands() {
        let input = r#"
#define X 5
int a = X;
"#;
        let lex = lex_all_lexemes(input);
        assert_eq!(lex, vec!["int", "a", "=", "5", ";", ""]);
    }

    #[test]
    fn object_macro_does_not_expand_as_substring() {
        let input = r#"
#define X 5
int Xy = 1;
"#;
        let lex = lex_all_lexemes(input);
        assert_eq!(lex, vec!["int", "Xy", "=", "1", ";", ""]);
    }

    #[test]
    fn object_macro_multiple_use_sites() {
        let input = r#"
#define N 42
int a = N;
int b = N;
"#;
        let lex = lex_all_lexemes(input);
        assert_eq!(
            lex,
            vec!["int", "a", "=", "42", ";", "int", "b", "=", "42", ";", ""]
        );
    }

    #[test]
    fn function_macro_basic_expands() {
        let input = r#"
#define ADD(a,b) ((a)+(b))
int z = ADD(1,2);
"#;
        let lex = lex_all_lexemes(input);
        assert_eq!(
            lex,
            vec![
                "int", "z", "=", "(", "(", "1", ")", "+", "(", "2", ")", ")", ";", ""
            ]
        );
    }

    #[test]
    fn function_macro_handles_nested_paren_args() {
        let input = r#"
#define ID(x) x
int a = ID((1+2));
"#;
        let lex = lex_all_lexemes(input);
        assert_eq!(lex, vec!["int", "a", "=", "(", "1", "+", "2", ")", ";", ""]);
    }

    #[test]
    fn function_macro_zero_args() {
        let input = r#"
#define F() 99
int x = F();
"#;
        let lex = lex_all_lexemes(input);
        assert_eq!(lex, vec!["int", "x", "=", "99", ";", ""]);
    }

    #[test]
    fn function_macro_arity_mismatch_falls_back_to_call_tokens() {
        let input = r#"
#define ADD(a,b) ((a)+(b))
int z = ADD(1);
"#;
        let lex = lex_all_lexemes(input);

        // Current implementation fallback emits original call shape
        assert_eq!(lex, vec!["int", "z", "=", "ADD", "(", "1", ")", ";", ""]);
    }

    #[test]
    fn function_macro_requires_call_syntax() {
        let input = r#"
#define INC(x) ((x)+1)
int a = INC;
"#;
        let lex = lex_all_lexemes(input);

        // Not followed by '(' => stays identifier
        assert_eq!(lex, vec!["int", "a", "=", "INC", ";", ""]);
    }

    #[test]
    fn directive_only_line_produces_no_runtime_tokens() {
        let input = r#"
#define X 5
"#;
        let kinds = lex_all_kinds(input);
        assert_eq!(kinds, vec![TokenKind::Eof]);
    }

    #[test]
    fn non_define_directive_is_ignored() {
        let input = r#"
#unknown stuff here
int a = 1;
"#;
        let lex = lex_all_lexemes(input);
        assert_eq!(lex, vec!["int", "a", "=", "1", ";", ""]);
    }

    #[test]
    fn define_with_leading_whitespace_at_line_start_works() {
        let input = "   #define X 7\nint y=X;\n";
        let lex = lex_all_lexemes(input);
        assert_eq!(lex, vec!["int", "y", "=", "7", ";", ""]);
    }

    #[test]
    fn no_nested_expansion_in_replacement() {
        let input = r#"
#define A B
#define B 9
int x = A;
"#;
        let lex = lex_all_lexemes(input);

        // As requested: no nested expansion. A -> B, stop there.
        assert_eq!(lex, vec!["int", "x", "=", "B", ";", ""]);
    }

    #[test]
    fn macro_can_expand_to_multiple_tokens() {
        let input = r#"
#define PAIR 1,2
int a[] = { PAIR };
"#;
        let lex = lex_all_lexemes(input);
        assert_eq!(
            lex,
            vec!["int", "a", "[", "]", "=", "{", "1", ",", "2", "}", ";", ""]
        );
    }

    #[test]
    fn string_and_char_lexing_still_work() {
        let input = r#"
char c = 'x';
char* s = "hi\n";
"#;
        let kinds = lex_all_kinds(input);
        assert_eq!(
            kinds,
            vec![
                TokenKind::Char,
                TokenKind::Identifier,
                TokenKind::Eq,
                TokenKind::CharLiteral,
                TokenKind::Semicolon,
                TokenKind::Char,
                TokenKind::Star,
                TokenKind::Identifier,
                TokenKind::Eq,
                TokenKind::StringLiteral,
                TokenKind::Semicolon,
                TokenKind::Eof
            ]
        );
    }

    #[test]
    fn diagraphs() {
        let input = r#"
        {
        int
        }

        <%
        int
        %>

        [int]

        <:int:>
"#;
        let kinds = lex_all_kinds(input);
        assert_eq!(
            kinds,
            vec![
                TokenKind::LBrace,
                TokenKind::Int,
                TokenKind::RBrace,
                TokenKind::LBrace,
                TokenKind::Int,
                TokenKind::RBrace,
                TokenKind::LBracket,
                TokenKind::Int,
                TokenKind::RBracket,
                TokenKind::LBracket,
                TokenKind::Int,
                TokenKind::RBracket,
                TokenKind::Eof
            ]
        );
    }
}
