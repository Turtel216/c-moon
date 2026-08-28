//! Abstract syntax tree produced by the parser.
//!
//! Every node carries a [`NodeId`] and a [`Span`]. The id gives later passes a
//! stable key for side tables -- the renamer's resolution map, for instance --
//! so the tree itself never has to be rewritten; the span is what diagnostics
//! point at.

use std::fmt;

use crate::frontend::span::Span;

/// Identifies one AST node within a translation unit.
///
/// Ids are handed out by the parser in construction order and are unique
/// across the whole tree, which lets passes record their results in a side
/// table keyed by id instead of mutating the AST.
pub type NodeId = u32;

#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    pub id: NodeId,
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    /// e.g., `42`, `3.14`, `"hello"`
    Literal(Literal),
    /// e.g., `x`, `my_var`
    Identifier(String),
    /// e.g., `a + b`
    Binary(BinaryOp, Box<Expr>, Box<Expr>),
    /// e.g., `-x`, `*ptr`, `&var`
    Unary(UnaryOp, Box<Expr>),
    /// e.g., `func(arg1, arg2)`
    Call { callee: Box<Expr>, args: Vec<Expr> },
    /// e.g., `arr[index]`
    Index { array: Box<Expr>, index: Box<Expr> },
    /// e.g., `struct_val.member` or `struct_ptr->member`
    MemberAccess {
        base: Box<Expr>,
        member: String,
        is_arrow: bool,
    },
    /// e.g., `(int)x`
    Cast(CType, Box<Expr>),
    /// e.g., `sizeof(int)` or `sizeof(x)`
    SizeOf(Box<Expr>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Stmt {
    pub id: NodeId,
    pub kind: StmtKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StmtKind {
    /// A lone semicolon, e.g. the body of `while (drain()) ;`
    Empty,
    /// A standalone expression followed by a semicolon, e.g., `x = 5;`
    Expr(Expr),
    /// e.g., `return x;`
    Return(Option<Expr>),
    /// e.g., `if (cond) { ... } else { ... }`
    If {
        condition: Expr,
        then_branch: Box<Stmt>,
        else_branch: Option<Box<Stmt>>,
    },
    /// e.g., `while (cond) { ... }`
    While { condition: Expr, body: Box<Stmt> },
    /// e.g., `for (init; cond; step) { ... }`
    For {
        init: Option<Box<Stmt>>,
        condition: Option<Expr>,
        step: Option<Expr>,
        body: Box<Stmt>,
    },
    /// e.g., `break;`
    Break,
    /// e.g., `continue;`
    Continue,
    /// e.g., `{ stmt1; stmt2; }`
    Block(Vec<BlockItem>),
}

/// Inside a C block, you can have both statements and declarations.
#[derive(Debug, Clone, PartialEq)]
pub enum BlockItem {
    Stmt(Stmt),
    Decl(Decl),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Decl {
    pub id: NodeId,
    pub kind: DeclKind,
    /// The storage class specifier written in front of the declaration.
    pub storage: StorageClass,
    /// The declaration as written. A function's span stops at its parameter
    /// list: underlining a whole definition would quote the entire body.
    pub span: Span,
    /// The declared identifier on its own, which is what a diagnostic about
    /// the *name* -- a redeclaration, say -- points at. An unnamed declaration
    /// repeats `span` here.
    pub name_span: Span,
}

/// The storage class specifier written in front of a declaration.
///
/// C spells out where a name is defined and how long its object lives; the
/// subset here has the two cases that differ for a function. The specifier
/// sits on the declaration rather than inside [`DeclKind`] because it is read
/// before the parser knows which kind it is reading -- `extern` comes first,
/// and only the declarator that follows says whether a variable or a function
/// is being declared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StorageClass {
    /// No specifier written. A function declared this way still has external
    /// linkage; what makes it different from `extern` is that a body may
    /// follow, and for a variable that storage is reserved here.
    #[default]
    None,
    /// `extern`: the name is defined elsewhere -- later in this translation
    /// unit, in another one, or in a library -- so nothing is reserved for it
    /// and the linker is left to find it.
    Extern,
}

impl StorageClass {
    /// The specifier as written, with the space that separates it from the
    /// type, or the empty string when none was written.
    ///
    /// Mirrors [`Sign::prefix`], and for the same reason: a diagnostic or a
    /// tree dump should quote the declaration the way the reader wrote it.
    pub const fn prefix(self) -> &'static str {
        match self {
            StorageClass::None => "",
            StorageClass::Extern => "extern ",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum DeclKind {
    /// e.g., `int x = 5;`
    Variable {
        ty: CType,
        name: String,
        initializer: Option<Expr>,
    },
    /// e.g., `int add(int a, int b) { ... }`
    Function {
        return_ty: CType,
        name: String,
        params: Vec<ParamDecl>,
        body: Option<Stmt>, // None if it's just a forward declaration / prototype
    },
    /// e.g., `struct Point { int x; int y; };`
    Struct {
        /// The tag, absent for an anonymous `struct { ... };`.
        name: Option<String>,
        /// The members as written, or `None` for the forward declaration
        /// `struct Point;`, which names a tag without saying what it holds.
        /// The distinction matters: only a definition gives the type a layout.
        members: Option<Vec<Decl>>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParamDecl {
    pub ty: CType,
    pub name: Option<String>,
    pub id: NodeId,
    /// The parameter as written, so a bad argument can be blamed on the
    /// parameter it was passed for.
    pub span: Span,
}

/// Whether an integer type's top bit is a sign or just another digit.
///
/// Every integer type comes in both forms, so it is a property of the type
/// rather than a type of its own: `unsigned char` and `char` are the same
/// eight bits read two different ways. Which way decides how a value widens,
/// how two of them compare, and how one divides another -- and nothing else,
/// which is why the same addition serves both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Sign {
    Signed,
    Unsigned,
}

impl Sign {
    /// The specifier written in front of a type of this signedness.
    ///
    /// Empty for a signed type: C spells `int` rather than `signed int`, and
    /// a diagnostic should quote the type the way the reader would write it.
    pub const fn prefix(self) -> &'static str {
        match self {
            Sign::Signed => "",
            Sign::Unsigned => "unsigned ",
        }
    }
}

/// Representation of C Types
#[derive(Debug, Clone, PartialEq)]
pub enum CType {
    Void,
    /// `char`, an 8-bit integer.
    Char(Sign),
    /// `int`, a 32-bit integer.
    Int(Sign),
    /// `long int`, a 64-bit integer. `long` on its own means the same.
    Long(Sign),
    Float,
    Double,
    Pointer(Box<CType>),
    /// A fixed-size array; the size is `None` for `int a[]`.
    Array(Box<CType>, Option<usize>),
    Struct(String),
}

impl fmt::Display for CType {
    /// Writes the type in C declaration syntax, e.g. `int*` or `int[10]`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CType::Void => write!(f, "void"),
            CType::Char(sign) => write!(f, "{}char", sign.prefix()),
            CType::Int(sign) => write!(f, "{}int", sign.prefix()),
            CType::Long(sign) => write!(f, "{}long int", sign.prefix()),
            CType::Float => write!(f, "float"),
            CType::Double => write!(f, "double"),
            CType::Pointer(inner) => write!(f, "{inner}*"),
            CType::Array(elem, Some(size)) => write!(f, "{elem}[{size}]"),
            CType::Array(elem, None) => write!(f, "{elem}[]"),
            CType::Struct(name) => write!(f, "struct {name}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    /// The magnitude of an integer constant, e.g. the `42` of `42`.
    ///
    /// Unsigned because a constant has no sign of its own: C has no negative
    /// literal, only unary minus applied to a positive one. Holding the
    /// magnitude is also what lets a constant too large for a `long int` --
    /// and so an `unsigned long int` -- be written at all.
    Int(u64),
    Float(f64),
    Char(u8),
    String(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    Neq,
    Lt,
    Lte,
    Gt,
    Gte,
    LogicalAnd,
    LogicalOr,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    Assign,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,       // -x
    Not,       // !x
    BitNot,    // ~x
    Deref,     // *x
    AddressOf, // &x
    PreInc,    // ++x
    PreDec,    // --x
    PostInc,   // x++
    PostDec,   // x--
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn displays_derived_types_in_c_syntax() {
        let pointer_to_int = CType::Pointer(Box::new(CType::Int(Sign::Signed)));
        assert_eq!(pointer_to_int.to_string(), "int*");

        let array_of_pointers = CType::Array(Box::new(pointer_to_int), Some(10));
        assert_eq!(array_of_pointers.to_string(), "int*[10]");

        assert_eq!(
            CType::Array(Box::new(CType::Char(Sign::Signed)), None).to_string(),
            "char[]"
        );
        assert_eq!(
            CType::Struct("Point".to_string()).to_string(),
            "struct Point"
        );
        assert_eq!(
            CType::Pointer(Box::new(CType::Long(Sign::Signed))).to_string(),
            "long int*"
        );
    }

    #[test]
    fn a_storage_class_writes_the_specifier_that_names_it() {
        // Arrange / Act / Assert: a declaration with nothing written in front
        // of it prints exactly as it did before there was a storage class.
        assert_eq!(StorageClass::Extern.prefix(), "extern ");
        assert_eq!(StorageClass::None.prefix(), "");
        assert_eq!(StorageClass::default(), StorageClass::None);
    }

    #[test]
    fn displays_an_unsigned_type_with_the_specifier_that_names_it() {
        // Arrange / Act / Assert: the signed types are written without a
        // specifier, the way C spells them.
        assert_eq!(CType::Char(Sign::Unsigned).to_string(), "unsigned char");
        assert_eq!(CType::Int(Sign::Unsigned).to_string(), "unsigned int");
        assert_eq!(CType::Long(Sign::Unsigned).to_string(), "unsigned long int");
        assert_eq!(CType::Int(Sign::Signed).to_string(), "int");
    }
}
