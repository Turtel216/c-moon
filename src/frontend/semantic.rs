use std::collections::HashMap;
use std::fmt;

use crate::driver::diagnostics::CompilerError;
use crate::frontend::ast::{
    BinaryOp, BlockItem, CType, Decl, DeclKind, Expr, ExprKind, Literal, ParamDecl, Stmt, StmtKind,
    UnaryOp,
};
use crate::frontend::lexer::Span;

/// Convenient semantic result alias.
pub type SemanticResult<T> = Result<T, SemanticError>;

/// A semantic error produced during name resolution / type checking.
#[derive(Debug, Clone, PartialEq)]
pub enum SemanticError {
    UndeclaredVariable {
        name: String,
        span: Span,
    },
    RedeclaredVariable {
        name: String,
        span: Span,
    },
    TypeError {
        expected: Type,
        found: Type,
        span: Span,
        context: &'static str,
    },
    InvalidAssignmentTarget {
        span: Span,
    },
    UnsupportedType {
        ty: CType,
        span: Span,
        context: &'static str,
    },
    UndeclaredFunction {
        name: String,
        span: Span,
    },
    RedeclaredFunction {
        name: String,
        span: Span,
    },
    ArgumentCountMismatch {
        name: String,
        expected: usize,
        found: usize,
        span: Span,
    },
}

impl CompilerError for SemanticError {
    fn get_span(&self) -> Span {
        match self {
            SemanticError::UndeclaredVariable { span, .. } => *span,
            SemanticError::RedeclaredVariable { span, .. } => *span,
            SemanticError::UndeclaredFunction { span, .. } => *span,
            SemanticError::RedeclaredFunction { span, .. } => *span,
            SemanticError::ArgumentCountMismatch { span, .. } => *span,
            SemanticError::TypeError { span, .. } => *span,
            SemanticError::InvalidAssignmentTarget { span, .. } => *span,
            SemanticError::UnsupportedType { span, .. } => *span,
        }
    }

    fn get_message(&self) -> String {
        match self {
            SemanticError::UndeclaredVariable { name, .. } => {
                format!("Undeclared variable '{}'", name)
            }
            SemanticError::RedeclaredVariable { name, .. } => {
                format!("Redeclared variable '{}'", name)
            }
            SemanticError::UndeclaredFunction { name, .. } => {
                format!("Undeclared function '{}'", name)
            }
            SemanticError::RedeclaredFunction { name, .. } => {
                format!("Redeclared function '{}'", name)
            }
            SemanticError::ArgumentCountMismatch {
                name,
                expected,
                found,
                ..
            } => format!(
                "Argument count mismatch for function '{}'. Expected {} got {}",
                name, expected, found
            ),
            SemanticError::TypeError {
                // TODO: Probably wrong. Handle context correctly
                expected,
                found,
                context,
                ..
            } => format!(
                "{}. Expected type '{}' got type '{}'",
                context, expected, found
            ),
            SemanticError::InvalidAssignmentTarget { .. } => format!("Invalid assignment"),
            SemanticError::UnsupportedType { ty, context, .. } => {
                // TODO: Probably wrong
                format!("Unsupported type. {} {}", context, ty)
            }
        }
    }

    fn error_prefix(&self) -> String {
        String::from("Type Error")
    }
}

impl fmt::Display for SemanticError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SemanticError::UndeclaredVariable { name, .. } => {
                write!(f, "use of undeclared variable `{name}`")
            }
            SemanticError::RedeclaredVariable { name, .. } => {
                write!(f, "redeclaration of variable `{name}` in the same scope")
            }
            SemanticError::UndeclaredFunction { name, .. } => {
                write!(f, "call to undeclared function `{name}`")
            }
            SemanticError::RedeclaredFunction { name, .. } => {
                write!(f, "redeclaration of function `{name}`")
            }
            SemanticError::ArgumentCountMismatch {
                name,
                expected,
                found,
                ..
            } => {
                write!(
                    f,
                    "wrong number of arguments in call to `{name}`: expected {expected}, found {found}"
                )
            }
            SemanticError::TypeError {
                expected,
                found,
                context,
                ..
            } => {
                write!(
                    f,
                    "type mismatch in {context}: expected `{expected}`, found `{found}`"
                )
            }
            SemanticError::InvalidAssignmentTarget { .. } => {
                write!(f, "left-hand side of assignment is not assignable")
            }
            SemanticError::UnsupportedType { ty, context, .. } => {
                write!(
                    f,
                    "unsupported type `{ty:?}` in {context} (only `int` is supported currently)"
                )
            }
        }
    }
}

impl std::error::Error for SemanticError {}

/// Internal semantic type model for the current compiler stage.
/// Keep this intentionally small while your language support is small.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Type {
    Int,
    Void,
    /// A fixed-size array of a primitive type, e.g. `int arr[3]` -> `Array(Int, 3)`.
    Array(Box<Type>, usize),
    /// A pointer to another type, e.g. `int *p` -> `Pointer(Int)`.
    Pointer(Box<Type>),
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Type::Int => write!(f, "int"),
            Type::Void => write!(f, "void"),
            Type::Array(elem, size) => write!(f, "{}[{}]", elem, size),
            Type::Pointer(inner) => write!(f, "{}*", inner),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionSig {
    pub return_ty: Type,
    pub param_tys: Vec<Type>,
}

fn ctype_to_type(ty: &CType, span: Span, context: &'static str) -> SemanticResult<Type> {
    match ty {
        CType::Int => Ok(Type::Int),
        CType::Void => Ok(Type::Void),
        CType::Array(elem_ctype, Some(size)) => {
            let elem_ty = ctype_to_type(elem_ctype, span, context)?;
            Ok(Type::Array(Box::new(elem_ty), *size))
        }
        CType::Pointer(inner_ctype) => {
            let inner_ty = ctype_to_type(inner_ctype, span, context)?;
            Ok(Type::Pointer(Box::new(inner_ty)))
        }
        _ => Err(SemanticError::UnsupportedType {
            ty: ty.clone(),
            span,
            context,
        }),
    }
}

/// Lexical-scope symbol table.
/// Each scope is a hash map from identifier -> semantic type.
#[derive(Debug, Default)]
pub struct SymbolTable {
    scopes: Vec<HashMap<String, Type>>,
}

impl SymbolTable {
    pub fn new() -> Self {
        Self { scopes: Vec::new() }
    }

    pub fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    pub fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    /// Define a symbol in the current scope.
    /// Fails if already declared in *this* scope.
    pub fn define(&mut self, name: String, ty: Type) -> bool {
        if let Some(scope) = self.scopes.last_mut() {
            if scope.contains_key(&name) {
                return false;
            }
            scope.insert(name, ty);
            true
        } else {
            // If no scope exists, create a global scope eagerly.
            let mut scope = HashMap::new();
            scope.insert(name, ty);
            self.scopes.push(scope);
            true
        }
    }

    /// Resolve a symbol, searching from innermost to outermost scope.
    pub fn resolve(&self, name: &str) -> Option<Type> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).cloned())
    }
}

/// Semantic analyzer for declarations/statements/expressions.
pub struct SemanticAnalyzer {
    symbols: SymbolTable,
    functions: HashMap<String, FunctionSig>,
    current_function_return: Option<Type>,
}

impl Default for SemanticAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

impl SemanticAnalyzer {
    pub fn new() -> Self {
        let mut symbols = SymbolTable::new();
        symbols.push_scope(); // global scope
        Self {
            symbols,
            functions: HashMap::new(),
            current_function_return: None,
        }
    }

    /// Analyze a translation unit (top-level declarations).
    pub fn analyze_program(&mut self, decls: &[Decl]) -> SemanticResult<()> {
        self.register_function_signatures(decls)?;
        for decl in decls {
            self.analyze_decl(decl)?;
        }
        Ok(())
    }

    fn register_function_signatures(&mut self, decls: &[Decl]) -> SemanticResult<()> {
        for decl in decls {
            if let DeclKind::Function {
                return_ty,
                name,
                params,
                ..
            } = &decl.kind
            {
                let ret_ty = ctype_to_type(return_ty, decl.span, "function return type")?;
                let mut param_tys = Vec::with_capacity(params.len());

                for ParamDecl { ty, .. } in params {
                    let pty = ctype_to_type(ty, decl.span, "function parameter")?;
                    param_tys.push(pty);
                }

                let sig = FunctionSig {
                    return_ty: ret_ty,
                    param_tys,
                };

                if self.functions.insert(name.clone(), sig).is_some() {
                    return Err(SemanticError::RedeclaredFunction {
                        name: name.clone(),
                        span: decl.span,
                    });
                }
            }
        }

        Ok(())
    }

    fn analyze_decl(&mut self, decl: &Decl) -> SemanticResult<()> {
        match &decl.kind {
            DeclKind::Variable {
                ty,
                name,
                initializer,
            } => {
                let var_ty = ctype_to_type(ty, decl.span, "variable declaration")?;
                if var_ty == Type::Void {
                    return Err(SemanticError::TypeError {
                        expected: Type::Int,
                        found: Type::Void,
                        span: decl.span,
                        context: "variable declaration",
                    });
                }

                if !self.symbols.define(name.clone(), var_ty.clone()) {
                    return Err(SemanticError::RedeclaredVariable {
                        name: name.clone(),
                        span: decl.span,
                    });
                }

                // Array declarations have no scalar initializer.
                if let Some(init) = initializer {
                    if matches!(var_ty, Type::Array(_, _)) {
                        return Err(SemanticError::UnsupportedType {
                            ty: ty.clone(),
                            span: decl.span,
                            context: "array initializer lists are not yet supported",
                        });
                    }
                    let init_ty = self.analyze_expr(init)?;
                    self.expect_type(&var_ty, &init_ty, init.span, "initializer")?;
                }

                Ok(())
            }

            DeclKind::Function {
                return_ty,
                name: _,
                params,
                body,
            } => {
                let ret_ty = ctype_to_type(return_ty, decl.span, "function return type")?;

                if let Some(body_stmt) = body {
                    self.symbols.push_scope();

                    for ParamDecl { ty, name, .. } in params {
                        let pty = ctype_to_type(ty, decl.span, "function parameter")?;
                        let pname = name.clone().unwrap_or_else(|| "_".to_string());
                        if !self.symbols.define(pname.clone(), pty) {
                            return Err(SemanticError::RedeclaredVariable {
                                name: pname,
                                span: decl.span,
                            });
                        }
                    }

                    let prev = self.current_function_return.clone();
                    self.current_function_return = Some(ret_ty);

                    self.analyze_stmt(body_stmt)?;

                    self.current_function_return = prev;
                    self.symbols.pop_scope();
                }

                Ok(())
            }

            DeclKind::Struct { .. } => {
                // Out of scope for now.
                Ok(())
            }
        }
    }

    fn analyze_stmt(&mut self, stmt: &Stmt) -> SemanticResult<()> {
        match &stmt.kind {
            StmtKind::Expr(expr) => {
                self.analyze_expr(expr)?;
                Ok(())
            }

            StmtKind::Return(opt_expr) => {
                let expected_ret = self.current_function_return.clone().unwrap_or(Type::Void);
                match opt_expr {
                    Some(expr) => {
                        let found = self.analyze_expr(expr)?;
                        self.expect_type(&expected_ret, &found, stmt.span, "return statement")
                    }
                    None => {
                        self.expect_type(&expected_ret, &Type::Void, stmt.span, "return statement")
                    }
                }
            }

            StmtKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                let cond_ty = self.analyze_expr(condition)?;
                self.expect_type(&Type::Int, &cond_ty, condition.span, "if condition")?;
                self.analyze_stmt(then_branch)?;
                if let Some(e) = else_branch {
                    self.analyze_stmt(e)?;
                }
                Ok(())
            }

            StmtKind::While { condition, body } => {
                let cond_ty = self.analyze_expr(condition)?;
                self.expect_type(&Type::Int, &cond_ty, condition.span, "while condition")?;
                self.analyze_stmt(body)
            }

            StmtKind::For {
                init,
                condition,
                step,
                body,
            } => {
                self.symbols.push_scope();

                if let Some(init_stmt) = init {
                    self.analyze_stmt(init_stmt)?;
                }
                if let Some(cond) = condition {
                    let cond_ty = self.analyze_expr(cond)?;
                    self.expect_type(&Type::Int, &cond_ty, cond.span, "for condition")?;
                }
                if let Some(step_expr) = step {
                    self.analyze_expr(step_expr)?;
                }
                self.analyze_stmt(body)?;

                self.symbols.pop_scope();
                Ok(())
            }

            StmtKind::Block(items) => {
                self.symbols.push_scope();
                for item in items {
                    match item {
                        BlockItem::Stmt(s) => self.analyze_stmt(s)?,
                        BlockItem::Decl(d) => self.analyze_decl(d)?,
                    }
                }
                self.symbols.pop_scope();
                Ok(())
            }
        }
    }

    fn analyze_expr(&mut self, expr: &Expr) -> SemanticResult<Type> {
        match &expr.kind {
            ExprKind::Literal(lit) => match lit {
                Literal::Int(_) => Ok(Type::Int),
                _ => Err(SemanticError::UnsupportedType {
                    ty: match lit {
                        Literal::Float(_) => CType::Float,
                        Literal::Char(_) => CType::Char,
                        Literal::String(_) => CType::Pointer(Box::new(CType::Char)),
                        Literal::Int(_) => unreachable!(),
                    },
                    span: expr.span,
                    context: "literal",
                }),
            },

            ExprKind::Identifier(name) => {
                self.symbols
                    .resolve(name)
                    .ok_or_else(|| SemanticError::UndeclaredVariable {
                        name: name.clone(),
                        span: expr.span,
                    })
            }

            ExprKind::Binary(op, lhs, rhs) => self.analyze_binary(*op, lhs, rhs, expr.span),

            ExprKind::Unary(op, inner) => self.analyze_unary(*op, inner, expr.span),

            ExprKind::Cast(to_ty, inner) => {
                let _from = self.analyze_expr(inner)?;
                ctype_to_type(to_ty, expr.span, "cast")
            }

            ExprKind::Call { callee, args } => self.analyze_call(callee, args, expr.span),

            ExprKind::Index { array, index } => {
                let base_ty = self.analyze_expr(array)?;
                // The base must be an array type; extract its element type.
                let elem_ty = match base_ty {
                    Type::Array(elem, _) => *elem,
                    _ => {
                        return Err(SemanticError::TypeError {
                            expected: Type::Array(Box::new(Type::Int), 0),
                            found: base_ty,
                            span: array.span,
                            context: "subscript operator requires an array type",
                        });
                    }
                };
                // The index must be an integer.
                let idx_ty = self.analyze_expr(index)?;
                self.expect_type(&Type::Int, &idx_ty, index.span, "array index")?;
                Ok(elem_ty)
            }

            // Out of current language scope: reject with clear unsupported diagnostics.
            ExprKind::MemberAccess { .. } | ExprKind::SizeOf(_) => {
                Err(SemanticError::UnsupportedType {
                    ty: CType::Int,
                    span: expr.span,
                    context: "expression form not yet supported by this semantic phase",
                })
            }
        }
    }

    fn analyze_call(&mut self, callee: &Expr, args: &[Expr], span: Span) -> SemanticResult<Type> {
        let fname = match &callee.kind {
            ExprKind::Identifier(name) => name,
            _ => {
                return Err(SemanticError::UnsupportedType {
                    ty: CType::Int,
                    span: callee.span,
                    context: "non-identifier callee not yet supported",
                });
            }
        };

        let sig = self.functions.get(fname).cloned().ok_or_else(|| {
            SemanticError::UndeclaredFunction {
                name: fname.clone(),
                span,
            }
        })?;

        if args.len() != sig.param_tys.len() {
            return Err(SemanticError::ArgumentCountMismatch {
                name: fname.clone(),
                expected: sig.param_tys.len(),
                found: args.len(),
                span,
            });
        }

        for (arg, expected) in args.iter().zip(sig.param_tys.iter()) {
            let found = self.analyze_expr(arg)?;
            self.expect_type(expected, &found, arg.span, "function argument")?;
        }

        Ok(sig.return_ty)
    }

    fn analyze_binary(
        &mut self,
        op: BinaryOp,
        lhs: &Expr,
        rhs: &Expr,
        span: Span,
    ) -> SemanticResult<Type> {
        if op == BinaryOp::Assign {
            if !Self::is_assignable(lhs) {
                return Err(SemanticError::InvalidAssignmentTarget { span: lhs.span });
            }

            let lty = self.analyze_expr(lhs)?;
            let rty = self.analyze_expr(rhs)?;
            self.expect_type(&lty, &rty, span, "assignment")?;
            return Ok(lty);
        }

        let lty = self.analyze_expr(lhs)?;
        let rty = self.analyze_expr(rhs)?;

        // For current scope: all arithmetic/comparison/logical/bitwise ops require int,int.
        self.expect_type(&Type::Int, &lty, lhs.span, "binary operation (lhs)")?;
        self.expect_type(&Type::Int, &rty, rhs.span, "binary operation (rhs)")?;

        // In C these return int (for relational/logical too).
        Ok(Type::Int)
    }

    fn analyze_unary(&mut self, op: UnaryOp, inner: &Expr, span: Span) -> SemanticResult<Type> {
        let ity = self.analyze_expr(inner)?;

        match op {
            UnaryOp::Neg
            | UnaryOp::Not
            | UnaryOp::BitNot
            | UnaryOp::PreInc
            | UnaryOp::PreDec
            | UnaryOp::PostInc
            | UnaryOp::PostDec => {
                self.expect_type(&Type::Int, &ity, span, "unary operation")?;
                Ok(Type::Int)
            }
            UnaryOp::AddressOf => {
                // The operand must be an lvalue (addressable).
                if !Self::is_addressable(inner) {
                    return Err(SemanticError::InvalidAssignmentTarget { span: inner.span });
                }
                Ok(Type::Pointer(Box::new(ity)))
            }
            UnaryOp::Deref => {
                // The operand must be a pointer type.
                match ity {
                    Type::Pointer(pointee) => Ok(*pointee),
                    _ => Err(SemanticError::TypeError {
                        expected: Type::Pointer(Box::new(Type::Int)),
                        found: ity,
                        span,
                        context: "dereference requires a pointer type",
                    }),
                }
            }
        }
    }

    /// Check whether an expression can have its address taken.
    fn is_addressable(expr: &Expr) -> bool {
        matches!(
            expr.kind,
            ExprKind::Identifier(_) | ExprKind::Index { .. } | ExprKind::Unary(UnaryOp::Deref, _)
        )
    }

    fn expect_type(
        &self,
        expected: &Type,
        found: &Type,
        span: Span,
        context: &'static str,
    ) -> SemanticResult<()> {
        if expected == found {
            return Ok(());
        }
        // Allow assigning integer literal 0 (NULL) to any pointer type.
        if matches!(expected, Type::Pointer(_)) && *found == Type::Int {
            return Ok(());
        }
        Err(SemanticError::TypeError {
            expected: expected.clone(),
            found: found.clone(),
            span,
            context,
        })
    }

    fn is_assignable(expr: &Expr) -> bool {
        matches!(
            expr.kind,
            ExprKind::Identifier(_)
                | ExprKind::Index { .. }
                | ExprKind::Unary(UnaryOp::Deref, _)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::ast::*;
    use crate::frontend::lexer::Span;

    fn dummy_span() -> Span {
        Span {
            line: 0,
            column: 0,
            length: 0,
        }
    }

    #[test]
    fn valid_integer_declaration_and_assignment() {
        let span = dummy_span();

        let decl_x = Decl {
            kind: DeclKind::Variable {
                ty: CType::Int,
                name: "x".to_string(),
                initializer: Some(Expr {
                    kind: ExprKind::Literal(Literal::Int(5)),
                    span,
                    id: 0,
                }),
            },
            span,
            id: 0,
        };

        let stmt_assign = Stmt {
            kind: StmtKind::Expr(Expr {
                kind: ExprKind::Binary(
                    BinaryOp::Assign,
                    Box::new(Expr {
                        kind: ExprKind::Identifier("x".to_string()),
                        span,
                        id: 0,
                    }),
                    Box::new(Expr {
                        kind: ExprKind::Binary(
                            BinaryOp::Add,
                            Box::new(Expr {
                                kind: ExprKind::Identifier("x".to_string()),
                                span,
                                id: 0,
                            }),
                            Box::new(Expr {
                                kind: ExprKind::Literal(Literal::Int(1)),
                                span,
                                id: 0,
                            }),
                        ),
                        span,
                        id: 0,
                    }),
                ),
                span,
                id: 0,
            }),
            span,
            id: 0,
        };

        let mut analyzer = SemanticAnalyzer::new();
        analyzer.analyze_decl(&decl_x).unwrap();
        analyzer.analyze_stmt(&stmt_assign).unwrap();
    }

    #[test]
    fn fails_on_undeclared_variable() {
        let span = dummy_span();

        let stmt = Stmt {
            kind: StmtKind::Expr(Expr {
                kind: ExprKind::Binary(
                    BinaryOp::Assign,
                    Box::new(Expr {
                        kind: ExprKind::Identifier("y".to_string()),
                        span,
                        id: 0,
                    }),
                    Box::new(Expr {
                        kind: ExprKind::Literal(Literal::Int(10)),
                        span,
                        id: 0,
                    }),
                ),
                span,
                id: 0,
            }),
            span,
            id: 0,
        };

        let mut analyzer = SemanticAnalyzer::new();
        let err = analyzer.analyze_stmt(&stmt).unwrap_err();

        match err {
            SemanticError::UndeclaredVariable { name, .. } => assert_eq!(name, "y"),
            other => panic!("expected UndeclaredVariable, got: {other:?}"),
        }
    }
}
