use crate::driver::diagnostics::CompilerError;
use crate::frontend::ast::*;
use crate::frontend::lexer::{Lexer, Span, Token, TokenKind};

#[derive(Debug, Clone, PartialEq)]
pub struct ParseError {
    pub message: String,
    pub span: Span,
}

impl CompilerError for ParseError {
    fn get_span(&self) -> Span {
        self.span
    }

    fn get_message(&self) -> String {
        self.message.clone()
    }

    fn error_prefix(&self) -> String {
        String::from("Syntax Error")
    }
}

type PResult<T> = Result<T, ParseError>;

pub struct Parser<'a> {
    pub tokens: Vec<Token<'a>>,
    pub pos: usize,
    pub next_node_id: u32,
    /// Accumulated parse errors (for multi-error reporting).
    pub errors: Vec<ParseError>,
}

impl<'a> Parser<'a> {
    pub fn from_lexer(mut lexer: Lexer<'a>) -> PResult<Self> {
        let mut tokens = Vec::new();
        loop {
            let tok = lexer.next_token();
            if let TokenKind::Error(e) = tok.kind {
                return Err(ParseError {
                    message: format!("lex error: {:?}", e),
                    span: tok.span,
                });
            }
            let is_eof = tok.kind == TokenKind::Eof;
            tokens.push(tok);
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
            match self.parse_external_decl() {
                Ok(d) => decls.push(d),
                Err(e) => {
                    self.errors.push(e);
                    self.synchronize();
                }
            }
        }
        let errors = std::mem::take(&mut self.errors);
        (decls, errors)
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
            if self.prev().kind == TokenKind::Semicolon || self.prev().kind == TokenKind::RBrace {
                return;
            }

            // If the current token starts a new declaration or
            // statement, stop before consuming it.
            match self.current().kind {
                TokenKind::Int
                | TokenKind::Char
                | TokenKind::Float
                | TokenKind::Double
                | TokenKind::Void
                | TokenKind::Struct
                | TokenKind::If
                | TokenKind::While
                | TokenKind::For
                | TokenKind::Return => return,
                // Also stop before a closing brace (end of enclosing
                // block) so the caller can consume it.
                TokenKind::RBrace => return,
                _ => {
                    self.advance();
                }
            }
        }
    }

    fn parse_external_decl(&mut self) -> PResult<Decl> {
        if self.match_kind(TokenKind::Struct) {
            return self.parse_struct_decl_after_struct_kw();
        }

        let base_ty = self.parse_type_specifier()?;
        let (name, ty) = self.parse_declarator(base_ty.clone())?;

        if self.match_kind(TokenKind::LParen) {
            // function
            let params = self.parse_param_list()?;
            self.expect(TokenKind::RParen, "expected ')' after parameter list")?;

            let body = if self.match_kind(TokenKind::LBrace) {
                Some(self.parse_block_stmt_from_open_brace()?)
            } else {
                self.expect(
                    TokenKind::Semicolon,
                    "expected ';' after function prototype",
                )?;
                None
            };

            Ok(Decl {
                kind: DeclKind::Function {
                    return_ty: ty,
                    name,
                    params,
                    body,
                },
                span: self.prev_span(),
                id: self.allocate_id(),
            })
        } else {
            // variable
            let initializer = if self.match_kind(TokenKind::Eq) {
                Some(self.parse_expr()?)
            } else {
                None
            };
            self.expect(
                TokenKind::Semicolon,
                "expected ';' after variable declaration",
            )?;
            Ok(Decl {
                kind: DeclKind::Variable {
                    ty,
                    name,
                    initializer,
                },
                span: self.prev_span(),
                id: self.allocate_id(),
            })
        }
    }

    fn parse_struct_decl_after_struct_kw(&mut self) -> PResult<Decl> {
        let name = if self.check(TokenKind::Identifier) {
            Some(self.advance().lexeme.to_string())
        } else {
            None
        };

        let mut members = Vec::new();
        if self.match_kind(TokenKind::LBrace) {
            while !self.check(TokenKind::RBrace) && !self.check(TokenKind::Eof) {
                let member_base = self.parse_type_specifier()?;
                let (mname, mty) = self.parse_declarator(member_base)?;
                self.expect(TokenKind::Semicolon, "expected ';' after struct member")?;
                members.push(Decl {
                    kind: DeclKind::Variable {
                        ty: mty,
                        name: mname,
                        initializer: None,
                    },
                    span: self.prev_span(),
                    id: self.allocate_id(),
                });
            }
            self.expect(TokenKind::RBrace, "expected '}' after struct body")?;
        }
        self.expect(
            TokenKind::Semicolon,
            "expected ';' after struct declaration",
        )?;

        Ok(Decl {
            kind: DeclKind::Struct { name, members },
            span: self.prev_span(),
            id: self.allocate_id(),
        })
    }

    fn parse_type_specifier(&mut self) -> PResult<CType> {
        if self.match_kind(TokenKind::Int) {
            Ok(CType::Int)
        } else if self.match_kind(TokenKind::Char) {
            Ok(CType::Char)
        } else if self.match_kind(TokenKind::Float) {
            Ok(CType::Float)
        } else if self.match_kind(TokenKind::Double) {
            Ok(CType::Double)
        } else if self.match_kind(TokenKind::Void) {
            Ok(CType::Void)
        } else if self.match_kind(TokenKind::Struct) {
            let name = if self.check(TokenKind::Identifier) {
                self.advance().lexeme.to_string()
            } else {
                return self.err_here("expected struct name");
            };
            Ok(CType::Struct(name))
        } else {
            self.err_here("expected type specifier")
        }
    }

    fn parse_declarator(&mut self, mut ty: CType) -> PResult<(String, CType)> {
        while self.match_kind(TokenKind::Star) {
            ty = CType::Pointer(Box::new(ty));
        }

        let name = self
            .expect(TokenKind::Identifier, "expected identifier in declarator")?
            .lexeme
            .to_string();

        // array suffixes: int a[10][20];
        while self.match_kind(TokenKind::LBracket) {
            let size = if self.check(TokenKind::IntegerLiteral) {
                let n = self.advance().lexeme.parse::<usize>().ok();
                n
            } else {
                None
            };
            self.expect(TokenKind::RBracket, "expected ']'")?;
            ty = CType::Array(Box::new(ty), size);
        }

        Ok((name, ty))
    }

    fn parse_param_list(&mut self) -> PResult<Vec<ParamDecl>> {
        let mut params = Vec::new();
        if self.check(TokenKind::RParen) {
            return Ok(params);
        }

        loop {
            let base = self.parse_type_specifier()?;
            let (name, ty) = if self.check(TokenKind::Identifier) || self.check(TokenKind::Star) {
                self.parse_declarator(base)?
            } else {
                (String::new(), base)
            };

            params.push(ParamDecl {
                ty,
                name: if name.is_empty() { None } else { Some(name) },
                id: self.allocate_id(),
            });

            if !self.match_kind(TokenKind::Comma) {
                break;
            }
        }
        Ok(params)
    }

    fn parse_stmt(&mut self) -> PResult<Stmt> {
        if self.match_kind(TokenKind::LBrace) {
            return self.parse_block_stmt_from_open_brace();
        }
        if self.match_kind(TokenKind::If) {
            return self.parse_if_stmt();
        }
        if self.match_kind(TokenKind::While) {
            return self.parse_while_stmt();
        }
        if self.match_kind(TokenKind::For) {
            return self.parse_for_stmt();
        }
        if self.match_kind(TokenKind::Return) {
            return self.parse_return_stmt();
        }

        // expression statement
        let expr = self.parse_expr()?;
        self.expect(TokenKind::Semicolon, "expected ';' after expression")?;
        Ok(Stmt {
            kind: StmtKind::Expr(expr),
            span: self.prev_span(),
            id: self.allocate_id(),
        })
    }

    fn parse_block_stmt_from_open_brace(&mut self) -> PResult<Stmt> {
        let mut items = Vec::new();
        while !self.check(TokenKind::RBrace) && !self.check(TokenKind::Eof) {
            let result = if self.is_type_start() || self.check(TokenKind::Struct) {
                self.parse_block_decl().map(BlockItem::Decl)
            } else {
                self.parse_stmt().map(BlockItem::Stmt)
            };

            match result {
                Ok(item) => items.push(item),
                Err(e) => {
                    self.errors.push(e);
                    self.synchronize();
                }
            }
        }
        self.expect(TokenKind::RBrace, "expected '}' to close block")?;
        Ok(Stmt {
            kind: StmtKind::Block(items),
            span: self.prev_span(),
            id: self.allocate_id(),
        })
    }

    fn parse_block_decl(&mut self) -> PResult<Decl> {
        if self.match_kind(TokenKind::Struct) {
            // struct definition in block
            if self.check(TokenKind::Identifier) {
                let name_tok = self.advance().lexeme.to_string();
                if self.match_kind(TokenKind::LBrace) {
                    let mut members = Vec::new();
                    while !self.check(TokenKind::RBrace) && !self.check(TokenKind::Eof) {
                        let mbase = self.parse_type_specifier()?;
                        let (mname, mty) = self.parse_declarator(mbase)?;
                        self.expect(TokenKind::Semicolon, "expected ';' after struct member")?;
                        members.push(Decl {
                            kind: DeclKind::Variable {
                                ty: mty,
                                name: mname,
                                initializer: None,
                            },
                            id: self.allocate_id(),
                            span: self.prev_span(),
                        });
                    }
                    self.expect(TokenKind::RBrace, "expected '}'")?;
                    self.expect(
                        TokenKind::Semicolon,
                        "expected ';' after struct declaration",
                    )?;
                    return Ok(Decl {
                        kind: DeclKind::Struct {
                            name: Some(name_tok),
                            members,
                        },
                        span: self.prev_span(),
                        id: self.allocate_id(),
                    });
                } else {
                    // struct type + var decl
                    let mut ty = CType::Struct(name_tok);
                    while self.match_kind(TokenKind::Star) {
                        ty = CType::Pointer(Box::new(ty));
                    }
                    let name = self
                        .expect(TokenKind::Identifier, "expected variable name")?
                        .lexeme
                        .to_string();
                    let initializer = if self.match_kind(TokenKind::Eq) {
                        Some(self.parse_expr()?)
                    } else {
                        None
                    };
                    self.expect(TokenKind::Semicolon, "expected ';' after declaration")?;
                    return Ok(Decl {
                        kind: DeclKind::Variable {
                            ty,
                            name,
                            initializer,
                        },
                        id: self.allocate_id(),
                        span: self.prev_span(),
                    });
                }
            } else {
                return self.err_here("expected struct name");
            }
        }

        let base = self.parse_type_specifier()?;
        let (name, ty) = self.parse_declarator(base)?;
        let initializer = if self.match_kind(TokenKind::Eq) {
            Some(self.parse_expr()?)
        } else {
            None
        };
        self.expect(TokenKind::Semicolon, "expected ';' after declaration")?;
        Ok(Decl {
            kind: DeclKind::Variable {
                ty,
                name,
                initializer,
            },
            id: self.allocate_id(),
            span: self.prev_span(),
        })
    }

    fn parse_if_stmt(&mut self) -> PResult<Stmt> {
        self.expect(TokenKind::LParen, "expected '(' after if")?;
        let condition = self.parse_expr()?;
        self.expect(TokenKind::RParen, "expected ')' after if condition")?;
        let then_branch = Box::new(self.parse_stmt()?);
        let else_branch = if self.match_kind(TokenKind::Else) {
            Some(Box::new(self.parse_stmt()?))
        } else {
            None
        };
        Ok(Stmt {
            kind: StmtKind::If {
                condition,
                then_branch,
                else_branch,
            },
            span: self.prev_span(),
            id: self.allocate_id(),
        })
    }

    fn parse_while_stmt(&mut self) -> PResult<Stmt> {
        self.expect(TokenKind::LParen, "expected '(' after while")?;
        let condition = self.parse_expr()?;
        self.expect(TokenKind::RParen, "expected ')' after while condition")?;
        let body = Box::new(self.parse_stmt()?);
        Ok(Stmt {
            kind: StmtKind::While { condition, body },
            span: self.prev_span(),
            id: self.allocate_id(),
        })
    }

    fn parse_for_stmt(&mut self) -> PResult<Stmt> {
        self.expect(TokenKind::LParen, "expected '(' after for")?;

        let init = if self.match_kind(TokenKind::Semicolon) {
            None
        } else if self.is_type_start() || self.check(TokenKind::Struct) {
            let d = self.parse_block_decl()?;
            let fake_stmt = Stmt {
                kind: StmtKind::Block(vec![BlockItem::Decl(d)]),
                span: self.prev_span(),
                id: self.allocate_id(),
            };
            Some(Box::new(fake_stmt))
        } else {
            let e = self.parse_expr()?;
            self.expect(TokenKind::Semicolon, "expected ';' after for init")?;
            Some(Box::new(Stmt {
                kind: StmtKind::Expr(e),
                span: self.prev_span(),
                id: self.allocate_id(),
            }))
        };

        let condition = if self.match_kind(TokenKind::Semicolon) {
            None
        } else {
            let c = self.parse_expr()?;
            self.expect(TokenKind::Semicolon, "expected ';' after for condition")?;
            Some(c)
        };

        let step = if self.check(TokenKind::RParen) {
            None
        } else {
            Some(self.parse_expr()?)
        };
        self.expect(TokenKind::RParen, "expected ')' after for clauses")?;

        let body = Box::new(self.parse_stmt()?);
        Ok(Stmt {
            kind: StmtKind::For {
                init,
                condition,
                step,
                body,
            },
            span: self.prev_span(),
            id: self.allocate_id(),
        })
    }

    fn parse_return_stmt(&mut self) -> PResult<Stmt> {
        let expr = if self.check(TokenKind::Semicolon) {
            None
        } else {
            Some(self.parse_expr()?)
        };
        self.expect(TokenKind::Semicolon, "expected ';' after return")?;
        Ok(Stmt {
            kind: StmtKind::Return(expr),
            span: self.prev_span(),
            id: self.allocate_id(),
        })
    }

    fn parse_expr(&mut self) -> PResult<Expr> {
        self.parse_assignment()
    }

    fn parse_assignment(&mut self) -> PResult<Expr> {
        let mut left = self.parse_binary_expr(1)?;
        if self.match_kind(TokenKind::Eq) {
            let right = self.parse_assignment()?;
            let span = left.span;
            left = Expr {
                kind: ExprKind::Binary(BinaryOp::Assign, Box::new(left), Box::new(right)),
                span,
                id: self.allocate_id(),
            };
        }
        Ok(left)
    }

    fn parse_binary_expr(&mut self, min_prec: u8) -> PResult<Expr> {
        let mut lhs = self.parse_unary()?;

        loop {
            let (op, prec) = match self.current_binary_op() {
                Some(v) => v,
                None => break,
            };

            if prec < min_prec {
                break;
            }

            self.advance(); // consume op
            let rhs = self.parse_binary_expr(prec + 1)?;
            let span = lhs.span;
            lhs = Expr {
                kind: ExprKind::Binary(op, Box::new(lhs), Box::new(rhs)),
                span,
                id: self.allocate_id(),
            };
        }

        Ok(lhs)
    }

    fn parse_unary(&mut self) -> PResult<Expr> {
        let span = self.current().span;
        if self.match_kind(TokenKind::Minus) {
            let expr = self.parse_unary()?;
            return Ok(Expr {
                kind: ExprKind::Unary(UnaryOp::Neg, Box::new(expr)),
                span,
                id: self.allocate_id(),
            });
        }
        if self.match_kind(TokenKind::Bang) {
            let expr = self.parse_unary()?;
            return Ok(Expr {
                kind: ExprKind::Unary(UnaryOp::Not, Box::new(expr)),
                span,
                id: self.allocate_id(),
            });
        }
        if self.match_kind(TokenKind::Tilde) {
            let expr = self.parse_unary()?;
            return Ok(Expr {
                kind: ExprKind::Unary(UnaryOp::BitNot, Box::new(expr)),
                span,
                id: self.allocate_id(),
            });
        }
        if self.match_kind(TokenKind::Star) {
            let expr = self.parse_unary()?;
            return Ok(Expr {
                kind: ExprKind::Unary(UnaryOp::Deref, Box::new(expr)),
                span,
                id: self.allocate_id(),
            });
        }
        if self.match_kind(TokenKind::Ampersand) {
            let expr = self.parse_unary()?;
            return Ok(Expr {
                kind: ExprKind::Unary(UnaryOp::AddressOf, Box::new(expr)),
                span,
                id: self.allocate_id(),
            });
        }
        if self.match_kind(TokenKind::PlusPlus) {
            let expr = self.parse_unary()?;
            return Ok(Expr {
                kind: ExprKind::Unary(UnaryOp::PreInc, Box::new(expr)),
                span,
                id: self.allocate_id(),
            });
        }
        if self.match_kind(TokenKind::MinusMinus) {
            let expr = self.parse_unary()?;
            return Ok(Expr {
                kind: ExprKind::Unary(UnaryOp::PreDec, Box::new(expr)),
                span,
                id: self.allocate_id(),
            });
        }
        if self.match_kind(TokenKind::Sizeof) {
            self.expect(TokenKind::LParen, "expected '(' after sizeof")?;
            let inner = self.parse_expr()?;
            self.expect(TokenKind::RParen, "expected ')' after sizeof argument")?;
            return Ok(Expr {
                kind: ExprKind::SizeOf(Box::new(inner)),
                span,
                id: self.allocate_id(),
            });
        }

        // cast: (type) unary
        if self.check(TokenKind::LParen) && self.looks_like_cast() {
            self.expect(TokenKind::LParen, "expected '('")?;
            let ty = self.parse_type_specifier()?;
            self.expect(TokenKind::RParen, "expected ')' in cast")?;
            let expr = self.parse_unary()?;
            return Ok(Expr {
                kind: ExprKind::Cast(ty, Box::new(expr)),
                span,
                id: self.allocate_id(),
            });
        }

        self.parse_postfix()
    }

    fn parse_postfix(&mut self) -> PResult<Expr> {
        let mut expr = self.parse_primary()?;

        loop {
            if self.match_kind(TokenKind::LParen) {
                let mut args = Vec::new();
                if !self.check(TokenKind::RParen) {
                    loop {
                        args.push(self.parse_expr()?);
                        if !self.match_kind(TokenKind::Comma) {
                            break;
                        }
                    }
                }
                self.expect(TokenKind::RParen, "expected ')' after arguments")?;
                let span = expr.span;
                expr = Expr {
                    kind: ExprKind::Call {
                        callee: Box::new(expr),
                        args,
                    },
                    span,
                    id: self.allocate_id(),
                };
                continue;
            }

            if self.match_kind(TokenKind::LBracket) {
                let idx = self.parse_expr()?;
                self.expect(TokenKind::RBracket, "expected ']'")?;
                let span = expr.span;
                expr = Expr {
                    kind: ExprKind::Index {
                        array: Box::new(expr),
                        index: Box::new(idx),
                    },
                    span,
                    id: self.allocate_id(),
                };
                continue;
            }

            if self.match_kind(TokenKind::Dot) || self.match_kind(TokenKind::Arrow) {
                let is_arrow = self.prev().kind == TokenKind::Arrow;
                let member = self
                    .expect(TokenKind::Identifier, "expected member name")?
                    .lexeme
                    .to_string();
                let span = expr.span;
                expr = Expr {
                    kind: ExprKind::MemberAccess {
                        base: Box::new(expr),
                        member,
                        is_arrow,
                    },
                    span,
                    id: self.allocate_id(),
                };
                continue;
            }

            if self.match_kind(TokenKind::PlusPlus) {
                let span = expr.span;
                expr = Expr {
                    kind: ExprKind::Unary(UnaryOp::PostInc, Box::new(expr)),
                    span,
                    id: self.allocate_id(),
                };
                continue;
            }

            if self.match_kind(TokenKind::MinusMinus) {
                let span = expr.span;
                expr = Expr {
                    kind: ExprKind::Unary(UnaryOp::PostDec, Box::new(expr)),
                    span,
                    id: self.allocate_id(),
                };
                continue;
            }

            break;
        }

        Ok(expr)
    }

    fn parse_primary(&mut self) -> PResult<Expr> {
        let tok = self.current().clone();
        match tok.kind {
            TokenKind::Identifier => {
                self.advance();
                Ok(Expr {
                    kind: ExprKind::Identifier(tok.lexeme.to_string()),
                    span: tok.span,
                    id: self.allocate_id(),
                })
            }
            TokenKind::IntegerLiteral => {
                self.advance();
                let v = tok.lexeme.parse::<i64>().unwrap_or(0);
                Ok(Expr {
                    kind: ExprKind::Literal(Literal::Int(v)),
                    span: tok.span,
                    id: self.allocate_id(),
                })
            }
            TokenKind::FloatLiteral => {
                self.advance();
                let v = tok.lexeme.parse::<f64>().unwrap_or(0.0);
                Ok(Expr {
                    kind: ExprKind::Literal(Literal::Float(v)),
                    span: tok.span,
                    id: self.allocate_id(),
                })
            }
            TokenKind::StringLiteral => {
                self.advance();
                let s = tok.lexeme.trim_matches('"').to_string();
                Ok(Expr {
                    kind: ExprKind::Literal(Literal::String(s)),
                    span: tok.span,
                    id: self.allocate_id(),
                })
            }
            TokenKind::CharLiteral => {
                self.advance();
                let bytes = tok.lexeme.as_bytes();
                let val = if bytes.len() >= 3 { bytes[1] } else { 0 };
                Ok(Expr {
                    kind: ExprKind::Literal(Literal::Char(val)),
                    span: tok.span,
                    id: self.allocate_id(),
                })
            }
            TokenKind::LParen => {
                self.advance();
                let e = self.parse_expr()?;
                self.expect(TokenKind::RParen, "expected ')'")?;
                Ok(e)
            }
            _ => self.err_here("expected primary expression"),
        }
    }

    fn current_binary_op(&self) -> Option<(BinaryOp, u8)> {
        let k = &self.current().kind;
        Some(match k {
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

    fn looks_like_cast(&self) -> bool {
        if !self.check(TokenKind::LParen) {
            return false;
        }
        let k1 = self.peek_kind(1);
        let k2 = self.peek_kind(2);
        matches!(
            k1,
            Some(
                TokenKind::Int
                    | TokenKind::Char
                    | TokenKind::Float
                    | TokenKind::Double
                    | TokenKind::Void
                    | TokenKind::Struct
            )
        ) && matches!(k2, Some(TokenKind::RParen) | Some(TokenKind::Identifier))
    }

    fn is_type_start(&self) -> bool {
        matches!(
            self.current().kind,
            TokenKind::Int
                | TokenKind::Char
                | TokenKind::Float
                | TokenKind::Double
                | TokenKind::Void
        )
    }

    fn check(&self, kind: TokenKind) -> bool {
        self.current().kind == kind
    }

    fn match_kind(&mut self, kind: TokenKind) -> bool {
        if self.check(kind.clone()) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, kind: TokenKind, msg: &str) -> PResult<Token<'a>> {
        if self.check(kind.clone()) {
            Ok(self.advance().clone())
        } else {
            Err(ParseError {
                message: msg.to_string(),
                span: self.current().span,
            })
        }
    }

    fn advance(&mut self) -> &Token<'a> {
        if self.pos < self.tokens.len() {
            self.pos += 1;
        }
        self.prev()
    }

    fn current(&self) -> &Token<'a> {
        self.tokens
            .get(self.pos)
            .unwrap_or_else(|| self.tokens.last().expect("tokens not empty"))
    }

    fn prev(&self) -> &Token<'a> {
        let idx = self.pos.saturating_sub(1);
        &self.tokens[idx]
    }

    fn prev_span(&self) -> Span {
        self.prev().span
    }

    fn peek_kind(&self, n: usize) -> Option<TokenKind> {
        self.tokens.get(self.pos + n).map(|t| t.kind.clone())
    }

    fn err_here<T>(&self, msg: &str) -> PResult<T> {
        Err(ParseError {
            message: msg.to_string(),
            span: self.current().span,
        })
    }

    fn allocate_id(&mut self) -> NodeId {
        let id = self.next_node_id;
        self.next_node_id += 1;
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::lexer::Lexer;

    fn parse_ok(src: &str) -> Vec<Decl> {
        let lexer = Lexer::new(src);
        let mut parser = Parser::from_lexer(lexer).expect("lexer failed");
        let (decls, errors) = parser.parse_translation_unit();
        if !errors.is_empty() {
            panic!(
                "parse failed with {} error(s): {} at {:?}",
                errors.len(),
                errors[0].message,
                errors[0].span
            );
        }
        decls
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
            other => panic!("expected variable decl, got {:?}", other),
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
    fn parses_function_definition_with_return() {
        let tu = parse_ok("int add(int a, int b) { return a + b; }");
        match &tu[0].kind {
            DeclKind::Function { body, .. } => {
                let body = body.as_ref().expect("expected function body");
                match &body.kind {
                    StmtKind::Block(items) => {
                        assert_eq!(items.len(), 1);
                        match &items[0] {
                            BlockItem::Stmt(Stmt {
                                kind: StmtKind::Return(Some(expr)),
                                ..
                            }) => {
                                assert!(matches!(expr.kind, ExprKind::Binary(BinaryOp::Add, _, _)));
                            }
                            other => panic!("unexpected block item: {:?}", other),
                        }
                    }
                    _ => panic!("expected block body"),
                }
            }
            _ => panic!("expected function declaration"),
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
            other => panic!("expected add at root, got {:?}", other),
        }
    }

    #[test]
    fn parses_assignment_right_associative() {
        let tu = parse_ok("int f(){ int a; int b; int c; a = b = c; return a; }");
        let func_body = match &tu[0].kind {
            DeclKind::Function { body: Some(b), .. } => b,
            _ => panic!("expected function with body"),
        };

        let items = match &func_body.kind {
            StmtKind::Block(items) => items,
            _ => panic!("expected block"),
        };

        let assign_stmt = items
            .iter()
            .find_map(|it| match it {
                BlockItem::Stmt(Stmt {
                    kind: StmtKind::Expr(e),
                    ..
                }) => Some(e),
                _ => None,
            })
            .expect("missing assignment stmt");

        match &assign_stmt.kind {
            ExprKind::Binary(BinaryOp::Assign, _, rhs) => {
                assert!(matches!(rhs.kind, ExprKind::Binary(BinaryOp::Assign, _, _)));
            }
            _ => panic!("expected assignment expression"),
        }
    }

    #[test]
    fn parses_if_else_statement() {
        let tu = parse_ok("int f(int x){ if (x) return 1; else return 0; }");

        let body = match &tu[0].kind {
            DeclKind::Function { body: Some(b), .. } => b,
            _ => panic!("expected function body"),
        };

        let items = match &body.kind {
            StmtKind::Block(items) => items,
            _ => panic!("expected block"),
        };

        let if_stmt = match &items[0] {
            BlockItem::Stmt(s) => s,
            _ => panic!("expected stmt"),
        };

        match &if_stmt.kind {
            StmtKind::If { else_branch, .. } => assert!(else_branch.is_some()),
            _ => panic!("expected if statement"),
        }
    }

    #[test]
    fn parses_while_statement() {
        let tu = parse_ok("int f(){ int x; while (x) x = x - 1; return x; }");
        let body = match &tu[0].kind {
            DeclKind::Function { body: Some(b), .. } => b,
            _ => panic!("expected function body"),
        };

        let items = match &body.kind {
            StmtKind::Block(items) => items,
            _ => panic!("expected block"),
        };

        assert!(items.iter().any(|it| matches!(
            it,
            BlockItem::Stmt(Stmt {
                kind: StmtKind::While { .. },
                ..
            })
        )));
    }

    #[test]
    fn parses_for_statement() {
        let tu = parse_ok("int f(){ int i; for (i = 0; i < 10; i++) { } return i; }");
        let body = match &tu[0].kind {
            DeclKind::Function { body: Some(b), .. } => b,
            _ => panic!("expected function body"),
        };

        let items = match &body.kind {
            StmtKind::Block(items) => items,
            _ => panic!("expected block"),
        };

        assert!(items.iter().any(|it| matches!(
            it,
            BlockItem::Stmt(Stmt {
                kind: StmtKind::For { .. },
                ..
            })
        )));
    }

    #[test]
    fn parses_call_index_member_and_postfix_inc() {
        let tu = parse_ok("int f(){ arr[i].x++; return 0; }");
        let body = match &tu[0].kind {
            DeclKind::Function { body: Some(b), .. } => b,
            _ => panic!("expected function body"),
        };
        let items = match &body.kind {
            StmtKind::Block(items) => items,
            _ => panic!("expected block"),
        };

        let expr = match &items[0] {
            BlockItem::Stmt(Stmt {
                kind: StmtKind::Expr(e),
                ..
            }) => e,
            _ => panic!("expected expression stmt"),
        };

        assert!(matches!(expr.kind, ExprKind::Unary(UnaryOp::PostInc, _)));
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
    fn rejects_invalid_input() {
        let lexer = Lexer::new("int x = ;");
        let mut parser = Parser::from_lexer(lexer).expect("lexing should succeed");
        let (_decls, errors) = parser.parse_translation_unit();
        assert!(!errors.is_empty(), "should produce at least one error");
        assert!(!errors[0].message.is_empty());
    }

    #[test]
    fn reports_multiple_errors() {
        // Two bad declarations: missing initializer expressions.
        let lexer = Lexer::new("int x = ; int y = ;");
        let mut parser = Parser::from_lexer(lexer).expect("lexing should succeed");
        let (_decls, errors) = parser.parse_translation_unit();
        assert!(
            errors.len() >= 2,
            "expected at least 2 errors, got {}",
            errors.len()
        );
    }

    #[test]
    fn recovers_valid_decls_after_error() {
        // First decl is broken, second is valid.
        let lexer = Lexer::new("int x = ; int y = 5;");
        let mut parser = Parser::from_lexer(lexer).expect("lexing should succeed");
        let (decls, errors) = parser.parse_translation_unit();
        assert!(!errors.is_empty(), "should have parse errors");
        assert!(
            !decls.is_empty(),
            "should recover and parse valid declarations"
        );
    }
}
