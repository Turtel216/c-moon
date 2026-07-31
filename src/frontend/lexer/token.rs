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
}

impl fmt::Display for LexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LexError::UnexpectedChar => write!(f, "unexpected character"),
            LexError::UnterminatedString => write!(f, "unterminated string literal"),
            LexError::UnterminatedChar => write!(f, "unterminated character literal"),
        }
    }
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
