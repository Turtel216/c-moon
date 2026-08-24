//! C lexer with basic built-in macro expansion.
//!
//! The lexer is layered:
//!   - [`scanner::Scanner`] turns characters into raw tokens and recognises
//!     preprocessor directives;
//!   - [`macros::MacroDef`] stores what a `#define` stands for and expands it;
//!   - [`Lexer`] drives both, so its consumers see a single macro-expanded
//!     token stream.
//!
//! Supports:
//!   - Object-like macros:   #define X 5
//!   - Function-like macros: #define ADD(a,b) ((a)+(b))
//!
//! Limitations:
//!   - No nested macro expansion in replacement output
//!   - Only #define is handled
//!   - No stringification/token pasting/variadics
//!   - No conditional compilation yet

mod macros;
mod scanner;
mod token;

pub use token::{LexError, Token, TokenKind, char_literal_value};

use std::collections::{HashMap, VecDeque};

use macros::MacroDef;
use scanner::{Directive, Scanner};

/// A macro-expanded token stream over a C source file.
pub struct Lexer<'a> {
    scanner: Scanner<'a>,
    /// Macros defined so far. A `#define` only affects the text below it, so
    /// this grows as the stream is consumed.
    macros: HashMap<String, MacroDef<'a>>,
    /// Tokens produced by an expansion, waiting to be handed out.
    pending: VecDeque<Token<'a>>,
    /// Raw tokens read while checking whether a function-like macro name is
    /// followed by an argument list, and put back when it is not.
    pushed_back: VecDeque<Token<'a>>,
}

impl<'a> Lexer<'a> {
    /// Creates a lexer over `input`.
    pub fn new(input: &'a str) -> Self {
        Self {
            scanner: Scanner::new(input),
            macros: HashMap::new(),
            pending: VecDeque::new(),
            pushed_back: VecDeque::new(),
        }
    }

    /// Returns the next token of the program.
    ///
    /// Directives are consumed silently and macro names are replaced by their
    /// expansion, so the caller only ever sees program text. Once the input is
    /// exhausted this keeps returning [`TokenKind::Eof`].
    pub fn next_token(&mut self) -> Token<'a> {
        loop {
            if let Some(token) = self.pending.pop_front() {
                return token;
            }

            // Directives are read straight from the source text, so they may
            // only be looked for when no already-scanned token is buffered.
            if self.pushed_back.is_empty() && self.consume_directive() {
                continue;
            }

            let token = self.next_raw();
            if token.kind == TokenKind::Identifier && self.try_expand(&token) {
                continue;
            }
            return token;
        }
    }

    /// Consumes a directive if one starts here, recording any `#define`.
    ///
    /// # Returns
    ///
    /// Whether a directive was consumed, in which case no token was produced
    /// and the caller must try again.
    fn consume_directive(&mut self) -> bool {
        match self.scanner.scan_directive() {
            Some(Directive::Define { name, def }) => {
                self.macros.insert(name, def);
                true
            }
            Some(Directive::Ignored) => true,
            None => false,
        }
    }

    /// Queues the expansion of `name_token` if it names a macro.
    ///
    /// # Returns
    ///
    /// Whether tokens were queued. `false` means the identifier is not a macro
    /// name -- or is a function-like macro that is not being called -- and
    /// should be returned to the caller unchanged.
    fn try_expand(&mut self, name_token: &Token<'a>) -> bool {
        let Some(def) = self.macros.get(name_token.lexeme.as_ref()) else {
            return false;
        };

        if !def.is_function_like() {
            let expanded = def.expand(&[], name_token.span);
            self.pending.extend(expanded);
            return true;
        }

        // A function-like macro name expands only when it is applied, so
        // `INC` on its own stays an identifier while `INC(x)` expands.
        let arity = def.params().len();
        let next = self.next_raw();
        if next.kind != TokenKind::LParen {
            self.pushed_back.push_front(next);
            return false;
        }

        let Some(mut args) = self.read_arguments() else {
            // The input ended inside the argument list; there is nothing left
            // to expand into.
            return false;
        };

        // `F()` parses as one empty argument, which is how a call to a macro
        // that takes no parameters is written.
        if arity == 0 && args.len() == 1 && args[0].is_empty() {
            args.clear();
        }

        if args.len() != arity {
            self.emit_unexpanded_call(name_token, &args);
            return true;
        }

        let expanded = self
            .macros
            .get(name_token.lexeme.as_ref())
            .expect("macro was looked up above")
            .expand(&args, name_token.span);
        self.pending.extend(expanded);
        true
    }

    /// Reads the arguments of a function-like macro call, with `(` consumed.
    ///
    /// Arguments are split on top-level commas, so parentheses inside an
    /// argument -- `ID((1+2))` -- keep it in one piece.
    ///
    /// # Returns
    ///
    /// `None` if the input ends before the closing `)`.
    fn read_arguments(&mut self) -> Option<Vec<Vec<Token<'a>>>> {
        let mut args = Vec::new();
        let mut current = Vec::new();
        let mut depth = 0usize;

        loop {
            let token = self.next_raw();
            match token.kind {
                TokenKind::Eof => return None,
                TokenKind::RParen if depth == 0 => {
                    args.push(current);
                    return Some(args);
                }
                TokenKind::Comma if depth == 0 => args.push(std::mem::take(&mut current)),
                TokenKind::LParen => {
                    depth += 1;
                    current.push(token);
                }
                TokenKind::RParen => {
                    depth -= 1;
                    current.push(token);
                }
                _ => current.push(token),
            }
        }
    }

    /// Re-emits a macro call that could not be expanded, as written.
    ///
    /// Used when the call's argument count does not match the definition: the
    /// tokens are handed to the parser unchanged so the error surfaces there
    /// rather than as a confusing expansion.
    fn emit_unexpanded_call(&mut self, name_token: &Token<'a>, args: &[Vec<Token<'a>>]) {
        let span = name_token.span;
        self.pending.push_back(name_token.clone());
        self.pending
            .push_back(Token::owned(TokenKind::LParen, "(", span));

        for (index, arg) in args.iter().enumerate() {
            if index > 0 {
                self.pending
                    .push_back(Token::owned(TokenKind::Comma, ",", span));
            }
            self.pending.extend(arg.iter().cloned());
        }

        self.pending
            .push_back(Token::owned(TokenKind::RParen, ")", span));
    }

    /// Returns the next raw token, preferring one that was put back.
    fn next_raw(&mut self) -> Token<'a> {
        self.pushed_back
            .pop_front()
            .unwrap_or_else(|| self.scanner.next_token())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Collects every token of `input`, including the final `Eof`.
    fn lex(input: &str) -> Vec<Token<'_>> {
        let mut lexer = Lexer::new(input);
        let mut tokens = Vec::new();
        loop {
            let token = lexer.next_token();
            let is_eof = token.kind == TokenKind::Eof;
            tokens.push(token);
            if is_eof {
                return tokens;
            }
        }
    }

    fn kinds(input: &str) -> Vec<TokenKind> {
        lex(input).iter().map(|token| token.kind).collect()
    }

    fn lexemes(input: &str) -> Vec<String> {
        lex(input)
            .iter()
            .map(|token| token.lexeme.to_string())
            .collect()
    }

    #[test]
    fn test_keywords_and_identifiers() {
        let tokens = lex("int main auto_var");

        assert_eq!(tokens.len(), 4); // three tokens plus Eof
        assert_eq!(tokens[0].kind, TokenKind::Int);
        assert_eq!(tokens[1].kind, TokenKind::Identifier);
        assert_eq!(tokens[1].lexeme, "main");
        assert_eq!(tokens[2].kind, TokenKind::Identifier);
        assert_eq!(tokens[2].lexeme, "auto_var");
    }

    #[test]
    fn test_numeric_literals() {
        let tokens = lex("42 3.14 0");

        assert_eq!(tokens[0].kind, TokenKind::IntegerLiteral);
        assert_eq!(tokens[0].lexeme, "42");
        assert_eq!(tokens[1].kind, TokenKind::FloatLiteral);
        assert_eq!(tokens[1].lexeme, "3.14");
        assert_eq!(tokens[2].kind, TokenKind::IntegerLiteral);
        assert_eq!(tokens[2].lexeme, "0");
    }

    #[test]
    fn test_strings_and_chars() {
        let tokens = lex(r#" "hello, world!" 'c' "#);

        assert_eq!(tokens[0].kind, TokenKind::StringLiteral);
        assert_eq!(tokens[0].lexeme, "\"hello, world!\"");
        assert_eq!(tokens[1].kind, TokenKind::CharLiteral);
        assert_eq!(tokens[1].lexeme, "'c'");
    }

    #[test]
    fn test_operators_maximal_munch() {
        // Ensures that ++ is parsed as PlusPlus, not Plus, Plus
        let tokens = lex("+ ++ += -> <<=");

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
        let tokens = lex(input);

        // Should only see: int, x, =, 5, ;, return, x, ; and Eof
        assert_eq!(tokens.len(), 9);
        assert_eq!(tokens[0].kind, TokenKind::Int);
        assert_eq!(tokens[1].kind, TokenKind::Identifier);
        assert_eq!(tokens[5].kind, TokenKind::Return);
    }

    #[test]
    fn test_line_and_column_tracking() {
        let tokens = lex("int a;\n  a = 10;");

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
        let tokens = lex("\"unterminated string");
        assert_eq!(
            tokens[0].kind,
            TokenKind::Error(LexError::UnterminatedString)
        );

        let tokens = lex("'a");
        assert_eq!(tokens[0].kind, TokenKind::Error(LexError::UnterminatedChar));
    }

    #[test]
    fn a_character_literal_is_decoded_as_it_is_scanned() {
        // Arrange / Act / Assert: the scanner accepts a literal only if it
        // names exactly one character, so the parser never has to guess.
        assert_eq!(lex(r"'\n'")[0].kind, TokenKind::CharLiteral);
        assert_eq!(lex("''")[0].kind, TokenKind::Error(LexError::EmptyChar));
        assert_eq!(
            lex("'ab'")[0].kind,
            TokenKind::Error(LexError::MultiCharacterChar)
        );
        assert_eq!(
            lex(r"'\q'")[0].kind,
            TokenKind::Error(LexError::UnknownEscape)
        );
        // An escaped quote does not end the literal early.
        assert_eq!(lex(r"'\''")[0].lexeme, r"'\''");
    }

    #[test]
    fn object_macro_basic_expands() {
        let input = r#"
#define X 5
int a = X;
"#;
        assert_eq!(lexemes(input), vec!["int", "a", "=", "5", ";", ""]);
    }

    #[test]
    fn object_macro_does_not_expand_as_substring() {
        let input = r#"
#define X 5
int Xy = 1;
"#;
        assert_eq!(lexemes(input), vec!["int", "Xy", "=", "1", ";", ""]);
    }

    #[test]
    fn object_macro_multiple_use_sites() {
        let input = r#"
#define N 42
int a = N;
int b = N;
"#;
        assert_eq!(
            lexemes(input),
            vec!["int", "a", "=", "42", ";", "int", "b", "=", "42", ";", ""]
        );
    }

    #[test]
    fn object_macro_use_site_keeps_its_own_span() {
        let input = "#define X 5\nint a = X;\n";
        let expansion = lex(input)
            .into_iter()
            .find(|token| token.lexeme == "5")
            .expect("macro should expand");

        // The token comes from line 1 but must be reported where it is used.
        assert_eq!(expansion.span.line, 2);
    }

    #[test]
    fn function_macro_basic_expands() {
        let input = r#"
#define ADD(a,b) ((a)+(b))
int z = ADD(1,2);
"#;
        assert_eq!(
            lexemes(input),
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
        assert_eq!(
            lexemes(input),
            vec!["int", "a", "=", "(", "1", "+", "2", ")", ";", ""]
        );
    }

    #[test]
    fn function_macro_zero_args() {
        let input = r#"
#define F() 99
int x = F();
"#;
        assert_eq!(lexemes(input), vec!["int", "x", "=", "99", ";", ""]);
    }

    #[test]
    fn function_macro_arity_mismatch_falls_back_to_call_tokens() {
        let input = r#"
#define ADD(a,b) ((a)+(b))
int z = ADD(1);
"#;

        // Current implementation fallback emits original call shape
        assert_eq!(
            lexemes(input),
            vec!["int", "z", "=", "ADD", "(", "1", ")", ";", ""]
        );
    }

    #[test]
    fn function_macro_requires_call_syntax() {
        let input = r#"
#define INC(x) ((x)+1)
int a = INC;
"#;

        // Not followed by '(' => stays identifier
        assert_eq!(lexemes(input), vec!["int", "a", "=", "INC", ";", ""]);
    }

    #[test]
    fn function_macro_body_may_span_lines() {
        let input = "#define ADD(a,b) ((a) + \\\n                 (b))\nint z = ADD(1,2);\n";
        assert_eq!(
            lexemes(input),
            vec![
                "int", "z", "=", "(", "(", "1", ")", "+", "(", "2", ")", ")", ";", ""
            ]
        );
    }

    #[test]
    fn directive_only_line_produces_no_runtime_tokens() {
        let input = r#"
#define X 5
"#;
        assert_eq!(kinds(input), vec![TokenKind::Eof]);
    }

    #[test]
    fn non_define_directive_is_ignored() {
        let input = r#"
#unknown stuff here
int a = 1;
"#;
        assert_eq!(lexemes(input), vec!["int", "a", "=", "1", ";", ""]);
    }

    #[test]
    fn define_with_leading_whitespace_at_line_start_works() {
        let input = "   #define X 7\nint y=X;\n";
        assert_eq!(lexemes(input), vec!["int", "y", "=", "7", ";", ""]);
    }

    #[test]
    fn no_nested_expansion_in_replacement() {
        let input = r#"
#define A B
#define B 9
int x = A;
"#;

        // As requested: no nested expansion. A -> B, stop there.
        assert_eq!(lexemes(input), vec!["int", "x", "=", "B", ";", ""]);
    }

    #[test]
    fn macro_can_expand_to_multiple_tokens() {
        let input = r#"
#define PAIR 1,2
int a[] = { PAIR };
"#;
        assert_eq!(
            lexemes(input),
            vec!["int", "a", "[", "]", "=", "{", "1", ",", "2", "}", ";", ""]
        );
    }

    #[test]
    fn string_and_char_lexing_still_work() {
        let input = r#"
char c = 'x';
char* s = "hi\n";
"#;
        assert_eq!(
            kinds(input),
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
        assert_eq!(
            kinds(input),
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
