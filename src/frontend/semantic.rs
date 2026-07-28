//! Semantic analysis: scope checking and type checking over the AST.
//!
//! The analyzer walks the translation unit once, keeping a [`ScopeStack`] of
//! variable types and a table of function signatures. Errors are collected per
//! top-level declaration rather than aborting the pass, so one run reports the
//! problems in several functions.

use std::collections::HashMap;
use std::fmt;

use crate::driver::diagnostics::CompilerError;
use crate::frontend::ast::{
    BinaryOp, BlockItem, CType, Decl, DeclKind, Expr, ExprKind, Literal, ParamDecl, Stmt, StmtKind,
    UnaryOp,
};
use crate::frontend::scope::ScopeStack;
use crate::frontend::span::Span;

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

impl fmt::Display for SemanticError {
    /// Writes the message the compiler prints for this error.
    ///
    /// This is the single source of the wording; [`CompilerError::get_message`]
    /// defers to it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SemanticError::UndeclaredVariable { name, .. } => {
                write!(f, "Undeclared variable '{name}'")
            }
            SemanticError::RedeclaredVariable { name, .. } => {
                write!(f, "Redeclared variable '{name}'")
            }
            SemanticError::UndeclaredFunction { name, .. } => {
                write!(f, "Undeclared function '{name}'")
            }
            SemanticError::RedeclaredFunction { name, .. } => {
                write!(f, "Redeclared function '{name}'")
            }
            SemanticError::ArgumentCountMismatch {
                name,
                expected,
                found,
                ..
            } => write!(
                f,
                "Argument count mismatch for function '{name}'. Expected {expected} got {found}"
            ),
            // TODO: `context` reads as a sentence fragment in these two; give
            // the contexts a consistent phrasing.
            SemanticError::TypeError {
                expected,
                found,
                context,
                ..
            } => write!(
                f,
                "{context}. Expected type '{expected}' got type '{found}'"
            ),
            SemanticError::InvalidAssignmentTarget { .. } => write!(f, "Invalid assignment"),
            SemanticError::UnsupportedType { ty, context, .. } => {
                write!(f, "Unsupported type. {context} {ty}")
            }
        }
    }
}

impl std::error::Error for SemanticError {}

impl CompilerError for SemanticError {
    fn get_span(&self) -> Span {
        // Every variant carries a `span` field, so one or-pattern binds it.
        match self {
            SemanticError::UndeclaredVariable { span, .. }
            | SemanticError::RedeclaredVariable { span, .. }
            | SemanticError::UndeclaredFunction { span, .. }
            | SemanticError::RedeclaredFunction { span, .. }
            | SemanticError::ArgumentCountMismatch { span, .. }
            | SemanticError::TypeError { span, .. }
            | SemanticError::InvalidAssignmentTarget { span }
            | SemanticError::UnsupportedType { span, .. } => *span,
        }
    }

    fn get_message(&self) -> String {
        self.to_string()
    }

    fn error_prefix(&self) -> String {
        String::from("Type Error")
    }
}

/// Internal semantic type model for the current compiler stage.
///
/// This is deliberately smaller than [`CType`]: it only models what the rest of
/// the pipeline can compile, so unsupported syntax is rejected here rather than
/// crashing a later phase.
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
            Type::Array(elem, size) => write!(f, "{elem}[{size}]"),
            Type::Pointer(inner) => write!(f, "{inner}*"),
        }
    }
}

impl Type {
    /// Translates a syntactic type into the semantic model.
    ///
    /// # Arguments
    ///
    /// * `ty` - the type as written in the source
    /// * `span` - location to blame if the type is not supported
    /// * `context` - what was being declared, for the error message
    ///
    /// # Errors
    ///
    /// Returns [`SemanticError::UnsupportedType`] for types the compiler cannot
    /// lower yet: `char`, `float`, `double`, structs, and arrays of unknown
    /// size.
    fn from_ctype(ty: &CType, span: Span, context: &'static str) -> SemanticResult<Self> {
        match ty {
            CType::Int => Ok(Type::Int),
            CType::Void => Ok(Type::Void),
            CType::Array(elem, Some(size)) => Ok(Type::Array(
                Box::new(Type::from_ctype(elem, span, context)?),
                *size,
            )),
            CType::Pointer(inner) => Ok(Type::Pointer(Box::new(Type::from_ctype(
                inner, span, context,
            )?))),
            CType::Char
            | CType::Float
            | CType::Double
            | CType::Struct(_)
            | CType::Array(_, None) => Err(SemanticError::UnsupportedType {
                ty: ty.clone(),
                span,
                context,
            }),
        }
    }
}

/// A function's declared interface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionSig {
    pub return_ty: Type,
    pub param_tys: Vec<Type>,
}

/// Semantic analyzer for declarations/statements/expressions.
#[derive(Debug, Default)]
pub struct SemanticAnalyzer {
    /// Variable types, innermost scope last.
    symbols: ScopeStack<Type>,
    /// Signatures of every function in the translation unit, collected before
    /// bodies are checked so functions may be called before they are defined.
    functions: HashMap<String, FunctionSig>,
    /// Return type of the function whose body is being checked.
    current_function_return: Option<Type>,
    errors: Vec<SemanticError>,
}

impl SemanticAnalyzer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Analyze a translation unit (top-level declarations).
    ///
    /// # Returns
    ///
    /// Every error found. Analysis of one declaration stops at its first error
    /// but the remaining declarations are still checked.
    pub fn analyze_program(&mut self, decls: &[Decl]) -> Vec<SemanticError> {
        if let Err(error) = self.register_function_signatures(decls) {
            self.errors.push(error);
        }

        for decl in decls {
            if let Err(error) = self.analyze_decl(decl) {
                self.errors.push(error);
            }
        }

        std::mem::take(&mut self.errors)
    }

    /// Records every function's signature before any body is checked.
    fn register_function_signatures(&mut self, decls: &[Decl]) -> SemanticResult<()> {
        for decl in decls {
            let DeclKind::Function {
                return_ty,
                name,
                params,
                ..
            } = &decl.kind
            else {
                continue;
            };

            let sig = FunctionSig {
                return_ty: Type::from_ctype(return_ty, decl.span, "function return type")?,
                param_tys: Self::param_types(params, decl.span)?,
            };

            if self.functions.insert(name.clone(), sig).is_some() {
                return Err(SemanticError::RedeclaredFunction {
                    name: name.clone(),
                    span: decl.span,
                });
            }
        }

        Ok(())
    }

    /// Translates a parameter list into semantic types.
    fn param_types(params: &[ParamDecl], span: Span) -> SemanticResult<Vec<Type>> {
        params
            .iter()
            .map(|param| Type::from_ctype(&param.ty, span, "function parameter"))
            .collect()
    }

    fn analyze_decl(&mut self, decl: &Decl) -> SemanticResult<()> {
        match &decl.kind {
            DeclKind::Variable {
                ty,
                name,
                initializer,
            } => self.analyze_variable_decl(decl, ty, name, initializer.as_ref()),

            DeclKind::Function {
                return_ty,
                params,
                body,
                ..
            } => self.analyze_function_decl(decl, return_ty, params, body.as_ref()),

            // Struct definitions declare no runtime storage, and struct types
            // are rejected by `Type::from_ctype` wherever they are used.
            DeclKind::Struct { .. } => Ok(()),
        }
    }

    fn analyze_variable_decl(
        &mut self,
        decl: &Decl,
        ty: &CType,
        name: &str,
        initializer: Option<&Expr>,
    ) -> SemanticResult<()> {
        let var_ty = Type::from_ctype(ty, decl.span, "variable declaration")?;
        if var_ty == Type::Void {
            return Err(SemanticError::TypeError {
                expected: Type::Int,
                found: Type::Void,
                span: decl.span,
                context: "variable declaration",
            });
        }

        if !self.symbols.declare(name, var_ty.clone()) {
            return Err(SemanticError::RedeclaredVariable {
                name: name.to_owned(),
                span: decl.span,
            });
        }

        let Some(init) = initializer else {
            return Ok(());
        };

        // Array declarations have no scalar initializer.
        if matches!(var_ty, Type::Array(_, _)) {
            return Err(SemanticError::UnsupportedType {
                ty: ty.clone(),
                span: decl.span,
                context: "array initializer lists are not yet supported",
            });
        }

        let init_ty = self.analyze_expr(init)?;
        Self::expect_type(&var_ty, &init_ty, init.span, "initializer")
    }

    fn analyze_function_decl(
        &mut self,
        decl: &Decl,
        return_ty: &CType,
        params: &[ParamDecl],
        body: Option<&Stmt>,
    ) -> SemanticResult<()> {
        let ret_ty = Type::from_ctype(return_ty, decl.span, "function return type")?;

        // A prototype declares no scope of its own; only a definition is checked.
        let Some(body) = body else {
            return Ok(());
        };

        self.symbols.push_scope();

        for param in params {
            let param_ty = Type::from_ctype(&param.ty, decl.span, "function parameter")?;
            // An unnamed parameter still occupies a slot; `_` cannot collide
            // with a real identifier because it would be a valid C name --
            // a name clash here is reported like any other redeclaration.
            let param_name = param.name.as_deref().unwrap_or("_");
            if !self.symbols.declare(param_name, param_ty) {
                return Err(SemanticError::RedeclaredVariable {
                    name: param_name.to_owned(),
                    span: decl.span,
                });
            }
        }

        let enclosing = self.current_function_return.replace(ret_ty);
        let result = self.analyze_stmt(body);
        self.current_function_return = enclosing;
        self.symbols.pop_scope();

        result
    }

    fn analyze_stmt(&mut self, stmt: &Stmt) -> SemanticResult<()> {
        match &stmt.kind {
            StmtKind::Expr(expr) => self.analyze_expr(expr).map(|_| ()),

            StmtKind::Return(value) => {
                let expected = self.current_function_return.clone().unwrap_or(Type::Void);
                let found = match value {
                    Some(expr) => self.analyze_expr(expr)?,
                    None => Type::Void,
                };
                Self::expect_type(&expected, &found, stmt.span, "return statement")
            }

            StmtKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                self.analyze_condition(condition, "if condition")?;
                self.analyze_stmt(then_branch)?;
                match else_branch {
                    Some(branch) => self.analyze_stmt(branch),
                    None => Ok(()),
                }
            }

            StmtKind::While { condition, body } => {
                self.analyze_condition(condition, "while condition")?;
                self.analyze_stmt(body)
            }

            StmtKind::For {
                init,
                condition,
                step,
                body,
            } => {
                // The init clause may declare a variable, which is scoped to
                // the loop.
                self.in_new_scope(|analyzer| {
                    if let Some(init) = init {
                        analyzer.analyze_stmt(init)?;
                    }
                    if let Some(condition) = condition {
                        analyzer.analyze_condition(condition, "for condition")?;
                    }
                    if let Some(step) = step {
                        analyzer.analyze_expr(step)?;
                    }
                    analyzer.analyze_stmt(body)
                })
            }

            StmtKind::Block(items) => self.in_new_scope(|analyzer| {
                for item in items {
                    match item {
                        BlockItem::Stmt(stmt) => analyzer.analyze_stmt(stmt)?,
                        BlockItem::Decl(decl) => analyzer.analyze_decl(decl)?,
                    }
                }
                Ok(())
            }),
        }
    }

    /// Runs `analysis` in a nested scope, closing it even when the analysis fails.
    fn in_new_scope<T>(
        &mut self,
        analysis: impl FnOnce(&mut Self) -> SemanticResult<T>,
    ) -> SemanticResult<T> {
        self.symbols.push_scope();
        let result = analysis(self);
        self.symbols.pop_scope();
        result
    }

    /// Checks a controlling expression, which C requires to be a scalar.
    fn analyze_condition(&mut self, condition: &Expr, context: &'static str) -> SemanticResult<()> {
        let cond_ty = self.analyze_expr(condition)?;
        Self::expect_type(&Type::Int, &cond_ty, condition.span, context)
    }

    fn analyze_expr(&mut self, expr: &Expr) -> SemanticResult<Type> {
        match &expr.kind {
            ExprKind::Literal(literal) => Self::literal_type(literal, expr.span),

            ExprKind::Identifier(name) => self.symbols.lookup(name).cloned().ok_or_else(|| {
                SemanticError::UndeclaredVariable {
                    name: name.clone(),
                    span: expr.span,
                }
            }),

            ExprKind::Binary(op, lhs, rhs) => self.analyze_binary(*op, lhs, rhs, expr.span),

            ExprKind::Unary(op, operand) => self.analyze_unary(*op, operand, expr.span),

            ExprKind::Cast(to_ty, operand) => {
                self.analyze_expr(operand)?;
                Type::from_ctype(to_ty, expr.span, "cast")
            }

            ExprKind::Call { callee, args } => self.analyze_call(callee, args, expr.span),

            ExprKind::Index { array, index } => self.analyze_index(array, index),

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

    /// The type of a literal. Only integer literals are supported so far.
    fn literal_type(literal: &Literal, span: Span) -> SemanticResult<Type> {
        match literal {
            Literal::Int(_) => Ok(Type::Int),
            Literal::Float(_) => Err(SemanticError::unsupported_literal(CType::Float, span)),
            Literal::Char(_) => Err(SemanticError::unsupported_literal(CType::Char, span)),
            Literal::String(_) => Err(SemanticError::unsupported_literal(
                CType::Pointer(Box::new(CType::Char)),
                span,
            )),
        }
    }

    /// Checks `array[index]`, which requires an array and an integer index.
    fn analyze_index(&mut self, array: &Expr, index: &Expr) -> SemanticResult<Type> {
        let base_ty = self.analyze_expr(array)?;
        let Type::Array(elem_ty, _) = base_ty else {
            return Err(SemanticError::TypeError {
                expected: Type::Array(Box::new(Type::Int), 0),
                found: base_ty,
                span: array.span,
                context: "subscript operator requires an array type",
            });
        };

        let index_ty = self.analyze_expr(index)?;
        Self::expect_type(&Type::Int, &index_ty, index.span, "array index")?;
        Ok(*elem_ty)
    }

    fn analyze_call(&mut self, callee: &Expr, args: &[Expr], span: Span) -> SemanticResult<Type> {
        let ExprKind::Identifier(name) = &callee.kind else {
            // Calls through function pointers are not supported yet.
            return Err(SemanticError::UnsupportedType {
                ty: CType::Int,
                span: callee.span,
                context: "non-identifier callee not yet supported",
            });
        };

        let sig =
            self.functions
                .get(name)
                .cloned()
                .ok_or_else(|| SemanticError::UndeclaredFunction {
                    name: name.clone(),
                    span,
                })?;

        if args.len() != sig.param_tys.len() {
            return Err(SemanticError::ArgumentCountMismatch {
                name: name.clone(),
                expected: sig.param_tys.len(),
                found: args.len(),
                span,
            });
        }

        for (arg, expected) in args.iter().zip(&sig.param_tys) {
            let found = self.analyze_expr(arg)?;
            Self::expect_type(expected, &found, arg.span, "function argument")?;
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
            if !Self::is_lvalue(lhs) {
                return Err(SemanticError::InvalidAssignmentTarget { span: lhs.span });
            }

            let target_ty = self.analyze_expr(lhs)?;
            let value_ty = self.analyze_expr(rhs)?;
            Self::expect_type(&target_ty, &value_ty, span, "assignment")?;
            return Ok(target_ty);
        }

        // For the current subset every arithmetic, comparison, logical and
        // bitwise operator takes two ints...
        let lhs_ty = self.analyze_expr(lhs)?;
        let rhs_ty = self.analyze_expr(rhs)?;
        Self::expect_type(&Type::Int, &lhs_ty, lhs.span, "binary operation (lhs)")?;
        Self::expect_type(&Type::Int, &rhs_ty, rhs.span, "binary operation (rhs)")?;

        // ... and yields an int, comparisons and logical operators included.
        Ok(Type::Int)
    }

    fn analyze_unary(&mut self, op: UnaryOp, operand: &Expr, span: Span) -> SemanticResult<Type> {
        let operand_ty = self.analyze_expr(operand)?;

        match op {
            UnaryOp::Neg
            | UnaryOp::Not
            | UnaryOp::BitNot
            | UnaryOp::PreInc
            | UnaryOp::PreDec
            | UnaryOp::PostInc
            | UnaryOp::PostDec => {
                Self::expect_type(&Type::Int, &operand_ty, span, "unary operation")?;
                Ok(Type::Int)
            }
            UnaryOp::AddressOf => {
                if !Self::is_lvalue(operand) {
                    return Err(SemanticError::InvalidAssignmentTarget { span: operand.span });
                }
                Ok(Type::Pointer(Box::new(operand_ty)))
            }
            UnaryOp::Deref => match operand_ty {
                Type::Pointer(pointee) => Ok(*pointee),
                _ => Err(SemanticError::TypeError {
                    expected: Type::Pointer(Box::new(Type::Int)),
                    found: operand_ty,
                    span,
                    context: "dereference requires a pointer type",
                }),
            },
        }
    }

    /// Whether an expression denotes an object: something that can be assigned
    /// to and whose address can be taken.
    fn is_lvalue(expr: &Expr) -> bool {
        matches!(
            expr.kind,
            ExprKind::Identifier(_) | ExprKind::Index { .. } | ExprKind::Unary(UnaryOp::Deref, _)
        )
    }

    /// Checks that `found` is acceptable where `expected` is required.
    ///
    /// # Errors
    ///
    /// Returns [`SemanticError::TypeError`] blaming `span`, described by
    /// `context`.
    fn expect_type(
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
}

impl SemanticError {
    /// Error for a literal whose type the compiler cannot represent yet.
    fn unsupported_literal(ty: CType, span: Span) -> Self {
        SemanticError::UnsupportedType {
            ty,
            span,
            context: "literal",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::lexer::Lexer;
    use crate::frontend::parser::Parser;

    /// Analyzes `src` and returns the errors reported.
    fn analyze(src: &str) -> Vec<SemanticError> {
        let mut parser = Parser::from_lexer(Lexer::new(src)).expect("lexing should succeed");
        let (decls, parse_errors) = parser.parse_translation_unit();
        assert!(
            parse_errors.is_empty(),
            "unexpected parse errors: {parse_errors:?}"
        );

        SemanticAnalyzer::new().analyze_program(&decls)
    }

    /// Analyzes `src`, asserting that it is accepted.
    fn analyze_ok(src: &str) {
        let errors = analyze(src);
        assert!(errors.is_empty(), "expected no errors, got {errors:?}");
    }

    /// Analyzes `src`, asserting that exactly one error is reported.
    fn analyze_err(src: &str) -> SemanticError {
        let mut errors = analyze(src);
        assert_eq!(
            errors.len(),
            1,
            "expected exactly one error, got {errors:?}"
        );
        errors.remove(0)
    }

    #[test]
    fn valid_integer_declaration_and_assignment() {
        analyze_ok("int main() { int x = 5; x = x + 1; return x; }");
    }

    #[test]
    fn fails_on_undeclared_variable() {
        match analyze_err("int main() { y = 10; return 0; }") {
            SemanticError::UndeclaredVariable { name, .. } => assert_eq!(name, "y"),
            other => panic!("expected UndeclaredVariable, got: {other:?}"),
        }
    }

    #[test]
    fn fails_on_redeclaration_in_same_scope() {
        match analyze_err("int main() { int x; int x; return 0; }") {
            SemanticError::RedeclaredVariable { name, .. } => assert_eq!(name, "x"),
            other => panic!("expected RedeclaredVariable, got: {other:?}"),
        }
    }

    #[test]
    fn allows_shadowing_in_a_nested_block() {
        analyze_ok("int main() { int x = 1; { int x = 2; x = x + 1; } return x; }");
    }

    #[test]
    fn fails_on_argument_count_mismatch() {
        match analyze_err("int add(int a, int b) { return a + b; } int main() { return add(1); }") {
            SemanticError::ArgumentCountMismatch {
                name,
                expected,
                found,
                ..
            } => {
                assert_eq!(name, "add");
                assert_eq!((expected, found), (2, 1));
            }
            other => panic!("expected ArgumentCountMismatch, got: {other:?}"),
        }
    }

    #[test]
    fn fails_on_assignment_to_non_lvalue() {
        let error = analyze_err("int main() { 1 = 2; return 0; }");
        assert!(matches!(
            error,
            SemanticError::InvalidAssignmentTarget { .. }
        ));
    }

    #[test]
    fn checks_pointer_dereference_and_address_of() {
        analyze_ok("int main() { int x = 1; int *p = &x; *p = 2; return *p; }");
    }

    #[test]
    fn rejects_dereference_of_non_pointer() {
        let error = analyze_err("int main() { int x = 1; return *x; }");
        assert!(matches!(error, SemanticError::TypeError { .. }));
    }

    #[test]
    fn rejects_unsupported_types() {
        let error = analyze_err("int main() { float f; return 0; }");
        assert!(matches!(error, SemanticError::UnsupportedType { .. }));
    }

    #[test]
    fn reports_errors_from_several_functions() {
        let errors = analyze("int f() { return a; } int g() { return b; }");
        assert_eq!(errors.len(), 2, "got {errors:?}");
    }

    #[test]
    fn error_message_matches_diagnostic_output() {
        let error = analyze_err("int main() { return missing(); }");
        assert_eq!(error.get_message(), error.to_string());
        assert_eq!(error.get_message(), "Undeclared function 'missing'");
    }
}
