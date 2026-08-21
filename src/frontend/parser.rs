//! Hand-written recursive descent parser: tokens in, [`ast`](crate::frontend::ast) out.
//!
//! Expressions use precedence climbing (see [`Parser::parse_binary_expr`]),
//! which expresses C's binary operator precedence as a table instead of one
//! function per level.
//!
//! The parser recovers from errors rather than stopping at the first one: a
//! failed declaration or block item is recorded and the token stream is
//! resynchronised, so one run reports every syntax error in the file.

use crate::driver::diagnostics::{CompilerError, Diagnostic, codes};
use crate::frontend::ast::{
    BinaryOp, BlockItem, CType, Decl, DeclKind, Expr, ExprKind, Literal, NodeId, ParamDecl, Stmt,
    StmtKind, UnaryOp,
};
use crate::frontend::lexer::{LexError, Lexer, Token, TokenKind};
use crate::frontend::span::Span;

/// A syntax error, tied to the source text that could not be accepted.
///
/// The fields are private: outside the parser an error is only ever turned
/// into a [`Diagnostic`], so the wording stays this module's business.
#[derive(Debug, Clone, PartialEq)]
pub struct ParseError {
    /// The headline, e.g. ``expected `;`, found `return` ``.
    message: String,
    /// What the diagnostic points at.
    span: Span,
    /// Caption written under the span, e.g. ``expected `;` after declaration``.
    label: String,
    /// Which class of error this is, for the `error[E01xx]` header.
    code: &'static str,
}

impl ParseError {
    /// The parser wanted `expectation` and `found` turned up instead.
    ///
    /// # Arguments
    ///
    /// * `expectation` - what the grammar allows here, already quoted where it
    ///   names a literal token (see [`TokenKind`]'s `Display`)
    /// * `context` - where in the construct the error is, e.g. `after
    ///   declaration`; empty when the expectation speaks for itself
    /// * `found` - the token that was actually there
    /// * `span` - the text to underline, which is not always `found`'s
    fn expected(expectation: &str, context: &str, found: &Token<'_>, span: Span) -> Self {
        Self {
            message: format!("expected {expectation}, found {}", found.describe()),
            span,
            label: match context.is_empty() {
                true => format!("expected {expectation}"),
                false => format!("expected {expectation} {context}"),
            },
            code: codes::SYNTAX,
        }
    }

    /// Wraps a lexical error, which reaches the caller as a parse failure.
    fn lexical(error: LexError, span: Span) -> Self {
        Self {
            message: error.to_string(),
            span,
            label: String::from("not valid C"),
            code: codes::LEXICAL,
        }
    }
}

impl CompilerError for ParseError {
    fn into_diagnostic(self) -> Diagnostic {
        Diagnostic::error(self.code, self.message, self.span).with_label(self.label)
    }
}

type PResult<T> = Result<T, ParseError>;

/// Prefix operators, keyed by the token that introduces them.
const PREFIX_OPERATORS: [(TokenKind, UnaryOp); 7] = [
    (TokenKind::Minus, UnaryOp::Neg),
    (TokenKind::Bang, UnaryOp::Not),
    (TokenKind::Tilde, UnaryOp::BitNot),
    (TokenKind::Star, UnaryOp::Deref),
    (TokenKind::Ampersand, UnaryOp::AddressOf),
    (TokenKind::PlusPlus, UnaryOp::PreInc),
    (TokenKind::MinusMinus, UnaryOp::PreDec),
];

/// Postfix operators, keyed by the token that introduces them.
const POSTFIX_OPERATORS: [(TokenKind, UnaryOp); 2] = [
    (TokenKind::PlusPlus, UnaryOp::PostInc),
    (TokenKind::MinusMinus, UnaryOp::PostDec),
];

/// Where a declaration appears, which decides what it may declare.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeclContext {
    /// File scope, where functions may be declared and defined.
    TopLevel,
    /// Inside a block. C has no nested functions, so only variables are
    /// accepted here.
    Block,
}

/// A parsed declarator: what a declaration declares, before its initializer,
/// parameter list or body is read.
struct Declarator {
    name: String,
    /// Where the name is written, for diagnostics about the name itself.
    name_span: Span,
    /// The type built up by the pointer and array modifiers around the name.
    ty: CType,
}

pub struct Parser<'a> {
    /// The whole token stream, always ending in [`TokenKind::Eof`].
    tokens: Vec<Token<'a>>,
    pos: usize,
    next_node_id: NodeId,
    /// Accumulated parse errors (for multi-error reporting).
    errors: Vec<ParseError>,
}

impl<'a> Parser<'a> {
    /// Drains `lexer` into a token buffer the parser can look ahead in.
    ///
    /// # Errors
    ///
    /// Returns a [`ParseError`] on the first malformed token, since a lexical
    /// error leaves the rest of the stream meaningless.
    pub fn from_lexer(mut lexer: Lexer<'a>) -> PResult<Self> {
        let mut tokens = Vec::new();
        loop {
            let token = lexer.next_token();
            if let TokenKind::Error(error) = token.kind {
                return Err(ParseError::lexical(error, token.span));
            }
            let is_eof = token.kind == TokenKind::Eof;
            tokens.push(token);
            if is_eof {
                break;
            }
        }

        Ok(Self {
            tokens,
            pos: 0,
            next_node_id: 0,
            errors: Vec::new(),
        })
    }

    /// Parse a complete translation unit, recovering from errors.
    ///
    /// Returns the successfully-parsed declarations alongside any
    /// errors encountered. This allows the compiler to report
    /// multiple syntax errors in a single pass.
    pub fn parse_translation_unit(&mut self) -> (Vec<Decl>, Vec<ParseError>) {
        let mut decls = Vec::new();
        while !self.check(TokenKind::Eof) {
            match self.parse_decl(DeclContext::TopLevel) {
                Ok(decl) => decls.push(decl),
                Err(error) => self.recover_from(error),
            }
        }
        (decls, std::mem::take(&mut self.errors))
    }

    /// Records an error and resynchronises the token stream so parsing can go on.
    fn recover_from(&mut self, error: ParseError) {
        self.errors.push(error);
        self.synchronize();
    }

    /// Advance tokens until we reach a synchronization point.
    ///
    /// A synchronization point is a position where we are likely at
    /// the start of a new statement or declaration, allowing the
    /// parser to resume correctly after an error. We stop at:
    ///   - After a semicolon (end of statement)
    ///   - After a closing brace (end of block)
    ///   - Before a keyword that starts a declaration or statement
    fn synchronize(&mut self) {
        while !self.check(TokenKind::Eof) {
            // If we just passed a semicolon or closing brace, we are
            // at a good resumption point.
            if matches!(self.prev().kind, TokenKind::Semicolon | TokenKind::RBrace) {
                return;
            }

            // If the current token starts a new declaration or statement, stop
            // before consuming it. A closing brace ends the enclosing block, so
            // it is left for the caller to consume.
            match self.current().kind {
                TokenKind::If
                | TokenKind::While
                | TokenKind::For
                | TokenKind::Return
                | TokenKind::RBrace => return,
                kind if Self::starts_type(kind) => return,
                _ => {
                    self.advance();
                }
            }
        }
    }

    // ### Declarations ###

    /// Parses one declaration: a struct definition, a variable, or -- at file
    /// scope -- a function prototype or definition.
    fn parse_decl(&mut self, context: DeclContext) -> PResult<Decl> {
        // Every declaration node spans from its first token to its last, so the
        // start is remembered before anything is consumed.
        let start = self.current().span;

        if self.at_struct_decl() {
            return self.parse_struct_decl(start);
        }

        let base_ty = self.parse_type_specifier()?;
        let declarator = self.parse_declarator(base_ty)?;

        // Only a declarator followed by `(` at file scope is a function; inside
        // a block the variable path runs instead and rejects the `(`.
        if context == DeclContext::TopLevel && self.match_kind(TokenKind::LParen) {
            return self.parse_function_tail(start, declarator);
        }
        self.parse_variable_tail(start, declarator)
    }

    /// Parses a function's parameters and body, with `(` consumed.
    fn parse_function_tail(&mut self, start: Span, declarator: Declarator) -> PResult<Decl> {
        let params = self.parse_param_list()?;
        self.expect(TokenKind::RParen, "after parameter list")?;

        // The signature is what a diagnostic quotes; the body would drag the
        // whole function into the snippet.
        let signature = start.to(self.prev_span());

        let body = if self.match_kind(TokenKind::LBrace) {
            Some(self.parse_block(self.prev_span())?)
        } else {
            self.expect(TokenKind::Semicolon, "after function prototype")?;
            None
        };

        Ok(self.mk_decl(
            DeclKind::Function {
                return_ty: declarator.ty,
                name: declarator.name,
                params,
                body,
            },
            signature,
            declarator.name_span,
        ))
    }

    /// Parses a variable declaration's initializer and terminator, with the
    /// declarator consumed.
    fn parse_variable_tail(&mut self, start: Span, declarator: Declarator) -> PResult<Decl> {
        let initializer = if self.match_kind(TokenKind::Eq) {
            Some(self.parse_expr()?)
        } else {
            None
        };
        self.expect(TokenKind::Semicolon, "after declaration")?;

        Ok(self.mk_decl(
            DeclKind::Variable {
                ty: declarator.ty,
                name: declarator.name,
                initializer,
            },
            start.to(self.prev_span()),
            declarator.name_span,
        ))
    }

    /// Whether the cursor sits on a struct declaration -- `struct [Tag] { ... };`
    /// or the forward declaration `struct Tag;` -- rather than on a use of a
    /// struct type such as `struct Tag x;`.
    fn at_struct_decl(&self) -> bool {
        if !self.check(TokenKind::Struct) {
            return false;
        }
        match self.peek_kind(1) {
            Some(TokenKind::LBrace) => true,
            Some(TokenKind::Identifier) => matches!(
                self.peek_kind(2),
                Some(TokenKind::LBrace | TokenKind::Semicolon)
            ),
            _ => false,
        }
    }

    /// Parses `struct [Tag] { members };` or `struct Tag;`, with the cursor on
    /// `struct`.
    fn parse_struct_decl(&mut self, start: Span) -> PResult<Decl> {
        self.expect(TokenKind::Struct, "")?;

        // An anonymous struct has no name to point at, so the tag's span falls
        // back to the `struct` keyword.
        let (name, name_span) = if self.check(TokenKind::Identifier) {
            let tag = self.advance();
            (Some(tag.lexeme.to_string()), tag.span)
        } else {
            (None, start)
        };

        // A forward declaration names the tag but declares no members.
        let members = if self.match_kind(TokenKind::LBrace) {
            self.parse_struct_members()?
        } else {
            Vec::new()
        };
        self.expect(TokenKind::Semicolon, "after struct declaration")?;

        Ok(self.mk_decl(
            DeclKind::Struct { name, members },
            start.to(self.prev_span()),
            name_span,
        ))
    }

    /// Parses a struct body, with `{` consumed and up to and including `}`.
    fn parse_struct_members(&mut self) -> PResult<Vec<Decl>> {
        let mut members = Vec::new();
        while !self.check(TokenKind::RBrace) && !self.check(TokenKind::Eof) {
            let start = self.current().span;
            let base_ty = self.parse_type_specifier()?;
            let declarator = self.parse_declarator(base_ty)?;
            self.expect(TokenKind::Semicolon, "after struct member")?;
            members.push(self.mk_decl(
                DeclKind::Variable {
                    ty: declarator.ty,
                    name: declarator.name,
                    initializer: None,
                },
                start.to(self.prev_span()),
                declarator.name_span,
            ));
        }
        self.expect(TokenKind::RBrace, "after struct body")?;
        Ok(members)
    }

    /// Parses the base type of a declaration, e.g. the `int` of `int *p[4]`.
    ///
    /// `long` is the one specifier written as more than one word: `long` and
    /// `long int` name the same type, so the optional `int` is consumed here.
    fn parse_type_specifier(&mut self) -> PResult<CType> {
        let ty = match self.current().kind {
            TokenKind::Int => CType::Int,
            TokenKind::Long => {
                self.advance();
                self.match_kind(TokenKind::Int);
                return Ok(CType::Long);
            }
            TokenKind::Char => CType::Char,
            TokenKind::Float => CType::Float,
            TokenKind::Double => CType::Double,
            TokenKind::Void => CType::Void,
            TokenKind::Struct => {
                self.advance();
                return Ok(CType::Struct(self.expect_identifier("after `struct`")?.0));
            }
            _ => return self.err_here("a type specifier"),
        };
        self.advance();
        Ok(ty)
    }

    /// Parses the declared name together with the pointer and array modifiers
    /// wrapped around it, e.g. `*p[10]` given a base type of `int`.
    ///
    /// # Returns
    ///
    /// The declared name, where it is written, and the type it stands for --
    /// here `p` and `Array(Pointer(Int), 10)`.
    fn parse_declarator(&mut self, mut ty: CType) -> PResult<Declarator> {
        while self.match_kind(TokenKind::Star) {
            ty = CType::Pointer(Box::new(ty));
        }

        let (name, name_span) = self.expect_identifier("in declarator")?;

        // Array suffixes, e.g. `int a[10][20];`
        while self.match_kind(TokenKind::LBracket) {
            let size = if self.check(TokenKind::IntegerLiteral) {
                self.advance().lexeme.parse::<usize>().ok()
            } else {
                None
            };
            self.expect(TokenKind::RBracket, "after array size")?;
            ty = CType::Array(Box::new(ty), size);
        }

        Ok(Declarator {
            name,
            name_span,
            ty,
        })
    }

    /// Parses a parameter list, with `(` consumed and stopping before `)`.
    fn parse_param_list(&mut self) -> PResult<Vec<ParamDecl>> {
        let mut params = Vec::new();
        if self.check(TokenKind::RParen) {
            return Ok(params);
        }

        loop {
            let start = self.current().span;
            let base_ty = self.parse_type_specifier()?;
            // A prototype may omit the parameter name: `int f(int);`
            let (name, ty) = if self.check(TokenKind::Identifier) || self.check(TokenKind::Star) {
                let declarator = self.parse_declarator(base_ty)?;
                (Some(declarator.name), declarator.ty)
            } else {
                (None, base_ty)
            };

            params.push(ParamDecl {
                ty,
                name,
                id: self.allocate_id(),
                span: start.to(self.prev_span()),
            });

            if !self.match_kind(TokenKind::Comma) {
                return Ok(params);
            }
        }
    }

    // ### Statements ###

    fn parse_stmt(&mut self) -> PResult<Stmt> {
        // Where the statement begins; every node's span runs from here to the
        // last token the statement consumes.
        let start = self.current().span;

        // A lone `;` is the empty statement: legal wherever a statement is, and
        // the idiomatic body of a loop that does all its work in the header.
        if self.match_kind(TokenKind::Semicolon) {
            return Ok(self.mk_stmt(StmtKind::Empty, start));
        }
        if self.match_kind(TokenKind::LBrace) {
            return self.parse_block(start);
        }
        if self.match_kind(TokenKind::If) {
            return self.parse_if_stmt(start);
        }
        if self.match_kind(TokenKind::While) {
            return self.parse_while_stmt(start);
        }
        if self.match_kind(TokenKind::For) {
            return self.parse_for_stmt(start);
        }
        if self.match_kind(TokenKind::Return) {
            return self.parse_return_stmt(start);
        }

        let expr = self.parse_expr()?;
        self.expect(TokenKind::Semicolon, "after expression")?;
        Ok(self.mk_stmt(StmtKind::Expr(expr), start))
    }

    /// Parses a block's items, with `{` consumed and up to and including `}`.
    ///
    /// Item-level errors are recovered from here rather than propagated, so a
    /// mistake in one statement does not hide the rest of the function.
    fn parse_block(&mut self, start: Span) -> PResult<Stmt> {
        let mut items = Vec::new();
        while !self.check(TokenKind::RBrace) && !self.check(TokenKind::Eof) {
            let item = if self.at_declaration() {
                self.parse_decl(DeclContext::Block).map(BlockItem::Decl)
            } else {
                self.parse_stmt().map(BlockItem::Stmt)
            };

            match item {
                Ok(item) => items.push(item),
                Err(error) => self.recover_from(error),
            }
        }
        self.expect(TokenKind::RBrace, "to close this block")?;
        Ok(self.mk_stmt(StmtKind::Block(items), start))
    }

    fn parse_if_stmt(&mut self, start: Span) -> PResult<Stmt> {
        self.expect(TokenKind::LParen, "after `if`")?;
        let condition = self.parse_expr()?;
        self.expect(TokenKind::RParen, "after the `if` condition")?;

        let then_branch = Box::new(self.parse_stmt()?);
        let else_branch = if self.match_kind(TokenKind::Else) {
            Some(Box::new(self.parse_stmt()?))
        } else {
            None
        };

        Ok(self.mk_stmt(
            StmtKind::If {
                condition,
                then_branch,
                else_branch,
            },
            start,
        ))
    }

    fn parse_while_stmt(&mut self, start: Span) -> PResult<Stmt> {
        self.expect(TokenKind::LParen, "after `while`")?;
        let condition = self.parse_expr()?;
        self.expect(TokenKind::RParen, "after the `while` condition")?;

        let body = Box::new(self.parse_stmt()?);
        Ok(self.mk_stmt(StmtKind::While { condition, body }, start))
    }

    fn parse_for_stmt(&mut self, start: Span) -> PResult<Stmt> {
        self.expect(TokenKind::LParen, "after `for`")?;

        let init = self.parse_for_init()?;

        let condition = if self.match_kind(TokenKind::Semicolon) {
            None
        } else {
            let condition = self.parse_expr()?;
            self.expect(TokenKind::Semicolon, "after the `for` condition")?;
            Some(condition)
        };

        let step = if self.check(TokenKind::RParen) {
            None
        } else {
            Some(self.parse_expr()?)
        };
        self.expect(TokenKind::RParen, "after the `for` clauses")?;

        let body = Box::new(self.parse_stmt()?);
        Ok(self.mk_stmt(
            StmtKind::For {
                init,
                condition,
                step,
                body,
            },
            start,
        ))
    }

    /// Parses the first clause of a `for`, which may be empty, a declaration or
    /// an expression.
    fn parse_for_init(&mut self) -> PResult<Option<Box<Stmt>>> {
        let start = self.current().span;
        if self.match_kind(TokenKind::Semicolon) {
            return Ok(None);
        }

        let init = if self.at_declaration() {
            // A declaration is wrapped in a block so that the loop variable is
            // scoped to the loop, matching `for (int i = 0; ...)`.
            let decl = self.parse_decl(DeclContext::Block)?;
            self.mk_stmt(StmtKind::Block(vec![BlockItem::Decl(decl)]), start)
        } else {
            let expr = self.parse_expr()?;
            self.expect(TokenKind::Semicolon, "after the `for` init clause")?;
            self.mk_stmt(StmtKind::Expr(expr), start)
        };

        Ok(Some(Box::new(init)))
    }

    fn parse_return_stmt(&mut self, start: Span) -> PResult<Stmt> {
        let expr = if self.check(TokenKind::Semicolon) {
            None
        } else {
            Some(self.parse_expr()?)
        };
        self.expect(TokenKind::Semicolon, "after the returned value")?;
        Ok(self.mk_stmt(StmtKind::Return(expr), start))
    }

    // ### Expressions ###

    fn parse_expr(&mut self) -> PResult<Expr> {
        self.parse_assignment()
    }

    /// Parses `lhs = rhs`. Assignment is right-associative, which falls out of
    /// parsing the right-hand side with this same function.
    fn parse_assignment(&mut self) -> PResult<Expr> {
        let target = self.parse_binary_expr(Self::LOWEST_PRECEDENCE)?;
        if !self.match_kind(TokenKind::Eq) {
            return Ok(target);
        }

        let value = self.parse_assignment()?;
        let span = target.span;
        Ok(self.mk_expr(
            ExprKind::Binary(BinaryOp::Assign, Box::new(target), Box::new(value)),
            span,
        ))
    }

    /// Precedence climbing: parses operators binding at least as tightly as
    /// `min_precedence`.
    ///
    /// All binary operators in the supported subset are left-associative, so the
    /// right-hand side is parsed one precedence level up.
    fn parse_binary_expr(&mut self, min_precedence: u8) -> PResult<Expr> {
        let mut lhs = self.parse_unary()?;

        while let Some((op, precedence)) = Self::binary_op(self.current().kind) {
            if precedence < min_precedence {
                break;
            }
            self.advance();

            let rhs = self.parse_binary_expr(precedence + 1)?;
            let span = lhs.span;
            lhs = self.mk_expr(ExprKind::Binary(op, Box::new(lhs), Box::new(rhs)), span);
        }

        Ok(lhs)
    }

    fn parse_unary(&mut self) -> PResult<Expr> {
        let span = self.current().span;

        if let Some(op) = self.match_operator(&PREFIX_OPERATORS) {
            let operand = self.parse_unary()?;
            return Ok(self.mk_expr(ExprKind::Unary(op, Box::new(operand)), span));
        }

        if self.match_kind(TokenKind::Sizeof) {
            self.expect(TokenKind::LParen, "after `sizeof`")?;
            let operand = self.parse_expr()?;
            self.expect(TokenKind::RParen, "after the `sizeof` operand")?;
            return Ok(self.mk_expr(ExprKind::SizeOf(Box::new(operand)), span));
        }

        if self.at_cast() {
            self.expect(TokenKind::LParen, "to open the cast")?;
            let ty = self.parse_type_specifier()?;
            self.expect(TokenKind::RParen, "to close the cast")?;
            let operand = self.parse_unary()?;
            return Ok(self.mk_expr(ExprKind::Cast(ty, Box::new(operand)), span));
        }

        self.parse_postfix()
    }

    /// Parses calls, subscripts, member accesses and postfix `++`/`--`, which
    /// all bind tighter than any prefix operator and may be chained.
    fn parse_postfix(&mut self) -> PResult<Expr> {
        let mut expr = self.parse_primary()?;

        loop {
            let span = expr.span;

            if self.match_kind(TokenKind::LParen) {
                let args = self.parse_call_args()?;
                expr = self.mk_expr(
                    ExprKind::Call {
                        callee: Box::new(expr),
                        args,
                    },
                    span,
                );
                continue;
            }

            if self.match_kind(TokenKind::LBracket) {
                let index = self.parse_expr()?;
                self.expect(TokenKind::RBracket, "after the subscript")?;
                expr = self.mk_expr(
                    ExprKind::Index {
                        array: Box::new(expr),
                        index: Box::new(index),
                    },
                    span,
                );
                continue;
            }

            if self.check(TokenKind::Dot) || self.check(TokenKind::Arrow) {
                let is_arrow = self.advance().kind == TokenKind::Arrow;
                let member = self.expect_identifier("as the member name")?.0;
                expr = self.mk_expr(
                    ExprKind::MemberAccess {
                        base: Box::new(expr),
                        member,
                        is_arrow,
                    },
                    span,
                );
                continue;
            }

            let Some(op) = self.match_operator(&POSTFIX_OPERATORS) else {
                return Ok(expr);
            };
            expr = self.mk_expr(ExprKind::Unary(op, Box::new(expr)), span);
        }
    }

    /// Parses a call's arguments, with `(` consumed and up to and including `)`.
    fn parse_call_args(&mut self) -> PResult<Vec<Expr>> {
        let mut args = Vec::new();
        if !self.check(TokenKind::RParen) {
            loop {
                args.push(self.parse_expr()?);
                if !self.match_kind(TokenKind::Comma) {
                    break;
                }
            }
        }
        self.expect(TokenKind::RParen, "after the arguments")?;
        Ok(args)
    }

    fn parse_primary(&mut self) -> PResult<Expr> {
        let span = self.current().span;

        let kind = match self.current().kind {
            TokenKind::Identifier => ExprKind::Identifier(self.advance().lexeme.to_string()),
            // A literal that does not fit its type is clamped to zero for now;
            // the lexer has already accepted its shape.
            TokenKind::IntegerLiteral => {
                let value = self.advance().lexeme.parse::<i64>().unwrap_or(0);
                ExprKind::Literal(Literal::Int(value))
            }
            TokenKind::FloatLiteral => {
                let value = self.advance().lexeme.parse::<f64>().unwrap_or(0.0);
                ExprKind::Literal(Literal::Float(value))
            }
            TokenKind::StringLiteral => {
                let text = self.advance().lexeme.trim_matches('"').to_string();
                ExprKind::Literal(Literal::String(text))
            }
            TokenKind::CharLiteral => {
                // The lexeme still carries its quotes: `'c'`.
                let lexeme = self.advance().lexeme.as_bytes();
                let value = if lexeme.len() >= 3 { lexeme[1] } else { 0 };
                ExprKind::Literal(Literal::Char(value))
            }
            TokenKind::LParen => {
                self.advance();
                let inner = self.parse_expr()?;
                self.expect(TokenKind::RParen, "to close the group")?;
                // A parenthesised expression is its own node; the parentheses
                // only guided precedence.
                return Ok(inner);
            }
            _ => return self.err_here("an expression"),
        };

        Ok(self.mk_expr(kind, span))
    }

    /// The precedence of the loosest-binding binary operator.
    const LOWEST_PRECEDENCE: u8 = 1;

    /// Maps a token to the binary operator it denotes and how tightly it binds.
    ///
    /// Higher numbers bind tighter, mirroring the C grammar: `a || b && c`
    /// groups as `a || (b && c)`.
    fn binary_op(kind: TokenKind) -> Option<(BinaryOp, u8)> {
        Some(match kind {
            TokenKind::PipePipe => (BinaryOp::LogicalOr, 1),
            TokenKind::AmpAmp => (BinaryOp::LogicalAnd, 2),
            TokenKind::Pipe => (BinaryOp::BitOr, 3),
            TokenKind::Caret => (BinaryOp::BitXor, 4),
            TokenKind::Ampersand => (BinaryOp::BitAnd, 5),
            TokenKind::EqEq => (BinaryOp::Eq, 6),
            TokenKind::BangEq => (BinaryOp::Neq, 6),
            TokenKind::Less => (BinaryOp::Lt, 7),
            TokenKind::LessEq => (BinaryOp::Lte, 7),
            TokenKind::Greater => (BinaryOp::Gt, 7),
            TokenKind::GreaterEq => (BinaryOp::Gte, 7),
            TokenKind::Shl => (BinaryOp::Shl, 8),
            TokenKind::Shr => (BinaryOp::Shr, 8),
            TokenKind::Plus => (BinaryOp::Add, 9),
            TokenKind::Minus => (BinaryOp::Sub, 9),
            TokenKind::Star => (BinaryOp::Mul, 10),
            TokenKind::Slash => (BinaryOp::Div, 10),
            TokenKind::Percent => (BinaryOp::Mod, 10),
            _ => return None,
        })
    }

    // ### Lookahead predicates ###

    /// Whether `kind` can begin a type specifier.
    fn starts_type(kind: TokenKind) -> bool {
        matches!(
            kind,
            TokenKind::Int
                | TokenKind::Long
                | TokenKind::Char
                | TokenKind::Float
                | TokenKind::Double
                | TokenKind::Void
                | TokenKind::Struct
        )
    }

    /// Whether the cursor sits on a declaration rather than a statement.
    fn at_declaration(&self) -> bool {
        Self::starts_type(self.current().kind)
    }

    /// Whether the cursor sits on a cast such as `(int)x`, as opposed to a
    /// parenthesised expression.
    ///
    /// A few tokens of lookahead are enough for the supported type syntax: `(`
    /// followed by a type specifier, followed by either `)` or the start of the
    /// declarator being cast to. Every specifier is one token except `long
    /// int`, which is two.
    fn at_cast(&self) -> bool {
        if !self.check(TokenKind::LParen) || !self.peek_kind(1).is_some_and(Self::starts_type) {
            return false;
        }
        let after_specifier = match (self.peek_kind(1), self.peek_kind(2)) {
            (Some(TokenKind::Long), Some(TokenKind::Int)) => self.peek_kind(3),
            _ => self.peek_kind(2),
        };
        matches!(
            after_specifier,
            Some(TokenKind::RParen | TokenKind::Identifier)
        )
    }

    // ### Token stream helpers ###

    /// Whether the current token is of the given kind.
    fn check(&self, kind: TokenKind) -> bool {
        self.current().kind == kind
    }

    /// Consumes the current token if it is of the given kind.
    fn match_kind(&mut self, kind: TokenKind) -> bool {
        let matched = self.check(kind);
        if matched {
            self.advance();
        }
        matched
    }

    /// Consumes the current token if it introduces one of `operators`, and
    /// returns the operator it denotes.
    fn match_operator(&mut self, operators: &[(TokenKind, UnaryOp)]) -> Option<UnaryOp> {
        let &(_, op) = operators.iter().find(|(token, _)| self.check(*token))?;
        self.advance();
        Some(op)
    }

    /// Consumes the current token, requiring it to be of the given kind.
    ///
    /// # Arguments
    ///
    /// * `kind` - the token the grammar requires here
    /// * `context` - where in the construct it belongs, e.g. `after
    ///   declaration`, used as the caption under the underline
    ///
    /// # Errors
    ///
    /// Returns a [`ParseError`] blaming the token that was found instead --
    /// except when the missing token is a `;` or the input has run out, where
    /// the gap after the last token is blamed instead. Pointing at the next
    /// token there reads as if the following line were at fault, and at the end
    /// of input there is no token to point at at all.
    fn expect(&mut self, kind: TokenKind, context: &str) -> PResult<()> {
        if self.match_kind(kind) {
            return Ok(());
        }

        let span = match (kind, self.current().kind) {
            (TokenKind::Semicolon, _) | (_, TokenKind::Eof) => self.prev_span().after(),
            _ => self.current().span,
        };
        Err(ParseError::expected(
            &kind.to_string(),
            context,
            self.current(),
            span,
        ))
    }

    /// Consumes an identifier and returns its name and location.
    fn expect_identifier(&mut self, context: &str) -> PResult<(String, Span)> {
        if !self.check(TokenKind::Identifier) {
            return self.err_here_with("identifier", context);
        }
        let token = self.advance();
        Ok((token.lexeme.to_string(), token.span))
    }

    /// Consumes the current token and returns it.
    fn advance(&mut self) -> &Token<'a> {
        if self.pos < self.tokens.len() {
            self.pos += 1;
        }
        self.prev()
    }

    /// The token under the cursor, or the trailing `Eof` past the end.
    fn current(&self) -> &Token<'a> {
        self.tokens
            .get(self.pos)
            .unwrap_or_else(|| self.tokens.last().expect("token stream ends with Eof"))
    }

    /// The most recently consumed token.
    fn prev(&self) -> &Token<'a> {
        &self.tokens[self.pos.saturating_sub(1)]
    }

    /// Span of the most recently consumed token, which is where a completed
    /// statement or declaration is reported.
    fn prev_span(&self) -> Span {
        self.prev().span
    }

    /// The kind of the token `n` positions ahead, if there is one.
    fn peek_kind(&self, n: usize) -> Option<TokenKind> {
        self.tokens.get(self.pos + n).map(|token| token.kind)
    }

    /// Rejects the current token, which was not what the grammar allows here.
    fn err_here<T>(&self, expectation: &str) -> PResult<T> {
        self.err_here_with(expectation, "")
    }

    /// [`Parser::err_here`], with a phrase saying where in the construct the
    /// expectation comes from.
    fn err_here_with<T>(&self, expectation: &str, context: &str) -> PResult<T> {
        Err(ParseError::expected(
            expectation,
            context,
            self.current(),
            self.current().span,
        ))
    }

    // ### Node construction ###

    fn allocate_id(&mut self) -> NodeId {
        let id = self.next_node_id;
        self.next_node_id += 1;
        id
    }

    /// Builds an expression node covering `start` up to the last token consumed.
    fn mk_expr(&mut self, kind: ExprKind, start: Span) -> Expr {
        Expr {
            id: self.allocate_id(),
            kind,
            span: start.to(self.prev_span()),
        }
    }

    /// Builds a statement node covering `start` up to the last token consumed.
    fn mk_stmt(&mut self, kind: StmtKind, start: Span) -> Stmt {
        Stmt {
            id: self.allocate_id(),
            kind,
            span: start.to(self.prev_span()),
        }
    }

    /// Builds a declaration node.
    ///
    /// # Arguments
    ///
    /// * `kind` - what is being declared
    /// * `span` - the declaration as written; see [`Decl::span`]
    /// * `name_span` - the declared identifier alone
    fn mk_decl(&mut self, kind: DeclKind, span: Span, name_span: Span) -> Decl {
        Decl {
            id: self.allocate_id(),
            kind,
            span,
            name_span,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parses `src`, failing the test if any syntax error is reported.
    fn parse_ok(src: &str) -> Vec<Decl> {
        let (decls, errors) = parse(src);
        assert!(
            errors.is_empty(),
            "parse failed with {} error(s): {} at {:?}",
            errors.len(),
            errors[0].message,
            errors[0].span
        );
        decls
    }

    /// Parses `src`, returning the declarations and the errors found.
    fn parse(src: &str) -> (Vec<Decl>, Vec<ParseError>) {
        let mut parser = Parser::from_lexer(Lexer::new(src)).expect("lexing should succeed");
        parser.parse_translation_unit()
    }

    /// Returns the items of the single function definition in `src`.
    fn parse_function_body(src: &str) -> Vec<BlockItem> {
        let tu = parse_ok(src);
        let body = match &tu[0].kind {
            DeclKind::Function { body: Some(b), .. } => b,
            other => panic!("expected function with body, got {other:?}"),
        };
        match &body.kind {
            StmtKind::Block(items) => items.clone(),
            other => panic!("expected block body, got {other:?}"),
        }
    }

    /// Returns the first expression statement in a list of block items.
    fn first_expr(items: &[BlockItem]) -> &Expr {
        items
            .iter()
            .find_map(|item| match item {
                BlockItem::Stmt(Stmt {
                    kind: StmtKind::Expr(expr),
                    ..
                }) => Some(expr),
                _ => None,
            })
            .expect("expected an expression statement")
    }

    #[test]
    fn parses_variable_decl_without_initializer() {
        let tu = parse_ok("int x;");
        assert_eq!(tu.len(), 1);

        match &tu[0].kind {
            DeclKind::Variable {
                ty,
                name,
                initializer,
            } => {
                assert_eq!(*ty, CType::Int);
                assert_eq!(name, "x");
                assert!(initializer.is_none());
            }
            other => panic!("expected variable decl, got {other:?}"),
        }
    }

    #[test]
    fn parses_variable_decl_with_initializer() {
        let tu = parse_ok("int x = 42;");
        match &tu[0].kind {
            DeclKind::Variable {
                ty,
                name,
                initializer,
            } => {
                assert_eq!(*ty, CType::Int);
                assert_eq!(name, "x");
                let init = initializer.as_ref().expect("missing initializer");
                assert!(matches!(init.kind, ExprKind::Literal(Literal::Int(42))));
            }
            _ => panic!("expected variable declaration"),
        }
    }

    #[test]
    fn parses_pointer_and_array_declarator() {
        let tu = parse_ok("int *p[10];");
        match &tu[0].kind {
            DeclKind::Variable { ty, name, .. } => {
                assert_eq!(name, "p");
                assert_eq!(
                    *ty,
                    CType::Array(Box::new(CType::Pointer(Box::new(CType::Int))), Some(10))
                );
            }
            _ => panic!("expected variable declaration"),
        }
    }

    #[test]
    fn parses_function_prototype() {
        let tu = parse_ok("int add(int a, int b);");
        match &tu[0].kind {
            DeclKind::Function {
                return_ty,
                name,
                params,
                body,
            } => {
                assert_eq!(*return_ty, CType::Int);
                assert_eq!(name, "add");
                assert_eq!(params.len(), 2);
                assert!(body.is_none());
                assert_eq!(params[0].ty, CType::Int);
                assert_eq!(params[0].name.as_deref(), Some("a"));
                assert_eq!(params[1].ty, CType::Int);
                assert_eq!(params[1].name.as_deref(), Some("b"));
            }
            _ => panic!("expected function declaration"),
        }
    }

    #[test]
    fn parses_unnamed_prototype_parameter() {
        let tu = parse_ok("int f(int);");
        match &tu[0].kind {
            DeclKind::Function { params, .. } => {
                assert_eq!(params.len(), 1);
                assert_eq!(params[0].ty, CType::Int);
                assert!(params[0].name.is_none());
            }
            _ => panic!("expected function declaration"),
        }
    }

    #[test]
    fn parses_function_definition_with_return() {
        let items = parse_function_body("int add(int a, int b) { return a + b; }");
        assert_eq!(items.len(), 1);

        match &items[0] {
            BlockItem::Stmt(Stmt {
                kind: StmtKind::Return(Some(expr)),
                ..
            }) => assert!(matches!(expr.kind, ExprKind::Binary(BinaryOp::Add, _, _))),
            other => panic!("unexpected block item: {other:?}"),
        }
    }

    #[test]
    fn parses_operator_precedence_mul_before_add() {
        let tu = parse_ok("int x = 1 + 2 * 3;");
        let init = match &tu[0].kind {
            DeclKind::Variable {
                initializer: Some(e),
                ..
            } => e,
            _ => panic!("expected initialized variable"),
        };

        match &init.kind {
            ExprKind::Binary(BinaryOp::Add, _lhs, rhs) => {
                assert!(matches!(rhs.kind, ExprKind::Binary(BinaryOp::Mul, _, _)));
            }
            other => panic!("expected add at root, got {other:?}"),
        }
    }

    #[test]
    fn parses_assignment_right_associative() {
        let items = parse_function_body("int f(){ int a; int b; int c; a = b = c; return a; }");

        match &first_expr(&items).kind {
            ExprKind::Binary(BinaryOp::Assign, _, rhs) => {
                assert!(matches!(rhs.kind, ExprKind::Binary(BinaryOp::Assign, _, _)));
            }
            _ => panic!("expected assignment expression"),
        }
    }

    #[test]
    fn parses_if_else_statement() {
        let items = parse_function_body("int f(int x){ if (x) return 1; else return 0; }");

        match &items[0] {
            BlockItem::Stmt(Stmt {
                kind: StmtKind::If { else_branch, .. },
                ..
            }) => assert!(else_branch.is_some()),
            _ => panic!("expected if statement"),
        }
    }

    #[test]
    fn parses_while_statement() {
        let items = parse_function_body("int f(){ int x; while (x) x = x - 1; return x; }");

        assert!(items.iter().any(|item| matches!(
            item,
            BlockItem::Stmt(Stmt {
                kind: StmtKind::While { .. },
                ..
            })
        )));
    }

    #[test]
    fn parses_for_statement() {
        let items = parse_function_body("int f(){ int i; for (i = 0; i < 10; i++) { } return i; }");

        assert!(items.iter().any(|item| matches!(
            item,
            BlockItem::Stmt(Stmt {
                kind: StmtKind::For { .. },
                ..
            })
        )));
    }

    #[test]
    fn scopes_for_loop_declaration_to_the_loop() {
        let items = parse_function_body("int f(){ for (int i = 0; i < 10; i++) { } return 0; }");

        let init = match &items[0] {
            BlockItem::Stmt(Stmt {
                kind: StmtKind::For { init: Some(i), .. },
                ..
            }) => i,
            other => panic!("expected for statement, got {other:?}"),
        };

        // The declaration is wrapped in its own block, which is what scopes it.
        assert!(matches!(init.kind, StmtKind::Block(_)));
    }

    #[test]
    fn parses_empty_statement() {
        let items = parse_function_body("int f(){ ; return 0; }");

        assert!(matches!(
            items[0],
            BlockItem::Stmt(Stmt {
                kind: StmtKind::Empty,
                ..
            })
        ));
    }

    #[test]
    fn parses_loop_with_an_empty_body() {
        let items = parse_function_body("int f(){ for (int i = 0; i < 3; i = i + 1) ; return 0; }");

        let body = match &items[0] {
            BlockItem::Stmt(Stmt {
                kind: StmtKind::For { body, .. },
                ..
            }) => body,
            other => panic!("expected for statement, got {other:?}"),
        };
        assert!(matches!(body.kind, StmtKind::Empty));
    }

    #[test]
    fn parses_call_index_member_and_postfix_inc() {
        let items = parse_function_body("int f(){ arr[i].x++; return 0; }");

        assert!(matches!(
            first_expr(&items).kind,
            ExprKind::Unary(UnaryOp::PostInc, _)
        ));
    }

    #[test]
    fn parses_struct_definition() {
        let tu = parse_ok("struct Point { int x; int y; };");
        match &tu[0].kind {
            DeclKind::Struct { name, members } => {
                assert_eq!(name.as_deref(), Some("Point"));
                assert_eq!(members.len(), 2);
            }
            _ => panic!("expected struct declaration"),
        }
    }

    #[test]
    fn parses_struct_forward_declaration() {
        let tu = parse_ok("struct Point;");
        match &tu[0].kind {
            DeclKind::Struct { name, members } => {
                assert_eq!(name.as_deref(), Some("Point"));
                assert!(members.is_empty());
            }
            other => panic!("expected struct declaration, got {other:?}"),
        }
    }

    #[test]
    fn parses_variable_of_struct_type() {
        let tu = parse_ok("struct Point origin;");
        match &tu[0].kind {
            DeclKind::Variable { ty, name, .. } => {
                assert_eq!(*ty, CType::Struct("Point".to_string()));
                assert_eq!(name, "origin");
            }
            other => panic!("expected variable declaration, got {other:?}"),
        }
    }

    #[test]
    fn rejects_function_definition_inside_a_block() {
        // C has no nested functions, and the rest of the pipeline relies on
        // every block-level declaration being a variable.
        let (_decls, errors) = parse("int main(){ int nested(int a); return 0; }");
        assert!(!errors.is_empty(), "nested function should be rejected");
    }

    #[test]
    fn rejects_invalid_input() {
        let (_decls, errors) = parse("int x = ;");
        assert!(!errors.is_empty(), "should produce at least one error");
        assert!(!errors[0].message.is_empty());
    }

    #[test]
    fn reports_multiple_errors() {
        // Two bad declarations: missing initializer expressions.
        let (_decls, errors) = parse("int x = ; int y = ;");
        assert!(
            errors.len() >= 2,
            "expected at least 2 errors, got {}",
            errors.len()
        );
    }

    #[test]
    fn recovers_valid_decls_after_error() {
        // First decl is broken, second is valid.
        let (decls, errors) = parse("int x = ; int y = 5;");
        assert!(!errors.is_empty(), "should have parse errors");
        assert!(
            !decls.is_empty(),
            "should recover and parse valid declarations"
        );
    }

    #[test]
    fn reports_lex_errors_before_parsing() {
        let Err(error) = Parser::from_lexer(Lexer::new("int s = \"unterminated;")) else {
            panic!("unterminated string should be rejected");
        };
        assert!(error.message.contains("unterminated string"));
    }
}
