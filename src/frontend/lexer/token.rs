//! Token vocabulary produced by the scanner.

use std::borrow::Cow;
use std::fmt;

use crate::frontend::span::Span;

/// A lexical category. Every variant is a fixed shape, so the whole enum is
/// `Copy`: passing token kinds around never allocates or clones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

impl TokenKind {
    /// Maps an identifier-shaped lexeme to its keyword kind, if it is one.
    ///
    /// Written as a `match` rather than a lookup table on purpose: rustc
    /// compiles a match over string literals into a length-and-prefix decision
    /// tree, which beats hashing and never allocates.
    pub fn keyword(lexeme: &str) -> Option<Self> {
        Some(match lexeme {
            //"auto" => TokenKind::Auto,
            "break" => TokenKind::Break,
            "case" => TokenKind::Case,
            "char" => TokenKind::Char,
            "const" => TokenKind::Const,
            "continue" => TokenKind::Continue,
            "default" => TokenKind::Default,
            "do" => TokenKind::Do,
            "double" => TokenKind::Double,
            "else" => TokenKind::Else,
            "enum" => TokenKind::Enum,
            "extern" => TokenKind::Extern,
            "float" => TokenKind::Float,
            "for" => TokenKind::For,
            "goto" => TokenKind::Goto,
            "if" => TokenKind::If,
            "int" => TokenKind::Int,
            "long" => TokenKind::Long,
            //"register" => TokenKind::Register,
            "return" => TokenKind::Return,
            "short" => TokenKind::Short,
            "signed" => TokenKind::Signed,
            "sizeof" => TokenKind::Sizeof,
            "static" => TokenKind::Static,
            "struct" => TokenKind::Struct,
            "switch" => TokenKind::Switch,
            "typedef" => TokenKind::Typedef,
            "union" => TokenKind::Union,
            "unsigned" => TokenKind::Unsigned,
            "void" => TokenKind::Void,
            "volatile" => TokenKind::Volatile,
            "while" => TokenKind::While,
            _ => return None,
        })
    }
}

impl fmt::Display for TokenKind {
    /// Names the token the way a diagnostic should refer to it.
    ///
    /// Tokens with a fixed spelling are quoted -- `` `;` ``, `` `while` `` --
    /// while categories whose text varies are described in words, so that
    /// `format!("expected {kind}")` reads correctly either way.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            TokenKind::Break => "`break`",
            TokenKind::Case => "`case`",
            TokenKind::Char => "`char`",
            TokenKind::Const => "`const`",
            TokenKind::Continue => "`continue`",
            TokenKind::Default => "`default`",
            TokenKind::Do => "`do`",
            TokenKind::Double => "`double`",
            TokenKind::Else => "`else`",
            TokenKind::Enum => "`enum`",
            TokenKind::Extern => "`extern`",
            TokenKind::Float => "`float`",
            TokenKind::For => "`for`",
            TokenKind::Goto => "`goto`",
            TokenKind::If => "`if`",
            TokenKind::Int => "`int`",
            TokenKind::Long => "`long`",
            TokenKind::Return => "`return`",
            TokenKind::Short => "`short`",
            TokenKind::Signed => "`signed`",
            TokenKind::Sizeof => "`sizeof`",
            TokenKind::Static => "`static`",
            TokenKind::Struct => "`struct`",
            TokenKind::Switch => "`switch`",
            TokenKind::Typedef => "`typedef`",
            TokenKind::Union => "`union`",
            TokenKind::Unsigned => "`unsigned`",
            TokenKind::Void => "`void`",
            TokenKind::Volatile => "`volatile`",
            TokenKind::While => "`while`",

            TokenKind::Identifier => "identifier",
            TokenKind::IntegerLiteral => "integer literal",
            TokenKind::FloatLiteral => "floating-point literal",
            TokenKind::StringLiteral => "string literal",
            TokenKind::CharLiteral => "character literal",

            TokenKind::Plus => "`+`",
            TokenKind::Minus => "`-`",
            TokenKind::Star => "`*`",
            TokenKind::Slash => "`/`",
            TokenKind::Percent => "`%`",
            TokenKind::Eq => "`=`",
            TokenKind::EqEq => "`==`",
            TokenKind::Bang => "`!`",
            TokenKind::BangEq => "`!=`",
            TokenKind::Less => "`<`",
            TokenKind::LessEq => "`<=`",
            TokenKind::Greater => "`>`",
            TokenKind::GreaterEq => "`>=`",
            TokenKind::Ampersand => "`&`",
            TokenKind::AmpAmp => "`&&`",
            TokenKind::Pipe => "`|`",
            TokenKind::PipePipe => "`||`",
            TokenKind::Caret => "`^`",
            TokenKind::Tilde => "`~`",
            TokenKind::Shl => "`<<`",
            TokenKind::Shr => "`>>`",
            TokenKind::PlusPlus => "`++`",
            TokenKind::MinusMinus => "`--`",
            TokenKind::PlusEq => "`+=`",
            TokenKind::MinusEq => "`-=`",
            TokenKind::StarEq => "`*=`",
            TokenKind::SlashEq => "`/=`",
            TokenKind::PercentEq => "`%=`",
            TokenKind::AmpEq => "`&=`",
            TokenKind::PipeEq => "`|=`",
            TokenKind::CaretEq => "`^=`",
            TokenKind::ShlEq => "`<<=`",
            TokenKind::ShrEq => "`>>=`",

            TokenKind::LParen => "`(`",
            TokenKind::RParen => "`)`",
            TokenKind::LBrace => "`{`",
            TokenKind::RBrace => "`}`",
            TokenKind::LBracket => "`[`",
            TokenKind::RBracket => "`]`",
            TokenKind::Comma => "`,`",
            TokenKind::Semicolon => "`;`",
            TokenKind::Colon => "`:`",
            TokenKind::Dot => "`.`",
            TokenKind::Arrow => "`->`",
            TokenKind::Question => "`?`",

            TokenKind::Eof => "end of file",
            TokenKind::Error(error) => return write!(f, "{error}"),
        };

        f.write_str(name)
    }
}

/// Why the scanner could not turn a piece of input into a token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LexError {
    UnexpectedChar,
    UnterminatedString,
    UnterminatedChar,
    //UnterminatedBlockComment, TODO: Throw error in Scanner::skip_trivia()
    /// `''`, which names no character at all.
    EmptyChar,
    /// A character literal holding more than one character, e.g. `'ab'`.
    MultiCharacterChar,
    /// A backslash followed by something that spells no escape, e.g. `'\q'`.
    UnknownEscape,
    /// A numeric escape naming a value no `char` can hold, e.g. `'\400'`.
    EscapeOutOfRange,
}

impl fmt::Display for LexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LexError::UnexpectedChar => write!(f, "unexpected character"),
            LexError::UnterminatedString => write!(f, "unterminated string literal"),
            LexError::UnterminatedChar => write!(f, "unterminated character literal"),
            LexError::EmptyChar => write!(f, "empty character literal"),
            LexError::MultiCharacterChar => {
                write!(f, "character literal holding more than one character")
            }
            LexError::UnknownEscape => write!(f, "unknown escape sequence"),
            LexError::EscapeOutOfRange => {
                write!(f, "escape sequence out of range for a `char`")
            }
        }
    }
}

/// The escapes that stand for one character each, spelling paired with value.
///
/// `\0` is absent because it is not a case of its own: it is the octal escape
/// `\ooo` with a single digit, and is decoded as one.
const SIMPLE_ESCAPES: [(u8, u8); 10] = [
    (b'n', b'\n'),
    (b't', b'\t'),
    (b'r', b'\r'),
    (b'\\', b'\\'),
    (b'\'', b'\''),
    (b'"', b'"'),
    (b'a', 0x07),
    (b'b', 0x08),
    (b'f', 0x0c),
    (b'v', 0x0b),
];

/// The character a literal names, e.g. `65` for `'A'` and `10` for `'\n'`.
///
/// The scanner calls this to validate a literal it has just delimited and the
/// parser to read the value out of it, so the two can never disagree about
/// what a literal means.
///
/// # Arguments
///
/// * `lexeme` - the literal as written, quotes included
///
/// # Errors
///
/// * [`LexError::UnterminatedChar`] if `lexeme` is not quote-delimited
/// * [`LexError::EmptyChar`] for `''`
/// * [`LexError::MultiCharacterChar`] if anything follows the first character
/// * [`LexError::UnknownEscape`], [`LexError::EscapeOutOfRange`] from the
///   escape sequence
///
/// # Examples
///
/// ```text
/// char_literal_value("'A'")    == Ok(65)
/// char_literal_value("'\101'") == Ok(65)
/// char_literal_value("'\x41'") == Ok(65)
/// ```
pub fn char_literal_value(lexeme: &str) -> Result<u8, LexError> {
    let body = lexeme
        .strip_prefix('\'')
        .and_then(|rest| rest.strip_suffix('\''))
        .ok_or(LexError::UnterminatedChar)?
        .as_bytes();

    let (value, consumed) = match body.first() {
        None => return Err(LexError::EmptyChar),
        Some(b'\\') => decode_escape(body)?,
        // Source text is scanned byte-wise, so a multi-byte character is
        // several bytes here and is rejected below like any other pair.
        Some(&character) => (character, 1),
    };

    match consumed == body.len() {
        true => Ok(value),
        false => Err(LexError::MultiCharacterChar),
    }
}

/// Decodes the escape sequence `escape` starts with.
///
/// # Returns
///
/// The character the escape stands for, and how many bytes it spans counted
/// from its backslash.
fn decode_escape(escape: &[u8]) -> Result<(u8, usize), LexError> {
    match escape.get(1) {
        // `\ooo`: one to three octal digits, which is also what `\0` is.
        Some(b'0'..=b'7') => {
            let (value, digits) = decode_digits(&escape[1..], 8, 3)?;
            Ok((value, 1 + digits))
        }
        // `\xhh`: as many hexadecimal digits as follow it.
        Some(b'x') => {
            let (value, digits) = decode_digits(&escape[2..], 16, usize::MAX)?;
            Ok((value, 2 + digits))
        }
        Some(&introducer) => SIMPLE_ESCAPES
            .iter()
            .find(|&&(spelling, _)| spelling == introducer)
            .map(|&(_, value)| (value, 2))
            .ok_or(LexError::UnknownEscape),
        // A backslash with nothing after it, as in `'\'`.
        None => Err(LexError::UnknownEscape),
    }
}

/// Reads the value of at most `limit` leading digits of `radix`.
///
/// # Returns
///
/// The value the digits name and how many of them there were.
///
/// # Errors
///
/// [`LexError::UnknownEscape`] when no digit follows at all, as in `\x`, and
/// [`LexError::EscapeOutOfRange`] when the value does not fit in a `char`.
fn decode_digits(digits: &[u8], radix: u32, limit: usize) -> Result<(u8, usize), LexError> {
    let mut value: u32 = 0;
    let mut read = 0;

    for &byte in digits.iter().take(limit) {
        // `to_digit` is the digit test and the conversion in one; ASCII bytes
        // are their own characters, so the cast is exact.
        let Some(digit) = char::from(byte).to_digit(radix) else {
            break;
        };
        // Saturating rather than wrapping: a long run of digits is out of
        // range whatever it says, and must not wrap back into it.
        value = value.saturating_mul(radix).saturating_add(digit);
        read += 1;
    }

    if read == 0 {
        return Err(LexError::UnknownEscape);
    }
    let value = u8::try_from(value).map_err(|_| LexError::EscapeOutOfRange)?;
    Ok((value, read))
}

/// A token: its category, the source text it covers and where it came from.
///
/// The lexeme is a [`Cow`] because tokens scanned straight from the input can
/// borrow it, while tokens produced by macro expansion must own their text --
/// they outlive the replacement list they were copied from.
#[derive(Debug, Clone, PartialEq)]
pub struct Token<'a> {
    pub kind: TokenKind,
    pub lexeme: Cow<'a, str>,
    pub span: Span,
}

impl<'a> Token<'a> {
    /// Token whose lexeme borrows the source text.
    pub fn borrowed(kind: TokenKind, lexeme: &'a str, span: Span) -> Self {
        Self {
            kind,
            lexeme: Cow::Borrowed(lexeme),
            span,
        }
    }

    /// Token whose lexeme is owned, for text that is not a slice of the input.
    pub fn owned(kind: TokenKind, lexeme: impl Into<String>, span: Span) -> Self {
        Self {
            kind,
            lexeme: Cow::Owned(lexeme.into()),
            span,
        }
    }

    /// Names this token as a diagnostic should refer to it.
    ///
    /// A token that carries text is quoted as written -- `` `return` `` -- so
    /// the message shows what the reader typed; the end of input has no text to
    /// quote and is described instead. The result borrows the lexeme whenever
    /// no quoting is needed.
    pub fn describe(&self) -> Cow<'_, str> {
        match self.kind {
            TokenKind::Eof => Cow::Borrowed("end of file"),
            _ => Cow::Owned(format!("`{}`", self.lexeme)),
        }
    }

    /// Copies this token, detaching it from the text it borrows and re-anchoring
    /// it at `span`.
    ///
    /// Macro expansion replays one stored replacement list at many use sites:
    /// each copy owns its lexeme (hence the free lifetime `'b`) and reports the
    /// location of the macro *use*, so diagnostics point at the user's code
    /// rather than at the `#define`.
    pub fn to_owned_at<'b>(&self, span: Span) -> Token<'b> {
        Token {
            kind: self.kind,
            lexeme: Cow::Owned(self.lexeme.as_ref().to_owned()),
            span,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_literal_is_the_character_it_holds() {
        // Arrange / Act / Assert
        assert_eq!(char_literal_value("'A'"), Ok(65));
        assert_eq!(char_literal_value("' '"), Ok(32));
        assert_eq!(char_literal_value("'0'"), Ok(48));
    }

    #[test]
    fn every_simple_escape_names_its_control_character() {
        // Arrange / Act / Assert
        assert_eq!(char_literal_value(r"'\n'"), Ok(0x0a));
        assert_eq!(char_literal_value(r"'\t'"), Ok(0x09));
        assert_eq!(char_literal_value(r"'\r'"), Ok(0x0d));
        assert_eq!(char_literal_value(r"'\\'"), Ok(0x5c));
        assert_eq!(char_literal_value(r"'\''"), Ok(0x27));
        assert_eq!(char_literal_value(r#"'\"'"#), Ok(0x22));
        assert_eq!(char_literal_value(r"'\a'"), Ok(0x07));
        assert_eq!(char_literal_value(r"'\b'"), Ok(0x08));
        assert_eq!(char_literal_value(r"'\f'"), Ok(0x0c));
        assert_eq!(char_literal_value(r"'\v'"), Ok(0x0b));
    }

    #[test]
    fn a_numeric_escape_names_a_value_in_octal_or_hexadecimal() {
        // Arrange / Act / Assert: one to three octal digits ...
        assert_eq!(char_literal_value(r"'\0'"), Ok(0));
        assert_eq!(char_literal_value(r"'\7'"), Ok(7));
        assert_eq!(char_literal_value(r"'\41'"), Ok(33));
        assert_eq!(char_literal_value(r"'\101'"), Ok(65));
        // ... and however many hexadecimal ones follow `\x`.
        assert_eq!(char_literal_value(r"'\x41'"), Ok(65));
        assert_eq!(char_literal_value(r"'\x7'"), Ok(7));
        assert_eq!(char_literal_value(r"'\x0041'"), Ok(65));
    }

    #[test]
    fn an_octal_escape_stops_after_three_digits() {
        // Arrange / Act / Assert: `\1011` is `\101` followed by a `1`, which
        // makes it a two-character literal rather than a larger value.
        assert_eq!(
            char_literal_value(r"'\1011'"),
            Err(LexError::MultiCharacterChar)
        );
    }

    #[test]
    fn a_malformed_literal_says_what_is_wrong_with_it() {
        // Arrange / Act / Assert
        assert_eq!(char_literal_value("''"), Err(LexError::EmptyChar));
        assert_eq!(
            char_literal_value("'ab'"),
            Err(LexError::MultiCharacterChar)
        );
        assert_eq!(char_literal_value(r"'\q'"), Err(LexError::UnknownEscape));
        assert_eq!(char_literal_value(r"'\x'"), Err(LexError::UnknownEscape));
        assert_eq!(
            char_literal_value(r"'\400'"),
            Err(LexError::EscapeOutOfRange)
        );
        assert_eq!(
            char_literal_value(r"'\x100'"),
            Err(LexError::EscapeOutOfRange)
        );
        // A run of digits long enough to overflow the accumulator is still
        // out of range rather than wrapping back into it.
        assert_eq!(
            char_literal_value(r"'\xffffffffff41'"),
            Err(LexError::EscapeOutOfRange)
        );
    }

    #[test]
    fn a_multi_byte_character_does_not_fit_in_a_char() {
        // Arrange / Act / Assert: `é` is two bytes of UTF-8, so it is a
        // multi-character literal to a compiler that reads bytes.
        assert_eq!(char_literal_value("'é'"), Err(LexError::MultiCharacterChar));
    }
}
