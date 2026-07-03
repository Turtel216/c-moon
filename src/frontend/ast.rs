use std::fmt;

use crate::frontend::lexer::Span;

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
    pub span: Span,
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
    Array(Box<CType>, Option<usize>),
    Struct(String),
}

impl fmt::Display for CType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CType::Int => write!(f, "int"),
            CType::Void => write!(f, "void"),
            CType::Pointer(inner) => write!(f, "{}*", inner),
            _ => todo!("Other types not supported yet"),
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
