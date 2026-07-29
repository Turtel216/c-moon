//! Pretty Printer for C AST
use crate::frontend::ast::*;
use std::fmt::{self, Write};

pub struct AstPrinter {
    indent_level: usize,
}

impl AstPrinter {
    pub fn new() -> Self {
        Self { indent_level: 0 }
    }

    fn indent(&self, w: &mut impl Write) -> fmt::Result {
        write!(w, "{}", "    ".repeat(self.indent_level))
    }

    pub fn print_decl(&mut self, decl: &Decl, w: &mut impl Write) -> fmt::Result {
        match &decl.kind {
            DeclKind::Variable {
                ty,
                name,
                initializer,
            } => {
                self.indent(w)?;
                write!(w, "{ty} {name}")?;
                if let Some(init) = initializer {
                    write!(w, " = ")?;
                    self.print_expr(init, w)?;
                }
                writeln!(w, ";")
            }
            DeclKind::Function {
                return_ty,
                name,
                params,
                body,
            } => {
                self.indent(w)?;
                write!(w, "{return_ty} {name}(")?;
                for (i, param) in params.iter().enumerate() {
                    if i > 0 {
                        write!(w, ", ")?;
                    }
                    write!(w, "{}", param.ty)?;
                    if let Some(pname) = &param.name {
                        write!(w, " {}", pname)?;
                    }
                }
                write!(w, ")")?;

                if let Some(b) = body {
                    writeln!(w, " {{")?;
                    self.indent_level += 1;
                    self.print_stmt_inner(b, w)?;
                    self.indent_level -= 1;
                    self.indent(w)?;
                    writeln!(w, "}}")
                } else {
                    writeln!(w, ";")
                }
            }
            DeclKind::Struct { name, members } => {
                self.indent(w)?;
                write!(w, "struct ")?;
                if let Some(n) = name {
                    write!(w, "{} ", n)?;
                }
                writeln!(w, "{{")?;
                self.indent_level += 1;
                for member in members {
                    self.print_decl(member, w)?;
                }
                self.indent_level -= 1;
                self.indent(w)?;
                writeln!(w, "}};")
            }
        }
    }

    pub fn print_stmt(&mut self, stmt: &Stmt, w: &mut impl Write) -> fmt::Result {
        self.indent(w)?;
        self.print_stmt_inner(stmt, w)?;
        writeln!(w)
    }

    /// Prints statements without the leading indent (useful for blocks/loops)
    fn print_stmt_inner(&mut self, stmt: &Stmt, w: &mut impl Write) -> fmt::Result {
        match &stmt.kind {
            StmtKind::Empty => write!(w, ";"),
            StmtKind::Expr(expr) => {
                self.print_expr(expr, w)?;
                write!(w, ";")
            }
            StmtKind::Return(expr) => {
                write!(w, "return")?;
                if let Some(e) = expr {
                    write!(w, " ")?;
                    self.print_expr(e, w)?;
                }
                write!(w, ";")
            }
            StmtKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                write!(w, "if (")?;
                self.print_expr(condition, w)?;
                writeln!(w, ") {{")?;
                self.indent_level += 1;
                self.print_stmt(then_branch, w)?;
                self.indent_level -= 1;

                if let Some(els) = else_branch {
                    self.indent(w)?;
                    writeln!(w, "}} else {{")?;
                    self.indent_level += 1;
                    self.print_stmt(els, w)?;
                    self.indent_level -= 1;
                }
                self.indent(w)?;
                write!(w, "}}")
            }
            StmtKind::While { condition, body } => {
                write!(w, "while (")?;
                self.print_expr(condition, w)?;
                writeln!(w, ") {{")?;
                self.indent_level += 1;
                self.print_stmt(body, w)?;
                self.indent_level -= 1;
                self.indent(w)?;
                write!(w, "}}")
            }
            StmtKind::For {
                init,
                condition,
                step,
                body,
            } => {
                write!(w, "for (")?;
                if let Some(i) = init {
                    // Quick hack: print without trailing newline
                    let mut temp = String::new();
                    self.print_stmt_inner(i, &mut temp)?;
                    write!(w, "{} ", temp)?;
                } else {
                    write!(w, "; ")?;
                }
                if let Some(c) = condition {
                    self.print_expr(c, w)?;
                }
                write!(w, "; ")?;
                if let Some(s) = step {
                    self.print_expr(s, w)?;
                }
                writeln!(w, ") {{")?;
                self.indent_level += 1;
                self.print_stmt(body, w)?;
                self.indent_level -= 1;
                self.indent(w)?;
                write!(w, "}}")
            }
            StmtKind::Block(items) => {
                for item in items {
                    match item {
                        BlockItem::Stmt(s) => self.print_stmt(s, w)?,
                        BlockItem::Decl(d) => self.print_decl(d, w)?,
                    }
                }
                Ok(())
            }
        }
    }

    pub fn print_expr(&mut self, expr: &Expr, w: &mut impl Write) -> fmt::Result {
        match &expr.kind {
            ExprKind::Literal(lit) => match lit {
                Literal::Int(i) => write!(w, "{}", i),
                Literal::Float(f) => write!(w, "{}", f),
                Literal::Char(c) => write!(w, "'{}'", *c as char),
                Literal::String(s) => write!(w, "\"{}\"", s),
            },
            ExprKind::Identifier(name) => write!(w, "{}", name),
            ExprKind::Binary(op, left, right) => {
                write!(w, "(")?;
                self.print_expr(left, w)?;
                write!(w, " {} ", self.binop_str(*op))?;
                self.print_expr(right, w)?;
                write!(w, ")")
            }
            ExprKind::Unary(op, inner) => {
                let op_str = self.unop_str(*op);
                // Prefix operators
                if !matches!(op, UnaryOp::PostInc | UnaryOp::PostDec) {
                    write!(w, "{}", op_str)?;
                }
                self.print_expr(inner, w)?;
                // Postfix operators
                if matches!(op, UnaryOp::PostInc | UnaryOp::PostDec) {
                    write!(w, "{}", op_str)?;
                }
                Ok(())
            }
            ExprKind::Call { callee, args } => {
                self.print_expr(callee, w)?;
                write!(w, "(")?;
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 {
                        write!(w, ", ")?;
                    }
                    self.print_expr(arg, w)?;
                }
                write!(w, ")")
            }
            ExprKind::Index { array, index } => {
                self.print_expr(array, w)?;
                write!(w, "[")?;
                self.print_expr(index, w)?;
                write!(w, "]")
            }
            ExprKind::MemberAccess {
                base,
                member,
                is_arrow,
            } => {
                self.print_expr(base, w)?;
                write!(w, "{}{}", if *is_arrow { "->" } else { "." }, member)
            }
            ExprKind::Cast(ty, inner) => {
                write!(w, "({ty})")?;
                self.print_expr(inner, w)
            }
            ExprKind::SizeOf(inner) => {
                write!(w, "sizeof(")?;
                self.print_expr(inner, w)?;
                write!(w, ")")
            }
        }
    }

    fn binop_str(&self, op: BinaryOp) -> &'static str {
        match op {
            BinaryOp::Add => "+",
            BinaryOp::Sub => "-",
            BinaryOp::Mul => "*",
            BinaryOp::Div => "/",
            BinaryOp::Mod => "%",
            BinaryOp::Eq => "==",
            BinaryOp::Neq => "!=",
            BinaryOp::Lt => "<",
            BinaryOp::Lte => "<=",
            BinaryOp::Gt => ">",
            BinaryOp::Gte => ">=",
            BinaryOp::LogicalAnd => "&&",
            BinaryOp::LogicalOr => "||",
            BinaryOp::BitAnd => "&",
            BinaryOp::BitOr => "|",
            BinaryOp::BitXor => "^",
            BinaryOp::Shl => "<<",
            BinaryOp::Shr => ">>",
            BinaryOp::Assign => "=",
        }
    }

    fn unop_str(&self, op: UnaryOp) -> &'static str {
        match op {
            UnaryOp::Neg => "-",
            UnaryOp::Not => "!",
            UnaryOp::BitNot => "~",
            UnaryOp::Deref => "*",
            UnaryOp::AddressOf => "&",
            UnaryOp::PreInc | UnaryOp::PostInc => "++",
            UnaryOp::PreDec | UnaryOp::PostDec => "--",
        }
    }
}
