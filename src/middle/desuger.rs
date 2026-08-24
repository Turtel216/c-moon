//! Responsible for desugering the AST into the IR
//!
//! What comes out is unoptimised three-address code. Optimisation happens
//! afterwards, on the SSA form the middle-end builds from it -- see
//! [`ssa::passes`](crate::middle::ssa::passes).

use std::collections::BTreeMap;
use std::collections::HashMap;

use crate::frontend::ast::Sign;
use crate::frontend::ast::{
    BinaryOp, BlockItem, Decl, DeclKind, Expr, ExprKind, Literal, NodeId, Stmt, StmtKind, UnaryOp,
};
use crate::frontend::renamer::ResolutionMap;
use crate::frontend::semantic::{Type, TypeMap};
use crate::middle::ir::*;

/// Complete program IR representation
#[derive(Debug, Clone)]
pub struct ProgramIr {
    /// Program Functions
    pub functions: BTreeMap<String, CFG>,
    /// Maps variable id → the bytes each local array occupies.
    /// Used by the backend to allocate stack space.
    ///
    /// Bytes rather than elements because the element type decides how many
    /// each one takes: `char a[3]` is three bytes, `int a[3]` twelve and
    /// `long int a[3]` twenty-four.
    pub array_sizes: HashMap<usize, usize>,
    /// Maps variable id → the name it was declared with, for IR dumps.
    pub var_names: HashMap<usize, String>,
}

impl ProgramIr {
    pub fn new() -> Self {
        Self {
            functions: BTreeMap::new(),
            array_sizes: HashMap::new(),
            var_names: HashMap::new(),
        }
    }
}

/// Context for AST to IR desugering
pub struct LoweringContext<'a> {
    /// Complete ProgramIr
    pub program: ProgramIr,
    /// Resolution map used for conflicting identifiers
    res_map: &'a ResolutionMap,
    /// The type of every expression and declaration, which is what decides
    /// the width of the instruction each one becomes.
    types: &'a TypeMap,
    /// Width of the value the function being lowered returns.
    return_width: Width,
    /// Current ``CFG`` being lowered
    current_cfg: Option<CFG>,
    current_block: String,
    temp_counter: usize,
    label_counter: usize,
    /// Tracks local array sizes: var_id → bytes occupied.
    array_sizes: HashMap<usize, usize>,
}

impl<'a> LoweringContext<'a> {
    pub fn new(res_map: &'a ResolutionMap, types: &'a TypeMap) -> Self {
        Self {
            program: ProgramIr::new(),
            res_map,
            types,
            return_width: Width::Bits64,
            current_cfg: None,
            current_block: String::new(),
            temp_counter: 0,
            label_counter: 0,
            array_sizes: HashMap::new(),
        }
    }

    /// Desuger a list of AST ``Decl`` into the ``ProgramIr``
    pub fn lower_program(mut self, decls: &[Decl]) -> ProgramIr {
        self.program.var_names = self.res_map.var_names.clone();

        for decl in decls {
            match &decl.kind {
                DeclKind::Function {
                    name, body, params, ..
                } => {
                    self.return_width = width_of(&self.types.function(name).return_ty);
                    let bod = body.clone().unwrap(); // TODO: Fix unsafe unwrap and clone

                    // Each parameter as the variable it binds and the width
                    // its declared type is held at.
                    let parameters: Vec<(usize, Width)> = params
                        .iter()
                        .map(|p| {
                            let var =
                                *self.res_map.decl_to_var.get(&p.id).expect(
                                    "Compiler Bug: Ranamer failed to map function parameters",
                                );
                            (var, width_of(self.types.decl(p.id)))
                        })
                        .collect();

                    self.lower_function(name, &parameters, &bod);
                }
                _ => {
                    todo!("Global variables not implemented yet")
                }
            }
        }
        self.program
    }

    fn lower_function(&mut self, name: &str, params: &[(usize, Width)], body: &Stmt) {
        // Setup a new CFG for this function
        let entry = format!("{}_entry", name);
        let exit = format!("{}_exit", name);

        let mut cfg = CFG::new(entry.clone(), exit.clone());
        cfg.add_block(BasicBlock::new(entry.clone()));
        cfg.add_block(BasicBlock::new(exit.clone()));

        self.current_cfg = Some(cfg);
        self.current_block = entry.clone();

        // Bind parameters to local variables
        for (index, &(param_id, width)) in params.iter().enumerate() {
            self.emit(TACInstruction::new(
                Opcode::GetParam,
                width,
                Some(Operand::Var(param_id)), // dest: local variable
                Some(Operand::ImmInt(index as i64)), // arg1: parameter index
                None,
            ));
        }

        // Lower the body
        self.lower_statement(body);

        // Fallthrough to exit if the last block didn't explicitly return
        let cur = self.current_block.clone();
        if cur != exit {
            self.emit(TACInstruction::transfer(
                Opcode::Jump,
                None,
                Some(Operand::Label(exit.clone())),
                None,
            ));
            self.add_edge(&cur, &exit);
        }

        // Save the finished CFG into the Program
        if let Some(finished_cfg) = self.current_cfg.take() {
            self.program
                .functions
                .insert(name.to_string(), finished_cfg);
        }

        // Transfer array size metadata to ProgramIr
        for (var_id, size) in &self.array_sizes {
            self.program.array_sizes.insert(*var_id, *size);
        }
    }

    fn fresh_temp(&mut self) -> Operand {
        self.temp_counter += 1;
        Operand::Temp(format!("t{}", self.temp_counter))
    }

    fn fresh_label(&mut self, prefix: &str) -> String {
        self.label_counter += 1;
        format!("{}_L{}", prefix, self.label_counter)
    }

    fn create_block(&mut self, prefix: &str) -> String {
        let label = self.fresh_label(prefix);
        let cfg = self.current_cfg.as_mut().expect("Not inside a function!");
        cfg.add_block(BasicBlock::new(label.clone()));
        label
    }

    fn set_current_block(&mut self, label: String) {
        self.current_block = label;
    }

    fn emit(&mut self, instr: TACInstruction) {
        let cfg = self.current_cfg.as_mut().expect("Not inside a function!");
        let blk = cfg.blocks.get_mut(&self.current_block).unwrap();
        blk.emit(instr);
    }

    fn add_edge(&mut self, from: &str, to: &str) {
        let cfg = self.current_cfg.as_mut().expect("Not inside a function!");
        cfg.add_edge(from, to);
    }

    fn lower_statement(&mut self, stmt: &Stmt) {
        match &stmt.kind {
            // The empty statement produces no code.
            StmtKind::Empty => {}

            StmtKind::Expr(expr) => {
                // Includes assignments represented as BinaryOp::Assign.
                let _ = self.lower_expression(expr);
            }

            StmtKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                let then_label = self.create_block("if_then");
                let else_label = self.create_block("if_else");
                let end_label = self.create_block("if_end");

                let (cond, width) = self.lower_condition(condition);

                let cur = self.current_block.clone();
                // if !cond -> else
                self.emit(TACInstruction::new(
                    Opcode::BranchIfNot,
                    width,
                    None,
                    Some(cond),
                    Some(Operand::Label(else_label.clone())),
                ));
                self.add_edge(&cur, &else_label);

                // otherwise -> then
                self.emit(TACInstruction::transfer(
                    Opcode::Jump,
                    None,
                    Some(Operand::Label(then_label.clone())),
                    None,
                ));
                self.add_edge(&cur, &then_label);

                // then branch
                self.set_current_block(then_label.clone());
                self.lower_statement(then_branch);
                let then_end = self.current_block.clone();
                self.emit(TACInstruction::transfer(
                    Opcode::Jump,
                    None,
                    Some(Operand::Label(end_label.clone())),
                    None,
                ));
                self.add_edge(&then_end, &end_label);

                // else branch (or empty else)
                self.set_current_block(else_label.clone());
                if let Some(e) = else_branch {
                    self.lower_statement(e);
                }
                let else_end = self.current_block.clone();
                self.emit(TACInstruction::transfer(
                    Opcode::Jump,
                    None,
                    Some(Operand::Label(end_label.clone())),
                    None,
                ));
                self.add_edge(&else_end, &end_label);

                self.set_current_block(end_label);
            }

            StmtKind::While { condition, body } => {
                let cond_label = self.create_block("while_cond");
                let body_label = self.create_block("while_body");
                let end_label = self.create_block("while_end");

                // preheader -> cond
                let preheader = self.current_block.clone();
                self.emit(TACInstruction::transfer(
                    Opcode::Jump,
                    None,
                    Some(Operand::Label(cond_label.clone())),
                    None,
                ));
                self.add_edge(&preheader, &cond_label);

                // cond block
                self.set_current_block(cond_label.clone());
                let (cond, width) = self.lower_condition(condition);

                // A short-circuiting condition branches on its way to a value,
                // so the test is reached in the block the condition left off
                // in rather than in the one it started in.
                let cond_end = self.current_block.clone();

                // if !cond -> end
                self.emit(TACInstruction::new(
                    Opcode::BranchIfNot,
                    width,
                    None,
                    Some(cond),
                    Some(Operand::Label(end_label.clone())),
                ));
                self.add_edge(&cond_end, &end_label);

                // else -> body
                self.emit(TACInstruction::transfer(
                    Opcode::Jump,
                    None,
                    Some(Operand::Label(body_label.clone())),
                    None,
                ));
                self.add_edge(&cond_end, &body_label);

                // body block
                self.set_current_block(body_label.clone());
                self.lower_statement(body);

                // back-edge body -> cond
                let body_end = self.current_block.clone();
                self.emit(TACInstruction::transfer(
                    Opcode::Jump,
                    None,
                    Some(Operand::Label(cond_label.clone())),
                    None,
                ));
                self.add_edge(&body_end, &cond_label);

                // continue after loop
                self.set_current_block(end_label);
            }

            StmtKind::Block(items) => {
                for item in items {
                    match item {
                        BlockItem::Stmt(s) => self.lower_statement(s),
                        BlockItem::Decl(d) => {
                            let stmt_id = self
                                .res_map
                                .decl_to_var
                                .get(&d.id)
                                .expect("Compiler Bug: Renamer failed to map declaration");

                            let stmt_id = *stmt_id;
                            if let Type::Array(element, count) = self.types.decl(d.id) {
                                // Record the array's storage for backend stack
                                // allocation. No TAC instructions needed --
                                // the array lives entirely on the stack.
                                let bytes = count * width_of(element).bytes() as usize;
                                self.array_sizes.insert(stmt_id, bytes);
                            } else if let DeclKind::Variable {
                                initializer: Some(init),
                                ..
                            } = &d.kind
                            {
                                // Scalar variable with initializer, converted
                                // to the declared type the way C converts it.
                                let width = width_of(self.types.decl(d.id));
                                let rhs = self.lower_converted(init, width);
                                self.emit(TACInstruction::new(
                                    Opcode::Mov,
                                    width,
                                    Some(Operand::Var(stmt_id)),
                                    Some(rhs),
                                    None,
                                ));
                            }
                            // Scalar variable without initializer — no code needed.
                        }
                    }
                }
            }

            StmtKind::Return(expr_opt) => {
                // The value is converted to the declared return type here, so
                // the caller always receives what the signature promises.
                let ret_val = expr_opt
                    .as_ref()
                    .map(|expr| self.lower_converted(expr, self.return_width));

                // The width is the return type's: `ret` reads exactly the
                // value the function promised its caller and no more.
                self.emit(TACInstruction::new(
                    Opcode::Ret,
                    self.return_width,
                    None,
                    ret_val,
                    None,
                ));

                let exit_label = self.current_cfg.as_ref().unwrap().exit.clone();

                let cur = self.current_block.clone();
                self.add_edge(&cur, &exit_label);

                let dead_block = self.create_block("unreachable_after_ret");
                self.set_current_block(dead_block);
            }

            // A `for` is lowered like a `while` with the init clause hoisted
            // into the preheader and the step appended to the body:
            //
            //   <init>; goto cond
            //   cond: if !condition goto end; goto body
            //   body: <body>; <step>; goto cond
            //   end:
            StmtKind::For {
                init,
                condition,
                step,
                body,
            } => {
                let cond_label = self.create_block("for_cond");
                let body_label = self.create_block("for_body");
                let end_label = self.create_block("for_end");

                // preheader: the init clause runs once, before the loop
                if let Some(init) = init {
                    self.lower_statement(init);
                }
                let preheader = self.current_block.clone();
                self.emit(TACInstruction::transfer(
                    Opcode::Jump,
                    None,
                    Some(Operand::Label(cond_label.clone())),
                    None,
                ));
                self.add_edge(&preheader, &cond_label);

                // cond block
                self.set_current_block(cond_label.clone());
                if let Some(condition) = condition {
                    let (cond, width) = self.lower_condition(condition);

                    // As in a `while`, a condition that branches on its way to
                    // a value is tested in the block it left off in.
                    let cond_end = self.current_block.clone();

                    // if !cond -> end
                    self.emit(TACInstruction::new(
                        Opcode::BranchIfNot,
                        width,
                        None,
                        Some(cond),
                        Some(Operand::Label(end_label.clone())),
                    ));
                    self.add_edge(&cond_end, &end_label);
                }
                // An omitted condition is always true, so `for (;;)` falls
                // straight through to the body.

                // else -> body
                let cond_end = self.current_block.clone();
                self.emit(TACInstruction::transfer(
                    Opcode::Jump,
                    None,
                    Some(Operand::Label(body_label.clone())),
                    None,
                ));
                self.add_edge(&cond_end, &body_label);

                // body block, with the step at its end
                self.set_current_block(body_label);
                self.lower_statement(body);
                if let Some(step) = step {
                    let _ = self.lower_expression(step);
                }

                // back-edge body -> cond
                let body_end = self.current_block.clone();
                self.emit(TACInstruction::transfer(
                    Opcode::Jump,
                    None,
                    Some(Operand::Label(cond_label.clone())),
                    None,
                ));
                self.add_edge(&body_end, &cond_label);

                // continue after loop
                self.set_current_block(end_label);
            }
        }
    }

    /// The width a value of the type `expr` was given is held at.
    fn width_of_expr(&self, expr: &Expr) -> Width {
        width_of(self.types.expr(expr.id))
    }

    /// How the bits of the value `expr` produces read.
    fn sign_of_expr(&self, expr: &Expr) -> Sign {
        sign_of(self.types.expr(expr.id))
    }

    /// Lower a controlling expression.
    ///
    /// # Returns
    ///
    /// The value to test and the width to test it at, which is the width of
    /// its own type: only that many bits of it mean anything.
    fn lower_condition(&mut self, condition: &Expr) -> (Operand, Width) {
        let width = self.width_of_expr(condition);
        (self.lower_expression(condition), width)
    }

    /// Lower `expr` and convert its value to `target`, as C converts it when
    /// it is assigned, passed or returned somewhere of that width.
    fn lower_converted(&mut self, expr: &Expr, target: Width) -> Operand {
        let (from, sign) = (self.width_of_expr(expr), self.sign_of_expr(expr));
        let value = self.lower_expression(expr);
        self.convert(value, from, sign, target)
    }

    /// Emit the conversion of `value` from one integer width to another.
    ///
    /// Converting a value to the width it already has is what most of the
    /// program does, so that case emits nothing at all.
    ///
    /// # Arguments
    ///
    /// * `value` - what to convert
    /// * `from` - the width it is held at
    /// * `sign` - how its bits read, which is what a widening puts above them
    /// * `to` - the width to produce
    fn convert(&mut self, value: Operand, from: Width, sign: Sign, to: Width) -> Operand {
        if from == to {
            return value;
        }
        // A literal is a bit pattern the compiler can read for itself, so its
        // conversion is a matter of writing it down differently rather than of
        // running an instruction.
        if let Operand::ImmInt(constant) = value {
            return Operand::ImmInt(to.narrow(from.read(sign, constant)));
        }

        let dest = self.fresh_temp();
        self.emit(TACInstruction::new(
            Opcode::Convert { from, sign },
            to,
            Some(dest.clone()),
            Some(value),
            None,
        ));
        dest
    }

    fn lower_expression(&mut self, expr: &Expr) -> Operand {
        match &expr.kind {
            // Constants travel through the IR as the bit pattern they name,
            // which is what a literal too large for an `i64` -- and so of
            // type `unsigned long int` -- is held as.
            ExprKind::Literal(Literal::Int(v)) => Operand::ImmInt(*v as i64),

            // A character constant is an `int` whose value is the character's
            // (see `SemanticAnalyzer::literal_type`), sign-extended because a
            // plain `char` is signed: `'\xff'` is -1, as it is to GCC.
            ExprKind::Literal(Literal::Char(c)) => Operand::ImmInt(i64::from(*c as i8)),

            ExprKind::Identifier(_) => {
                let idf_id = self
                    .res_map
                    .expr_to_var
                    .get(&expr.id)
                    .expect("Compiler Bug: Renamer failed to map lhs of expression");
                Operand::Var(*idf_id)
            }

            // *ptr = rhs (assignment through pointer dereference)
            ExprKind::Binary(BinaryOp::Assign, lhs, rhs)
                if matches!(lhs.kind, ExprKind::Unary(UnaryOp::Deref, _)) =>
            {
                // The object written is as wide as what the pointer points to,
                // and the value is converted to it before the store.
                let width = self.width_of_expr(lhs);
                let rhs_op = self.lower_converted(rhs, width);

                // Extract the pointer expression from the LHS dereference.
                let ptr_expr = match &lhs.kind {
                    ExprKind::Unary(UnaryOp::Deref, inner) => inner,
                    _ => unreachable!(),
                };

                let addr_op = self.lower_expression(ptr_expr);

                // Store: *addr_op = rhs_op
                self.emit(TACInstruction::new(
                    Opcode::Store,
                    width,
                    None,
                    Some(addr_op),
                    Some(rhs_op.clone()),
                ));

                rhs_op
            }

            // arr[idx] = rhs
            ExprKind::Binary(BinaryOp::Assign, lhs, rhs)
                if matches!(lhs.kind, ExprKind::Index { .. }) =>
            {
                // An element is as wide as the array's element type, which is
                // also what scales the index into a byte offset.
                let width = self.width_of_expr(lhs);
                let rhs_op = self.lower_converted(rhs, width);

                // Extract the array base variable and index from the LHS.
                let (array_base, index_expr) = match &lhs.kind {
                    ExprKind::Index { array, index } => (array, index),
                    _ => unreachable!(),
                };

                let base_var = self.lower_expression(array_base);
                let idx_op = self.lower_index(index_expr);

                // ArrayStore: dest=base_var, arg1=index, arg2=value
                self.emit(TACInstruction::new(
                    Opcode::ArrayStore,
                    width,
                    Some(base_var.clone()),
                    Some(idx_op),
                    Some(rhs_op.clone()),
                ));

                rhs_op
            }

            // x = rhs
            ExprKind::Binary(BinaryOp::Assign, lhs, rhs) => {
                let width = self.width_of_expr(lhs);
                let rhs_op = self.lower_converted(rhs, width);
                let lhs_var = self.expect_lvalue_var(lhs);

                let lhs_id = self
                    .res_map
                    .expr_to_var
                    .get(&lhs_var)
                    .expect("Compiler Bug: Renamer failed to map lhs of expression");

                self.emit(TACInstruction::new(
                    Opcode::Mov,
                    width,
                    Some(Operand::Var(*lhs_id)),
                    Some(rhs_op.clone()),
                    None,
                ));
                Operand::Var(*lhs_id)
            }

            // `a && b` and `a || b`, whose right operand is evaluated only
            // when the left one did not already settle the answer.
            ExprKind::Binary(operator @ (BinaryOp::LogicalAnd | BinaryOp::LogicalOr), lhs, rhs) => {
                self.lower_short_circuit(*operator, lhs, rhs)
            }

            // Arithmetic + comparisons used by if/while conditions.
            ExprKind::Binary(op, lhs, rhs) => {
                // C's usual arithmetic conversions: both operands are brought
                // to the type they have in common, and the operation is
                // computed at its width. A comparison narrows again on its own
                // -- its result is the 0 or 1 of an `int` -- which is why the
                // width recorded here is the one the operands are compared at.
                let types = self.types;
                let common = Type::common(types.expr(lhs.id), types.expr(rhs.id));
                let (width, sign) = (width_of(&common), sign_of(&common));
                let l = self.lower_converted(lhs, width);
                let r = self.lower_converted(rhs, width);
                let t = self.fresh_temp();

                // The operations that read the operands' bits rather than just
                // moving them carry the signedness of the type both were
                // converted to; the rest give the same answer either way.
                let opcode = match op {
                    BinaryOp::Add => Opcode::Add,
                    BinaryOp::Sub => Opcode::Sub,
                    BinaryOp::Mul => Opcode::Mul,
                    BinaryOp::Div => Opcode::Div(sign),
                    BinaryOp::Eq => Opcode::Eq,
                    BinaryOp::Neq => Opcode::Neq,
                    BinaryOp::Lt => Opcode::Lt(sign),
                    BinaryOp::Lte => Opcode::Lte(sign),
                    BinaryOp::Gt => Opcode::Gt(sign),
                    BinaryOp::Gte => Opcode::Gte(sign),
                    _ => panic!("Binary op {:?} not supported in this lowering phase", op),
                };

                self.emit(TACInstruction::new(
                    opcode,
                    width,
                    Some(t.clone()),
                    Some(l),
                    Some(r),
                ));
                t
            }

            ExprKind::Call { callee, args } => {
                // Lower arguments, each converted to the type its parameter
                // was declared with.
                let signature = match &callee.kind {
                    ExprKind::Identifier(name) => self.types.function(name),
                    _ => panic!("Compiler Bug: call to something that is not a named function"),
                };
                let param_widths: Vec<Width> = signature
                    .params
                    .iter()
                    .map(|param| width_of(&param.ty))
                    .collect();
                let return_width = width_of(&signature.return_ty);

                let mut arg_operands = Vec::with_capacity(args.len());
                for (arg, &width) in args.iter().zip(&param_widths) {
                    arg_operands.push(self.lower_converted(arg, width));
                }

                // Determine the target of the call.
                // If it's a direct identifier, its treated as a static Label.
                // Otherwise lower the expression is lowred (e.g., for function pointers).
                let callee_op = match &callee.kind {
                    ExprKind::Identifier(name) => Operand::Label(name.clone()),
                    _ => self.lower_expression(callee),
                };

                // Emit Param instructions, each as wide as the parameter it
                // is passed for.
                for (arg_op, &width) in arg_operands.into_iter().zip(&param_widths) {
                    self.emit(TACInstruction::new(
                        Opcode::Param,
                        width,
                        None,
                        Some(arg_op),
                        None,
                    ));
                }

                // Emit the Call instruction
                let ret_temp = self.fresh_temp();
                self.emit(TACInstruction::new(
                    Opcode::Call,
                    return_width,
                    Some(ret_temp.clone()),
                    Some(callee_op),
                    Some(Operand::ImmInt(args.len() as i64)),
                ));

                ret_temp
            }

            // arr[idx] — rvalue array element access
            ExprKind::Index { array, index } => {
                let base_var = self.lower_expression(array);
                let idx_op = self.lower_index(index);
                let dest = self.fresh_temp();

                // ArrayLoad: dest = base_var[idx_op]
                self.emit(TACInstruction::new(
                    Opcode::ArrayLoad,
                    self.width_of_expr(expr),
                    Some(dest.clone()),
                    Some(base_var),
                    Some(idx_op),
                ));

                dest
            }

            // -x -- negation, which is what subtracting `x` from zero is.
            // Written that way it needs no opcode of its own, and every pass
            // that already knows how to fold, simplify and select a
            // subtraction handles it as it stands.
            ExprKind::Unary(UnaryOp::Neg, inner) => {
                // The operand of a negation has the type of the whole
                // expression, so this converts nothing today; it is what C
                // says happens, and stays right if a narrower type is added.
                let width = self.width_of_expr(expr);
                let operand = self.lower_converted(inner, width);
                let dest = self.fresh_temp();

                self.emit(TACInstruction::new(
                    Opcode::Sub,
                    width,
                    Some(dest.clone()),
                    Some(Operand::ImmInt(0)),
                    Some(operand),
                ));

                dest
            }

            // !x -- one when `x` is zero and zero otherwise, which is exactly
            // what comparing it against zero answers. Like a negation it needs
            // no opcode of its own, so every pass that already folds and
            // selects an equality handles it as it stands.
            ExprKind::Unary(UnaryOp::Not, inner) => {
                // The comparison is made at the operand's own width; the
                // answer is the 0 or 1 of an `int` however wide that was.
                let width = self.width_of_expr(inner);
                let operand = self.lower_expression(inner);
                let dest = self.fresh_temp();

                self.emit(TACInstruction::new(
                    Opcode::Eq,
                    width,
                    Some(dest.clone()),
                    Some(operand),
                    Some(Operand::ImmInt(0)),
                ));

                dest
            }

            // &arr[i] -- the address of an array element, which is where
            // the element sits rather than where the array's name lives. The
            // backend addresses an element in one instruction, so the index
            // is handed to it as it stands instead of being turned into
            // arithmetic here.
            ExprKind::Unary(UnaryOp::AddressOf, inner)
                if matches!(inner.kind, ExprKind::Index { .. }) =>
            {
                let ExprKind::Index { array, index } = &inner.kind else {
                    unreachable!("the guard above matched an index expression")
                };

                let base_var = self.lower_expression(array);
                let idx_op = self.lower_index(index);
                let dest = self.fresh_temp();

                // ArrayAddr: dest = &base_var[idx_op], measured in elements of
                // the width the array holds.
                self.emit(TACInstruction::new(
                    Opcode::ArrayAddr,
                    self.width_of_expr(inner),
                    Some(dest.clone()),
                    Some(base_var),
                    Some(idx_op),
                ));

                dest
            }

            // &*p -- the address of a dereferenced pointer, which is the
            // pointer itself. C says the two operators cancel: neither is
            // evaluated, and the result is `p` (C11 6.5.3.2p3). Lowering it
            // literally would load through `p` and then ask for the address of
            // that loaded value, a temporary which has no address to give.
            ExprKind::Unary(UnaryOp::AddressOf, inner)
                if matches!(inner.kind, ExprKind::Unary(UnaryOp::Deref, _)) =>
            {
                let ExprKind::Unary(UnaryOp::Deref, pointer) = &inner.kind else {
                    unreachable!("the guard above matched a dereference")
                };

                self.lower_expression(pointer)
            }

            // &x -- address-of
            ExprKind::Unary(UnaryOp::AddressOf, inner) => {
                let inner_op = self.lower_expression(inner);
                let dest = self.fresh_temp();

                // AddrOf: dest = addr_of inner_op. An address is a full word
                // whatever it points at.
                self.emit(TACInstruction::new(
                    Opcode::AddrOf,
                    Width::Bits64,
                    Some(dest.clone()),
                    Some(inner_op),
                    None,
                ));

                dest
            }

            // *p -- rvalue dereference (read through pointer)
            ExprKind::Unary(UnaryOp::Deref, inner) => {
                let ptr_op = self.lower_expression(inner);
                let dest = self.fresh_temp();

                // Load: dest = *ptr_op, as wide as the object pointed at.
                self.emit(TACInstruction::new(
                    Opcode::Load,
                    self.width_of_expr(expr),
                    Some(dest.clone()),
                    Some(ptr_op),
                    None,
                ));

                dest
            }

            // A cast between integer types is exactly the conversion an
            // assignment would have performed, written out by hand.
            ExprKind::Cast(_, operand) => self.lower_converted(operand, self.width_of_expr(expr)),

            _ => panic!("Expr {:?} not supported in this lowering phase", expr.kind),
        }
    }

    /// Lower `lhs && rhs` or `lhs || rhs`.
    ///
    /// Both operators leave the right operand unevaluated when the left one
    /// already settles the answer, so neither can be a single instruction: the
    /// right operand needs a block of its own that the left one may jump past.
    /// The result is the 0 or 1 of an `int`, which is why the right operand is
    /// compared against zero unless it already answers with one -- see
    /// [`answers_zero_or_one`].
    ///
    /// ```text
    ///     <lhs>                        <lhs>
    ///     t = 0                        t = 1
    ///     if !lhs goto and_end         if lhs goto or_end
    ///     goto and_rhs                 goto or_rhs
    ///   and_rhs:                     or_rhs:
    ///     <rhs>                        <rhs>
    ///     t = rhs != 0                 t = rhs != 0
    ///     goto and_end                 goto or_end
    ///   and_end:                     or_end:
    /// ```
    ///
    /// # Arguments
    ///
    /// * `operator` - [`BinaryOp::LogicalAnd`] or [`BinaryOp::LogicalOr`]
    /// * `lhs` - the left operand, always evaluated
    /// * `rhs` - the right operand, evaluated only when the left one leaves
    ///   the answer open
    ///
    /// # Returns
    ///
    /// The temporary holding the operator's value, in the block control
    /// reaches whichever way it went.
    ///
    /// # Panics
    ///
    /// Panics if `operator` is any other binary operator.
    fn lower_short_circuit(&mut self, operator: BinaryOp, lhs: &Expr, rhs: &Expr) -> Operand {
        // Which left operand settles the answer, what the answer is then, and
        // what to call the blocks: `&&` stops at a false operand and is worth
        // 0, `||` stops at a true one and is worth 1.
        let (settles, settled_value, prefix) = match operator {
            BinaryOp::LogicalAnd => (Opcode::BranchIfNot, 0, "and"),
            BinaryOp::LogicalOr => (Opcode::BranchIf, 1, "or"),
            other => panic!("Compiler Bug: {:?} does not short-circuit", other),
        };

        let rhs_label = self.create_block(&format!("{prefix}_rhs"));
        let end_label = self.create_block(&format!("{prefix}_end"));

        let left_width = self.width_of_expr(lhs);
        let left = self.lower_expression(lhs);

        // The value the operator has when the left operand settles it, written
        // before the branch so that the path skipping the right operand
        // carries an answer. The result of a logical operator is an `int`.
        let result = self.fresh_temp();
        self.emit(TACInstruction::new(
            Opcode::Mov,
            Width::Bits32,
            Some(result.clone()),
            Some(Operand::ImmInt(settled_value)),
            None,
        ));

        // Lowering the left operand may itself have branched, so the block the
        // test belongs to is the one it left off in rather than the one it
        // started in.
        let tested = self.current_block.clone();
        self.emit(TACInstruction::new(
            settles,
            left_width,
            None,
            Some(left),
            Some(Operand::Label(end_label.clone())),
        ));
        self.add_edge(&tested, &end_label);

        self.emit(TACInstruction::transfer(
            Opcode::Jump,
            None,
            Some(Operand::Label(rhs_label.clone())),
            None,
        ));
        self.add_edge(&tested, &rhs_label);

        // Otherwise the answer is whether the right operand is non-zero,
        // which an operand that already answers 0 or 1 says by itself.
        self.set_current_block(rhs_label);
        let (opcode, width, zero) = match answers_zero_or_one(rhs) {
            true => (Opcode::Mov, Width::Bits32, None),
            false => (
                Opcode::Neq,
                self.width_of_expr(rhs),
                Some(Operand::ImmInt(0)),
            ),
        };
        let right = self.lower_expression(rhs);
        self.emit(TACInstruction::new(
            opcode,
            width,
            Some(result.clone()),
            Some(right),
            zero,
        ));

        let rhs_end = self.current_block.clone();
        self.emit(TACInstruction::transfer(
            Opcode::Jump,
            None,
            Some(Operand::Label(end_label.clone())),
            None,
        ));
        self.add_edge(&rhs_end, &end_label);

        self.set_current_block(end_label);
        result
    }

    /// Lower a subscript, which addresses memory and is therefore needed at
    /// full width however narrow the index expression's own type is.
    fn lower_index(&mut self, index: &Expr) -> Operand {
        self.lower_converted(index, Width::Bits64)
    }

    fn expect_lvalue_var(&self, expr: &Expr) -> NodeId {
        match &expr.kind {
            ExprKind::Identifier(_) => expr.id,
            _ => panic!("Expected assignable identifier lvalue, got {:?}", expr.kind),
        }
    }
}

/// Whether `expr` already evaluates to the 0 or 1 a logical operator answers
/// with, so that comparing it against zero would say nothing new.
///
/// Only the operators that C defines to yield 0 or 1 qualify. Anything else
/// answers conservatively: a `false` here costs one comparison, where a
/// wrongly optimistic `true` would let `a && 2` be 2.
///
/// # Examples
///
/// ```text
/// b < c   ->  true, so `a && b < c` is the comparison itself
/// f()     ->  false, so `a && f()` compares the returned value against zero
/// ```
fn answers_zero_or_one(expr: &Expr) -> bool {
    match &expr.kind {
        ExprKind::Binary(operator, _, _) => matches!(
            operator,
            BinaryOp::Eq
                | BinaryOp::Neq
                | BinaryOp::Lt
                | BinaryOp::Lte
                | BinaryOp::Gt
                | BinaryOp::Gte
                | BinaryOp::LogicalAnd
                | BinaryOp::LogicalOr
        ),
        ExprKind::Unary(operator, _) => *operator == UnaryOp::Not,
        // Everything else may evaluate to any value at all.
        ExprKind::Literal(_)
        | ExprKind::Identifier(_)
        | ExprKind::Call { .. }
        | ExprKind::Index { .. }
        | ExprKind::MemberAccess { .. }
        | ExprKind::Cast(_, _)
        | ExprKind::SizeOf(_) => false,
    }
}

/// The machine width a value of type `ty` is held at.
///
/// `char` and `int` are the narrow types; everything else the compiler can
/// lower -- `long int`, a pointer, the address an array name decays to -- is a
/// full machine word. `void` never holds anything, and is given the width a
/// plain copy uses so that a call to a void function still has a destination
/// the backend can name.
pub fn width_of(ty: &Type) -> Width {
    match ty {
        Type::Char(_) => Width::Bits8,
        Type::Int(_) => Width::Bits32,
        Type::Long(_) | Type::Pointer(_) | Type::Array(_, _) | Type::Void => Width::Bits64,
    }
}

/// How the bits of a value of type `ty` read.
///
/// This is what decides between the two instructions wherever the machine has
/// one of each: a widening that sign-extends or one that fills with zeroes, a
/// signed or an unsigned division, a signed or an unsigned comparison.
///
/// An address is unsigned -- there is no such thing as a negative one -- and
/// so are the array and pointer types that produce one. `void` holds nothing
/// to read either way.
pub fn sign_of(ty: &Type) -> Sign {
    match ty {
        Type::Char(sign) | Type::Int(sign) | Type::Long(sign) => *sign,
        Type::Pointer(_) | Type::Array(_, _) | Type::Void => Sign::Unsigned,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_type_is_held_at_the_width_its_size_calls_for() {
        // Arrange / Act / Assert: `sizeof(char)` is 1 and `sizeof(int)` is 4,
        // which is what makes a `char` array pack one element per byte. An
        // `unsigned` type is exactly as wide as its signed counterpart.
        assert_eq!(width_of(&Type::Char(Sign::Signed)).bytes(), 1);
        assert_eq!(width_of(&Type::Char(Sign::Unsigned)).bytes(), 1);
        assert_eq!(width_of(&Type::INT).bytes(), 4);
        assert_eq!(width_of(&Type::Int(Sign::Unsigned)).bytes(), 4);
        assert_eq!(width_of(&Type::Long(Sign::Signed)).bytes(), 8);
        assert_eq!(width_of(&Type::Long(Sign::Unsigned)).bytes(), 8);
        // A pointer and the address an array name decays to are full words
        // however narrow the object at the other end is.
        assert_eq!(width_of(&Type::Pointer(Box::new(Type::INT))).bytes(), 8);
        assert_eq!(
            width_of(&Type::Array(Box::new(Type::Char(Sign::Signed)), 3)).bytes(),
            8
        );
    }

    #[test]
    fn only_the_integer_types_carry_a_signedness_of_their_own() {
        // Arrange / Act / Assert
        assert_eq!(sign_of(&Type::Char(Sign::Unsigned)), Sign::Unsigned);
        assert_eq!(sign_of(&Type::INT), Sign::Signed);
        assert_eq!(sign_of(&Type::Long(Sign::Unsigned)), Sign::Unsigned);
        // An address is never negative, so the types that produce one read
        // unsigned.
        assert_eq!(sign_of(&Type::Pointer(Box::new(Type::INT))), Sign::Unsigned);
    }
}
