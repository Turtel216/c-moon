//! Name resolution: binds every identifier to a unique variable id.
//!
//! C lets an inner block shadow a name from an outer one, so a name is not
//! enough to identify a variable once the AST is flattened into IR. This pass
//! walks the tree and records, for every declaration and every use, which
//! variable is meant. The result is a side table keyed by [`NodeId`], so the
//! AST itself is left untouched.

use std::collections::{HashMap, HashSet};
use std::fmt;

use crate::frontend::ast::{
    BlockItem, Decl, DeclKind, Expr, ExprKind, NodeId, ParamDecl, Stmt, StmtKind,
};
use crate::frontend::scope::ScopeStack;
use crate::frontend::span::Span;

/// Identifies one variable in a translation unit, shadowing included.
pub type VarId = usize;

/// What each identifier in the AST resolves to.
#[derive(Debug, Clone, Default)]
pub struct ResolutionMap {
    /// Maps *identifier-expression node id* -> globally unique variable id
    pub expr_to_var: HashMap<NodeId, VarId>,
    /// Maps *declaration node id* -> globally unique variable id
    pub decl_to_var: HashMap<NodeId, VarId>,
}

impl ResolutionMap {
    pub fn new() -> Self {
        Self::default()
    }
}

/// A name that could not be resolved.
///
/// Semantic analysis rejects these programs first, so in the compiler pipeline
/// a rename error means the two passes disagree -- see [`resolve_names`].
#[derive(Debug, Clone, PartialEq)]
pub enum RenameError {
    UndeclaredVariable { name: String, span: Span },
    RedeclarationInSameScope { name: String, span: Span },
    UndeclaredFunction { name: String, span: Span },
}

impl fmt::Display for RenameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RenameError::UndeclaredVariable { name, span } => {
                write!(f, "{span}: use of undeclared variable '{name}'")
            }
            RenameError::RedeclarationInSameScope { name, span } => {
                write!(f, "{span}: redeclaration of '{name}' in the same scope")
            }
            RenameError::UndeclaredFunction { name, span } => {
                write!(f, "{span}: call to undeclared function '{name}'")
            }
        }
    }
}

impl std::error::Error for RenameError {}

type RenameResult<T> = Result<T, RenameError>;

/// Walks the AST assigning a unique id to every declared variable.
struct Renamer {
    /// Variables in scope, innermost scope last.
    scopes: ScopeStack<VarId>,
    /// Functions declared in the translation unit. They live in their own
    /// namespace here because a call resolves to a function, not to a variable.
    functions: HashSet<String>,
    next_var_id: VarId,
    resolution: ResolutionMap,
}

impl Renamer {
    fn new() -> Self {
        Self {
            scopes: ScopeStack::new(),
            functions: HashSet::new(),
            next_var_id: 0,
            resolution: ResolutionMap::new(),
        }
    }

    /// Declares `name` in the innermost scope and binds the declaring node to
    /// the new variable id.
    ///
    /// # Errors
    ///
    /// Returns [`RenameError::RedeclarationInSameScope`] if the name is already
    /// declared in that scope. Shadowing an enclosing scope is allowed.
    fn declare(&mut self, name: &str, node: NodeId, span: Span) -> RenameResult<()> {
        let var_id = self.next_var_id;
        if !self.scopes.declare(name, var_id) {
            return Err(RenameError::RedeclarationInSameScope {
                name: name.to_owned(),
                span,
            });
        }

        self.next_var_id += 1;
        self.resolution.decl_to_var.insert(node, var_id);
        Ok(())
    }

    fn resolve_decl(&mut self, decl: &Decl) -> RenameResult<()> {
        match &decl.kind {
            DeclKind::Variable {
                name, initializer, ..
            } => {
                // The variable is declared before its initializer is resolved:
                // in C a name is in scope from the end of its declarator, so
                // the `x` in `int x = x;` is the one being declared. Semantic
                // analysis makes the same choice, and the two passes must
                // agree.
                self.declare(name, decl.id, decl.span)?;
                match initializer {
                    Some(init) => self.resolve_expr(init),
                    None => Ok(()),
                }
            }

            DeclKind::Function {
                name, params, body, ..
            } => {
                self.functions.insert(name.clone());
                self.resolve_function(params, body.as_ref(), decl.span)
            }

            DeclKind::Struct { .. } => Ok(()),
        }
    }

    /// Resolves a function's parameters and body in a scope of their own.
    fn resolve_function(
        &mut self,
        params: &[ParamDecl],
        body: Option<&Stmt>,
        span: Span,
    ) -> RenameResult<()> {
        self.in_new_scope(|renamer| {
            for param in params {
                // An unnamed parameter cannot be referred to, so it needs no id.
                if let Some(name) = param.name.as_deref() {
                    renamer.declare(name, param.id, span)?;
                }
            }

            match body {
                Some(body) => renamer.resolve_stmt(body),
                None => Ok(()),
            }
        })
    }

    fn resolve_stmt(&mut self, stmt: &Stmt) -> RenameResult<()> {
        match &stmt.kind {
            StmtKind::Expr(expr) => self.resolve_expr(expr),

            StmtKind::Return(value) => match value {
                Some(expr) => self.resolve_expr(expr),
                None => Ok(()),
            },

            StmtKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                self.resolve_expr(condition)?;
                self.resolve_stmt(then_branch)?;
                match else_branch {
                    Some(branch) => self.resolve_stmt(branch),
                    None => Ok(()),
                }
            }

            StmtKind::While { condition, body } => {
                self.resolve_expr(condition)?;
                self.resolve_stmt(body)
            }

            StmtKind::For {
                init,
                condition,
                step,
                body,
            } => self.in_new_scope(|renamer| {
                // A variable declared in the init clause is scoped to the loop.
                if let Some(init) = init {
                    renamer.resolve_stmt(init)?;
                }
                if let Some(condition) = condition {
                    renamer.resolve_expr(condition)?;
                }
                if let Some(step) = step {
                    renamer.resolve_expr(step)?;
                }
                renamer.resolve_stmt(body)
            }),

            StmtKind::Block(items) => self.in_new_scope(|renamer| {
                for item in items {
                    match item {
                        BlockItem::Stmt(stmt) => renamer.resolve_stmt(stmt)?,
                        BlockItem::Decl(decl) => renamer.resolve_decl(decl)?,
                    }
                }
                Ok(())
            }),
        }
    }

    fn resolve_expr(&mut self, expr: &Expr) -> RenameResult<()> {
        match &expr.kind {
            ExprKind::Literal(_) => Ok(()),

            ExprKind::Identifier(name) => {
                let var_id =
                    *self
                        .scopes
                        .lookup(name)
                        .ok_or_else(|| RenameError::UndeclaredVariable {
                            name: name.clone(),
                            span: expr.span,
                        })?;
                self.resolution.expr_to_var.insert(expr.id, var_id);
                Ok(())
            }

            ExprKind::Binary(_, lhs, rhs) => {
                self.resolve_expr(lhs)?;
                self.resolve_expr(rhs)
            }

            ExprKind::Unary(_, operand) => self.resolve_expr(operand),

            ExprKind::Call { callee, args } => {
                self.resolve_callee(callee)?;
                for arg in args {
                    self.resolve_expr(arg)?;
                }
                Ok(())
            }

            ExprKind::Index { array, index } => {
                self.resolve_expr(array)?;
                self.resolve_expr(index)
            }

            ExprKind::MemberAccess { base, .. } => self.resolve_expr(base),
            ExprKind::Cast(_, operand) => self.resolve_expr(operand),
            ExprKind::SizeOf(operand) => self.resolve_expr(operand),
        }
    }

    /// Checks the callee of a call.
    ///
    /// A plain function name is left unbound on purpose: it names a function,
    /// not a variable, so it gets no variable id. Anything else is an ordinary
    /// expression and is resolved as one.
    fn resolve_callee(&mut self, callee: &Expr) -> RenameResult<()> {
        let ExprKind::Identifier(name) = &callee.kind else {
            return self.resolve_expr(callee);
        };

        if !self.functions.contains(name) && self.scopes.lookup(name).is_none() {
            return Err(RenameError::UndeclaredFunction {
                name: name.clone(),
                span: callee.span,
            });
        }
        Ok(())
    }

    /// Runs `resolution` in a nested scope, closing it even when it fails.
    fn in_new_scope<T>(
        &mut self,
        resolution: impl FnOnce(&mut Self) -> RenameResult<T>,
    ) -> RenameResult<T> {
        self.scopes.push_scope();
        let result = resolution(self);
        self.scopes.pop_scope();
        result
    }
}

/// Resolves every identifier in a translation unit.
///
/// # Arguments
///
/// * `decls` - parsed and semantically valid top-level declarations
///
/// # Returns
///
/// The side table binding declaration and identifier nodes to variable ids.
///
/// # Errors
///
/// Returns the first [`RenameError`]. Semantic analysis already rejects the
/// programs this pass can reject, so an error here means the two passes
/// disagree -- a compiler bug rather than a user error.
pub fn resolve_names(decls: &[Decl]) -> RenameResult<ResolutionMap> {
    let mut renamer = Renamer::new();

    // Functions are collected up front so a call may appear above the
    // definition it refers to.
    for decl in decls {
        if let DeclKind::Function { name, .. } = &decl.kind {
            renamer.functions.insert(name.clone());
        }
    }

    for decl in decls {
        renamer.resolve_decl(decl)?;
    }

    Ok(renamer.resolution)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::lexer::Lexer;
    use crate::frontend::parser::Parser;

    /// The node ids of a parsed program, in source order, so a test can ask for
    /// "the second use of `x`" without hard-coding ids.
    struct NodeIndex {
        uses: Vec<(String, NodeId)>,
        decls: Vec<(String, NodeId)>,
    }

    impl NodeIndex {
        fn build(decls: &[Decl]) -> Self {
            let mut index = Self {
                uses: Vec::new(),
                decls: Vec::new(),
            };
            for decl in decls {
                index.index_decl(decl);
            }
            index
        }

        fn index_decl(&mut self, decl: &Decl) {
            match &decl.kind {
                DeclKind::Variable {
                    name, initializer, ..
                } => {
                    if let Some(init) = initializer {
                        self.index_expr(init);
                    }
                    self.decls.push((name.clone(), decl.id));
                }
                DeclKind::Function { params, body, .. } => {
                    for param in params {
                        if let Some(name) = &param.name {
                            self.decls.push((name.clone(), param.id));
                        }
                    }
                    if let Some(body) = body {
                        self.index_stmt(body);
                    }
                }
                DeclKind::Struct { .. } => {}
            }
        }

        fn index_stmt(&mut self, stmt: &Stmt) {
            match &stmt.kind {
                StmtKind::Expr(expr) => self.index_expr(expr),
                StmtKind::Return(value) => {
                    if let Some(expr) = value {
                        self.index_expr(expr);
                    }
                }
                StmtKind::If {
                    condition,
                    then_branch,
                    else_branch,
                } => {
                    self.index_expr(condition);
                    self.index_stmt(then_branch);
                    if let Some(branch) = else_branch {
                        self.index_stmt(branch);
                    }
                }
                StmtKind::While { condition, body } => {
                    self.index_expr(condition);
                    self.index_stmt(body);
                }
                StmtKind::For {
                    init,
                    condition,
                    step,
                    body,
                } => {
                    if let Some(init) = init {
                        self.index_stmt(init);
                    }
                    if let Some(condition) = condition {
                        self.index_expr(condition);
                    }
                    if let Some(step) = step {
                        self.index_expr(step);
                    }
                    self.index_stmt(body);
                }
                StmtKind::Block(items) => {
                    for item in items {
                        match item {
                            BlockItem::Stmt(stmt) => self.index_stmt(stmt),
                            BlockItem::Decl(decl) => self.index_decl(decl),
                        }
                    }
                }
            }
        }

        fn index_expr(&mut self, expr: &Expr) {
            match &expr.kind {
                ExprKind::Literal(_) => {}
                ExprKind::Identifier(name) => self.uses.push((name.clone(), expr.id)),
                ExprKind::Binary(_, lhs, rhs) => {
                    self.index_expr(lhs);
                    self.index_expr(rhs);
                }
                ExprKind::Unary(_, operand)
                | ExprKind::Cast(_, operand)
                | ExprKind::SizeOf(operand)
                | ExprKind::MemberAccess { base: operand, .. } => self.index_expr(operand),
                ExprKind::Call { callee, args } => {
                    self.index_expr(callee);
                    for arg in args {
                        self.index_expr(arg);
                    }
                }
                ExprKind::Index { array, index } => {
                    self.index_expr(array);
                    self.index_expr(index);
                }
            }
        }

        /// Node id of the `occurrence`-th use of `name`, counting from zero.
        fn use_of(&self, name: &str, occurrence: usize) -> NodeId {
            Self::nth(&self.uses, name, occurrence, "use")
        }

        /// Node id of the `occurrence`-th declaration of `name`.
        fn decl_of(&self, name: &str, occurrence: usize) -> NodeId {
            Self::nth(&self.decls, name, occurrence, "declaration")
        }

        fn nth(entries: &[(String, NodeId)], name: &str, occurrence: usize, what: &str) -> NodeId {
            entries
                .iter()
                .filter(|(entry, _)| entry == name)
                .map(|(_, id)| *id)
                .nth(occurrence)
                .unwrap_or_else(|| panic!("no {what} #{occurrence} of '{name}'"))
        }
    }

    /// Parses `src` and returns its declarations alongside their node index.
    fn parse(src: &str) -> (Vec<Decl>, NodeIndex) {
        let mut parser = Parser::from_lexer(Lexer::new(src)).expect("lexing should succeed");
        let (decls, errors) = parser.parse_translation_unit();
        assert!(errors.is_empty(), "unexpected parse errors: {errors:?}");

        let index = NodeIndex::build(&decls);
        (decls, index)
    }

    #[test]
    fn resolves_uses_to_their_declaration() {
        let src = "
            int foo(int x, int y) { return x + 1; }
            int main() {
                int x = 1;
                int y = foo(x, 2);
                { int x = 5; }
                if (x < y) { return 0; }
                return 1;
            }
        ";
        let (decls, index) = parse(src);
        let map = resolve_names(&decls).expect("name resolution should succeed");

        let main_x = map.decl_to_var[&index.decl_of("x", 1)];
        let main_y = map.decl_to_var[&index.decl_of("y", 1)];
        let inner_x = map.decl_to_var[&index.decl_of("x", 2)];
        let foo_param_x = map.decl_to_var[&index.decl_of("x", 0)];

        // `x` in `foo(x, 2)` and in `x < y` is main's `x`.
        assert_eq!(map.expr_to_var[&index.use_of("x", 1)], main_x);
        assert_eq!(map.expr_to_var[&index.use_of("x", 2)], main_x);
        assert_eq!(map.expr_to_var[&index.use_of("y", 0)], main_y);

        // `x` in foo's body is foo's parameter.
        assert_eq!(map.expr_to_var[&index.use_of("x", 0)], foo_param_x);

        assert_ne!(inner_x, main_x, "shadowed x must have a different id");
        assert_ne!(foo_param_x, main_x, "foo's x is distinct from main's x");
    }

    #[test]
    fn function_callee_is_not_a_variable() {
        let (decls, index) = parse("int foo() { return 0; } int main() { return foo(); }");
        let map = resolve_names(&decls).expect("name resolution should succeed");

        assert!(
            !map.expr_to_var.contains_key(&index.use_of("foo", 0)),
            "a function name is not a variable use"
        );
    }

    #[test]
    fn errors_on_undeclared_variable() {
        let (decls, _) = parse("int main() { return z; }");

        match resolve_names(&decls).unwrap_err() {
            RenameError::UndeclaredVariable { name, .. } => assert_eq!(name, "z"),
            other => panic!("expected UndeclaredVariable, got {other:?}"),
        }
    }

    #[test]
    fn errors_on_undeclared_function() {
        let (decls, _) = parse("int main() { return missing(); }");

        match resolve_names(&decls).unwrap_err() {
            RenameError::UndeclaredFunction { name, .. } => assert_eq!(name, "missing"),
            other => panic!("expected UndeclaredFunction, got {other:?}"),
        }
    }

    #[test]
    fn errors_on_redeclaration_in_same_scope() {
        let (decls, _) = parse("int main() { int x = 1; int x = 2; return x; }");

        match resolve_names(&decls).unwrap_err() {
            RenameError::RedeclarationInSameScope { name, .. } => assert_eq!(name, "x"),
            other => panic!("expected RedeclarationInSameScope, got {other:?}"),
        }
    }

    #[test]
    fn allows_shadowing_in_nested_block() {
        let (decls, index) = parse("int main() { int x = 1; { int x = 2; } return x; }");
        let map = resolve_names(&decls).expect("should allow shadowing");

        let outer_x = map.decl_to_var[&index.decl_of("x", 0)];
        let inner_x = map.decl_to_var[&index.decl_of("x", 1)];

        assert_ne!(outer_x, inner_x);
        assert_eq!(
            map.expr_to_var[&index.use_of("x", 0)],
            outer_x,
            "after the nested block, x resolves to the outer x"
        );
    }
}
