//! Semantic analysis: scope checking and type checking over the AST.
//!
//! The analyzer walks the translation unit once, keeping a [`ScopeStack`] of
//! variable types and a table of function signatures. Errors are collected per
//! top-level declaration rather than aborting the pass, so one run reports the
//! problems in several functions.

use std::collections::HashMap;
use std::fmt;

use crate::driver::diagnostics::{CompilerError, Diagnostic, codes};
use crate::frontend::ast::{
    BinaryOp, BlockItem, CType, Decl, DeclKind, Expr, ExprKind, Literal, ParamDecl, Stmt, StmtKind,
    UnaryOp,
};
use crate::frontend::scope::ScopeStack;
use crate::frontend::span::Span;
use crate::frontend::suggest;

/// Convenient semantic result alias.
pub type SemanticResult<T> = Result<T, SemanticError>;

/// A semantic error produced during name resolution / type checking.
///
/// Every variant carries the spans a diagnostic needs: the one to blame, and
/// where relevant the earlier declaration that explains why it is wrong.
#[derive(Debug, Clone, PartialEq)]
pub enum SemanticError {
    UndeclaredVariable {
        name: String,
        span: Span,
        /// A name in scope that the unknown one is probably a typo of.
        suggestion: Option<String>,
    },
    RedeclaredVariable {
        name: String,
        span: Span,
        /// Where the name was declared the first time.
        previous: Span,
    },
    UndeclaredFunction {
        name: String,
        span: Span,
        suggestion: Option<String>,
    },
    RedeclaredFunction {
        name: String,
        span: Span,
        previous: Span,
    },
    ArgumentCountMismatch {
        name: String,
        expected: usize,
        found: usize,
        span: Span,
        /// The function's signature, which fixes the expected count.
        declared: Span,
    },
    TypeError {
        expected: Type,
        found: Type,
        span: Span,
        /// What imposed the expected type -- the declaration an initializer
        /// must match, say. `None` when the language itself does.
        origin: Option<Span>,
        /// Why that type is required here, read either as a caption for
        /// `origin` or, without one, as a closing note.
        context: &'static str,
    },
    NotIndexable {
        found: Type,
        span: Span,
    },
    NotDereferenceable {
        found: Type,
        span: Span,
    },
    NotAnObject {
        span: Span,
        /// What was attempted, e.g. `assign to`.
        operation: &'static str,
    },
    Unsupported {
        /// What the compiler cannot do yet, e.g. ``the type `float` ``.
        feature: String,
        span: Span,
        /// Where it was attempted, e.g. `in this variable declaration`.
        context: &'static str,
    },
}

impl fmt::Display for SemanticError {
    /// Writes the headline the compiler prints for this error.
    ///
    /// This is the single source of the wording; the diagnostic's message
    /// defers to it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SemanticError::UndeclaredVariable { name, .. } => {
                write!(f, "cannot find value `{name}` in this scope")
            }
            SemanticError::UndeclaredFunction { name, .. } => {
                write!(f, "cannot find function `{name}` in this scope")
            }
            SemanticError::RedeclaredVariable { name, .. }
            | SemanticError::RedeclaredFunction { name, .. } => {
                write!(f, "the name `{name}` is defined multiple times")
            }
            SemanticError::ArgumentCountMismatch {
                expected, found, ..
            } => write!(
                f,
                "this function takes {} but {} supplied",
                arguments(*expected),
                match found {
                    1 => String::from("1 argument was"),
                    _ => format!("{found} arguments were"),
                }
            ),
            SemanticError::TypeError { .. } => write!(f, "mismatched types"),
            SemanticError::NotIndexable { found, .. } => {
                write!(f, "cannot index into a value of type `{found}`")
            }
            SemanticError::NotDereferenceable { found, .. } => {
                write!(f, "type `{found}` cannot be dereferenced")
            }
            SemanticError::NotAnObject { operation, .. } => {
                write!(f, "cannot {operation} this expression")
            }
            SemanticError::Unsupported { feature, .. } => {
                write!(f, "{feature} is not supported yet")
            }
        }
    }
}

impl std::error::Error for SemanticError {}

impl CompilerError for SemanticError {
    fn into_diagnostic(self) -> Diagnostic {
        // The headline comes from `Display`, so the two never drift apart.
        let message = self.to_string();

        match self {
            SemanticError::UndeclaredVariable {
                span, suggestion, ..
            } => Diagnostic::error(codes::UNDECLARED_VARIABLE, message, span)
                .with_label("not found in this scope")
                .with_optional_help(
                    suggestion
                        .map(|name| format!("a variable with a similar name exists: `{name}`")),
                ),

            SemanticError::UndeclaredFunction {
                span, suggestion, ..
            } => Diagnostic::error(codes::UNDECLARED_FUNCTION, message, span)
                .with_label("not found in this scope")
                .with_optional_help(
                    suggestion
                        .map(|name| format!("a function with a similar name exists: `{name}`")),
                ),

            SemanticError::RedeclaredVariable {
                name,
                span,
                previous,
            } => Diagnostic::error(codes::DUPLICATE_DEFINITION, message, span)
                .with_label(format!("`{name}` redeclared here"))
                .with_secondary(previous, format!("previous declaration of `{name}` here"))
                .with_note(format!(
                    "`{name}` must be declared only once in the same scope"
                )),

            SemanticError::RedeclaredFunction {
                name,
                span,
                previous,
            } => Diagnostic::error(codes::DUPLICATE_DEFINITION, message, span)
                .with_label(format!("`{name}` redefined here"))
                .with_secondary(previous, format!("previous definition of `{name}` here"))
                .with_note(format!("`{name}` must be defined only once")),

            SemanticError::ArgumentCountMismatch {
                name,
                expected,
                found,
                span,
                declared,
            } => Diagnostic::error(codes::WRONG_ARGUMENT_COUNT, message, span)
                .with_label(format!("expected {}, found {found}", arguments(expected)))
                .with_secondary(declared, format!("function `{name}` defined here")),

            SemanticError::TypeError {
                expected,
                found,
                span,
                origin,
                context,
            } => {
                let diagnostic = Diagnostic::error(codes::MISMATCHED_TYPES, message, span)
                    .with_label(format!("expected `{expected}`, found `{found}`"));
                match origin {
                    Some(origin) => diagnostic.with_secondary(origin, context),
                    None => diagnostic.with_note(context),
                }
            }

            SemanticError::NotIndexable { span, .. } => {
                Diagnostic::error(codes::MISMATCHED_TYPES, message, span)
                    .with_label("not an array")
                    .with_note("only a value of array type can be indexed")
            }

            SemanticError::NotDereferenceable { span, .. } => {
                Diagnostic::error(codes::MISMATCHED_TYPES, message, span)
                    .with_label("not a pointer")
            }

            SemanticError::NotAnObject { span, .. } => {
                Diagnostic::error(codes::INVALID_ASSIGNMENT, message, span)
                    .with_label("not an lvalue")
                    .with_note("only variables, array elements and dereferences are lvalues")
            }

            SemanticError::Unsupported { span, context, .. } => {
                Diagnostic::error(codes::UNSUPPORTED, message, span)
                    .with_label(context)
                    .with_note("this compiler implements the subset of C listed in its README")
            }
        }
    }
}

/// Spells out an argument count, e.g. `1 argument` or `3 arguments`.
fn arguments(count: usize) -> String {
    match count {
        1 => String::from("1 argument"),
        _ => format!("{count} arguments"),
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
    /// * `context` - where the type was written, e.g. `in this declaration`
    ///
    /// # Errors
    ///
    /// Returns [`SemanticError::Unsupported`] for types the compiler cannot
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
            | CType::Array(_, None) => Err(SemanticError::Unsupported {
                feature: format!("the type `{ty}`"),
                span,
                context,
            }),
        }
    }
}

/// One parameter of a declared function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamSig {
    pub ty: Type,
    /// The parameter as written, so an argument of the wrong type can be
    /// blamed on the parameter that rejects it.
    pub span: Span,
}

/// A function's declared interface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionSig {
    pub return_ty: Type,
    pub params: Vec<ParamSig>,
    /// The signature as written, quoted when a call does not match it.
    pub span: Span,
}

/// A variable's declared type and where it was declared.
#[derive(Debug, Clone, PartialEq, Eq)]
struct VarInfo {
    ty: Type,
    /// The declared name, quoted when a second declaration collides with it.
    span: Span,
}

/// Semantic analyzer for declarations/statements/expressions.
#[derive(Debug, Default)]
pub struct SemanticAnalyzer {
    /// Variable types, innermost scope last.
    symbols: ScopeStack<VarInfo>,
    /// Signatures of every function in the translation unit, collected before
    /// bodies are checked so functions may be called before they are defined.
    functions: HashMap<String, FunctionSig>,
    /// Return type of the function whose body is being checked, and the
    /// signature that declared it.
    current_function: Option<(Type, Span)>,
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
                return_ty: Type::from_ctype(return_ty, decl.span, "in this return type")?,
                params: Self::param_signatures(params)?,
                span: decl.span,
            };

            if let Some(previous) = self.functions.insert(name.clone(), sig) {
                return Err(SemanticError::RedeclaredFunction {
                    name: name.clone(),
                    span: decl.name_span,
                    previous: previous.span,
                });
            }
        }

        Ok(())
    }

    /// Translates a parameter list into semantic types.
    fn param_signatures(params: &[ParamDecl]) -> SemanticResult<Vec<ParamSig>> {
        params
            .iter()
            .map(|param| {
                Ok(ParamSig {
                    ty: Type::from_ctype(&param.ty, param.span, "in this parameter")?,
                    span: param.span,
                })
            })
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
        let var_ty = Type::from_ctype(ty, decl.span, "in this declaration")?;
        if var_ty == Type::Void {
            return Err(SemanticError::TypeError {
                expected: Type::Int,
                found: Type::Void,
                span: decl.span,
                origin: None,
                context: "a variable cannot have type `void`",
            });
        }

        let info = VarInfo {
            ty: var_ty.clone(),
            span: decl.name_span,
        };
        if !self.symbols.declare(name, info) {
            let previous = self
                .symbols
                .lookup_local(name)
                .expect("`declare` only fails on a name already in this scope");
            return Err(SemanticError::RedeclaredVariable {
                name: name.to_owned(),
                span: decl.name_span,
                previous: previous.span,
            });
        }

        let Some(init) = initializer else {
            return Ok(());
        };

        // Array declarations have no scalar initializer.
        if matches!(var_ty, Type::Array(_, _)) {
            return Err(SemanticError::Unsupported {
                feature: String::from("array initializer lists"),
                span: decl.span,
                context: "in this declaration",
            });
        }

        let init_ty = self.analyze_expr(init)?;
        // The declaration is blamed through its name rather than its whole
        // text: a span that contained the initializer would underline the very
        // expression the error is about.
        Self::expect_type(
            &var_ty,
            &init_ty,
            init.span,
            Some(decl.name_span),
            "expected because of this declaration",
        )
    }

    fn analyze_function_decl(
        &mut self,
        decl: &Decl,
        return_ty: &CType,
        params: &[ParamDecl],
        body: Option<&Stmt>,
    ) -> SemanticResult<()> {
        let ret_ty = Type::from_ctype(return_ty, decl.span, "in this return type")?;

        // A prototype declares no scope of its own; only a definition is checked.
        let Some(body) = body else {
            return Ok(());
        };

        self.symbols.push_scope();

        for param in params {
            let info = VarInfo {
                ty: Type::from_ctype(&param.ty, param.span, "in this parameter")?,
                span: param.span,
            };
            // An unnamed parameter still occupies a slot; `_` cannot collide
            // with a real identifier because it would be a valid C name --
            // a name clash here is reported like any other redeclaration.
            let param_name = param.name.as_deref().unwrap_or("_");
            if !self.symbols.declare(param_name, info) {
                let previous = self
                    .symbols
                    .lookup_local(param_name)
                    .expect("`declare` only fails on a name already in this scope");
                return Err(SemanticError::RedeclaredVariable {
                    name: param_name.to_owned(),
                    span: param.span,
                    previous: previous.span,
                });
            }
        }

        let enclosing = self.current_function.replace((ret_ty, decl.span));
        let result = self.analyze_stmt(body);
        self.current_function = enclosing;
        self.symbols.pop_scope();

        result
    }

    fn analyze_stmt(&mut self, stmt: &Stmt) -> SemanticResult<()> {
        match &stmt.kind {
            StmtKind::Empty => Ok(()),

            StmtKind::Expr(expr) => self.analyze_expr(expr).map(|_| ()),

            StmtKind::Return(value) => {
                // A `return` outside any function cannot be parsed, so the
                // enclosing signature is always known here.
                let (expected, signature) = self
                    .current_function
                    .clone()
                    .unwrap_or((Type::Void, stmt.span));
                let found = match value {
                    Some(expr) => self.analyze_expr(expr)?,
                    None => Type::Void,
                };
                // Blame the returned value when there is one; a bare `return`
                // has only the statement itself to point at.
                let span = value.as_ref().map_or(stmt.span, |expr| expr.span);
                Self::expect_type(
                    &expected,
                    &found,
                    span,
                    Some(signature),
                    "expected because of this return type",
                )
            }

            StmtKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                self.analyze_condition(condition, "an `if` condition must be an integer")?;
                self.analyze_stmt(then_branch)?;
                match else_branch {
                    Some(branch) => self.analyze_stmt(branch),
                    None => Ok(()),
                }
            }

            StmtKind::While { condition, body } => {
                self.analyze_condition(condition, "a `while` condition must be an integer")?;
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
                        analyzer.analyze_for_init(init)?;
                    }
                    if let Some(condition) = condition {
                        analyzer
                            .analyze_condition(condition, "a `for` condition must be an integer")?;
                    }
                    if let Some(step) = step {
                        analyzer.analyze_expr(step)?;
                    }
                    analyzer.analyze_stmt(body)
                })
            }

            StmtKind::Block(items) => {
                self.in_new_scope(|analyzer| analyzer.analyze_block_items(items))
            }
        }
    }

    /// Checks the items of a block in the scope that is already open.
    ///
    /// A failing item is recorded and the block carries on with the next one,
    /// so a single run reports every bad statement in a function rather than
    /// just the first. Recovery is safe here because a declaration binds its
    /// name before its initializer is checked: a later use of that name does
    /// not produce a second, spurious error.
    fn analyze_block_items(&mut self, items: &[BlockItem]) -> SemanticResult<()> {
        for item in items {
            let checked = match item {
                BlockItem::Stmt(stmt) => self.analyze_stmt(stmt),
                BlockItem::Decl(decl) => self.analyze_decl(decl),
            };
            if let Err(error) = checked {
                self.errors.push(error);
            }
        }
        Ok(())
    }

    /// Checks the init clause of a `for`, which belongs to the loop's scope.
    ///
    /// The parser wraps a declaration in the init clause in a block so that it
    /// fits where a statement is expected. That wrapper must not get a scope of
    /// its own, or the loop variable would go out of scope before the condition
    /// is reached: in `for (int i = 0; i < n; i = i + 1)`, `i` has to stay
    /// visible to the condition, the step and the body.
    fn analyze_for_init(&mut self, init: &Stmt) -> SemanticResult<()> {
        match &init.kind {
            StmtKind::Block(items) => self.analyze_block_items(items),
            _ => self.analyze_stmt(init),
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
        Self::expect_type(&Type::Int, &cond_ty, condition.span, None, context)
    }

    fn analyze_expr(&mut self, expr: &Expr) -> SemanticResult<Type> {
        match &expr.kind {
            ExprKind::Literal(literal) => Self::literal_type(literal, expr.span),

            ExprKind::Identifier(name) => match self.symbols.lookup(name) {
                Some(info) => Ok(info.ty.clone()),
                None => Err(SemanticError::UndeclaredVariable {
                    name: name.clone(),
                    span: expr.span,
                    suggestion: suggest::nearest(name, self.symbols.names()).map(str::to_owned),
                }),
            },

            ExprKind::Binary(op, lhs, rhs) => self.analyze_binary(*op, lhs, rhs),

            ExprKind::Unary(op, operand) => self.analyze_unary(*op, operand, expr.span),

            ExprKind::Cast(to_ty, operand) => {
                self.analyze_expr(operand)?;
                Type::from_ctype(to_ty, expr.span, "in this cast")
            }

            ExprKind::Call { callee, args } => self.analyze_call(callee, args, expr.span),

            ExprKind::Index { array, index } => self.analyze_index(array, index),

            // Out of current language scope: reject with clear unsupported
            // diagnostics rather than crashing a later phase.
            ExprKind::MemberAccess { .. } => Err(SemanticError::Unsupported {
                feature: String::from("struct member access"),
                span: expr.span,
                context: "in this expression",
            }),

            ExprKind::SizeOf(_) => Err(SemanticError::Unsupported {
                feature: String::from("`sizeof`"),
                span: expr.span,
                context: "in this expression",
            }),
        }
    }

    /// The type of a literal. Only integer literals are supported so far.
    fn literal_type(literal: &Literal, span: Span) -> SemanticResult<Type> {
        match literal {
            Literal::Int(_) => Ok(Type::Int),
            Literal::Float(_) => Err(SemanticError::unsupported_literal("floating-point", span)),
            Literal::Char(_) => Err(SemanticError::unsupported_literal("character", span)),
            Literal::String(_) => Err(SemanticError::unsupported_literal("string", span)),
        }
    }

    /// Checks `array[index]`, which requires an array and an integer index.
    fn analyze_index(&mut self, array: &Expr, index: &Expr) -> SemanticResult<Type> {
        let base_ty = self.analyze_expr(array)?;
        let Type::Array(elem_ty, _) = base_ty else {
            return Err(SemanticError::NotIndexable {
                found: base_ty,
                span: array.span,
            });
        };

        let index_ty = self.analyze_expr(index)?;
        Self::expect_type(
            &Type::Int,
            &index_ty,
            index.span,
            None,
            "an array index must be an integer",
        )?;
        Ok(*elem_ty)
    }

    fn analyze_call(&mut self, callee: &Expr, args: &[Expr], span: Span) -> SemanticResult<Type> {
        let ExprKind::Identifier(name) = &callee.kind else {
            // Calls through function pointers are not supported yet.
            return Err(SemanticError::Unsupported {
                feature: String::from("calling anything but a named function"),
                span: callee.span,
                context: "in this call",
            });
        };

        let Some(sig) = self.functions.get(name).cloned() else {
            return Err(SemanticError::UndeclaredFunction {
                name: name.clone(),
                span: callee.span,
                suggestion: suggest::nearest(name, self.functions.keys().map(String::as_str))
                    .map(str::to_owned),
            });
        };

        if args.len() != sig.params.len() {
            return Err(SemanticError::ArgumentCountMismatch {
                name: name.clone(),
                expected: sig.params.len(),
                found: args.len(),
                span,
                declared: sig.span,
            });
        }

        for (arg, param) in args.iter().zip(&sig.params) {
            let found = self.analyze_expr(arg)?;
            Self::expect_type(
                &param.ty,
                &found,
                arg.span,
                Some(param.span),
                "expected because of this parameter",
            )?;
        }

        Ok(sig.return_ty)
    }

    fn analyze_binary(&mut self, op: BinaryOp, lhs: &Expr, rhs: &Expr) -> SemanticResult<Type> {
        if op == BinaryOp::Assign {
            if !Self::is_lvalue(lhs) {
                return Err(SemanticError::NotAnObject {
                    span: lhs.span,
                    operation: "assign to",
                });
            }

            let target_ty = self.analyze_expr(lhs)?;
            let value_ty = self.analyze_expr(rhs)?;
            Self::expect_type(
                &target_ty,
                &value_ty,
                rhs.span,
                Some(lhs.span),
                "expected because of this assignment target",
            )?;
            return Ok(target_ty);
        }

        // For the current subset every arithmetic, comparison, logical and
        // bitwise operator takes two ints...
        let lhs_ty = self.analyze_expr(lhs)?;
        let rhs_ty = self.analyze_expr(rhs)?;
        Self::expect_type(
            &Type::Int,
            &lhs_ty,
            lhs.span,
            None,
            OPERANDS_MUST_BE_INTEGERS,
        )?;
        Self::expect_type(
            &Type::Int,
            &rhs_ty,
            rhs.span,
            None,
            OPERANDS_MUST_BE_INTEGERS,
        )?;

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
                Self::expect_type(
                    &Type::Int,
                    &operand_ty,
                    operand.span,
                    None,
                    OPERANDS_MUST_BE_INTEGERS,
                )?;
                Ok(Type::Int)
            }
            UnaryOp::AddressOf => {
                if !Self::is_lvalue(operand) {
                    return Err(SemanticError::NotAnObject {
                        span: operand.span,
                        operation: "take the address of",
                    });
                }
                Ok(Type::Pointer(Box::new(operand_ty)))
            }
            UnaryOp::Deref => match operand_ty {
                Type::Pointer(pointee) => Ok(*pointee),
                _ => Err(SemanticError::NotDereferenceable {
                    found: operand_ty,
                    span,
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
    /// # Arguments
    ///
    /// * `expected` - the type this position requires
    /// * `found` - the type of the expression written there
    /// * `span` - the expression to blame
    /// * `origin` - the declaration that imposed `expected`, when there is one
    /// * `context` - why `expected` is required; see
    ///   [`SemanticError::TypeError`]
    ///
    /// # Errors
    ///
    /// Returns [`SemanticError::TypeError`] blaming `span`.
    fn expect_type(
        expected: &Type,
        found: &Type,
        span: Span,
        origin: Option<Span>,
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
            origin,
            context,
        })
    }
}

/// Why an arithmetic operand must be an integer; shared by the unary and
/// binary checks so the two never word it differently.
const OPERANDS_MUST_BE_INTEGERS: &str =
    "arithmetic, comparison and bitwise operators only apply to integers";

impl SemanticError {
    /// Error for a literal whose type the compiler cannot represent yet.
    fn unsupported_literal(kind: &str, span: Span) -> Self {
        SemanticError::Unsupported {
            feature: format!("{kind} literals"),
            span,
            context: "in this expression",
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
    fn accepts_a_variable_declared_in_a_for_init_clause() {
        analyze_ok(
            "int main() { int s = 0; for (int i = 0; i < 3; i = i + 1) { s = s + i; } return s; }",
        );
    }

    #[test]
    fn for_loop_variable_does_not_outlive_the_loop() {
        match analyze_err("int main() { for (int i = 0; i < 3; i = i + 1) { } return i; }") {
            SemanticError::UndeclaredVariable { name, .. } => assert_eq!(name, "i"),
            other => panic!("expected UndeclaredVariable, got: {other:?}"),
        }
    }

    #[test]
    fn accepts_empty_statements() {
        analyze_ok("int main() { ; if (1) ; else ; while (0) ; return 0; }");
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
        assert!(matches!(error, SemanticError::NotAnObject { .. }));
    }

    #[test]
    fn checks_pointer_dereference_and_address_of() {
        analyze_ok("int main() { int x = 1; int *p = &x; *p = 2; return *p; }");
    }

    #[test]
    fn rejects_dereference_of_non_pointer() {
        let error = analyze_err("int main() { int x = 1; return *x; }");
        assert!(matches!(error, SemanticError::NotDereferenceable { .. }));
    }

    #[test]
    fn rejects_unsupported_types() {
        let error = analyze_err("int main() { float f; return 0; }");
        assert!(matches!(error, SemanticError::Unsupported { .. }));
    }

    #[test]
    fn reports_errors_from_several_functions() {
        let errors = analyze("int f() { return a; } int g() { return b; }");
        assert_eq!(errors.len(), 2, "got {errors:?}");
    }

    #[test]
    fn headline_is_the_diagnostic_message() {
        let error = analyze_err("int main() { return missing(); }");
        let headline = error.to_string();

        assert_eq!(headline, "cannot find function `missing` in this scope");
        assert_eq!(error.into_diagnostic().message, headline);
    }

    #[test]
    fn suggests_a_similarly_spelled_variable() {
        match analyze_err("int main() { int value = 1; return valu; }") {
            SemanticError::UndeclaredVariable { suggestion, .. } => {
                assert_eq!(suggestion.as_deref(), Some("value"));
            }
            other => panic!("expected UndeclaredVariable, got: {other:?}"),
        }
    }

    #[test]
    fn points_a_redeclaration_at_the_first_one() {
        match analyze_err("int main() { int x; int x; return 0; }") {
            SemanticError::RedeclaredVariable { span, previous, .. } => {
                assert_eq!((previous.line, previous.column), (1, 18));
                assert_eq!((span.line, span.column), (1, 25));
            }
            other => panic!("expected RedeclaredVariable, got: {other:?}"),
        }
    }
}
