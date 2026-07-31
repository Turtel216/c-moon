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
    /// The declaration as written. A function's span stops at its parameter
    /// list: underlining a whole definition would quote the entire body.
    pub span: Span,
    /// The declared identifier on its own, which is what a diagnostic about
    /// the *name* -- a redeclaration, say -- points at. An unnamed declaration
    /// repeats `span` here.
    pub name_span: Span,
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
        name: Option<String>,
        members: Vec<Decl>, // Simplified for illustration
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

/// Representation of C Types
#[derive(Debug, Clone, PartialEq)]
pub enum CType {
    Void,
    Int,
    Char,
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
            CType::Int => write!(f, "int"),
            CType::Char => write!(f, "char"),
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
    Int(i64),
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
        let pointer_to_int = CType::Pointer(Box::new(CType::Int));
        assert_eq!(pointer_to_int.to_string(), "int*");

        let array_of_pointers = CType::Array(Box::new(pointer_to_int), Some(10));
        assert_eq!(array_of_pointers.to_string(), "int*[10]");

        assert_eq!(
            CType::Array(Box::new(CType::Char), None).to_string(),
            "char[]"
        );
        assert_eq!(
            CType::Struct("Point".to_string()).to_string(),
            "struct Point"
        );
    }
}
