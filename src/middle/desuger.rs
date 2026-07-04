//! Responsible for desugering the AST into the IR
//!
//! The Optimizations that can be applied are:
//!   - Constant folding
//!   - Algebraic simplification
//!   - Constant propagation
//!   - Dead code elimination
//!   - Unreachable code elimination

use std::collections::BTreeMap;
use std::collections::HashMap;

use crate::frontend::ast::{
    BinaryOp, BlockItem, Decl, DeclKind, Expr, ExprKind, Literal, Stmt, StmtKind, UnaryOp,
};
use crate::frontend::renamer::ResolutionMap;
use crate::middle::ir::*;

/// Complete program IR representation
#[derive(Debug, Clone)]
pub struct ProgramIr {
    /// Program Functions
    pub functions: BTreeMap<String, CFG>,
    /// Maps variable id → array element count for each local array.
    /// Used by the backend to allocate stack space.
    pub array_sizes: HashMap<usize, usize>,
}

impl ProgramIr {
    pub fn new() -> Self {
        Self {
            functions: BTreeMap::new(),
            array_sizes: HashMap::new(),
        }
    }

    /// Run the optmizations passes on the Program IR.
    /// The optimizations applied to each ``CFG`` inside the ``ProgramIr`` are:
    ///   - Constant folding
    ///   - Algebraic simplification
    ///   - Constant propagation
    ///   - Dead code elimination
    ///   - Unreachable code elimination
    pub fn optimize(&mut self) -> () {
        for (_, cfg) in self.functions.iter_mut() {
            while !cfg.run_optimizations() {}
        }
    }
}

/// Context for AST to IR desugering
pub struct LoweringContext<'a> {
    /// Complete ProgramIr
    pub program: ProgramIr,
    /// Resolution map used for conflicting identifiers
    res_map: &'a ResolutionMap,
    /// Current ``CFG`` being lowered
    current_cfg: Option<CFG>,
    current_block: String,
    temp_counter: usize,
    label_counter: usize,
    /// Tracks local array sizes: var_id → element count.
    array_sizes: HashMap<usize, usize>,
}

impl<'a> LoweringContext<'a> {
    pub fn new(res_map: &'a ResolutionMap) -> Self {
        Self {
            program: ProgramIr::new(),
            res_map,
            current_cfg: None,
            current_block: String::new(),
            temp_counter: 0,
            label_counter: 0,
            array_sizes: HashMap::new(),
        }
    }

    /// Desuger a list of AST ``Decl`` into the ``ProgramIr``
    pub fn lower_program(mut self, decls: &[Decl]) -> ProgramIr {
        for decl in decls {
            match &decl.kind {
                DeclKind::Function {
                    name, body, params, ..
                } => {
                    let bod = body.clone().unwrap(); // TODO: Fix unsafe unwrap and clone

                    // Extract just the parameter names as a Vec<String>
                    let param_names: Vec<usize> =
                        params
                            .iter()
                            .map(|p| {
                                *self.res_map.decl_to_var.get(&p.id).expect(
                                    "Compiler Bug: Ranamer failed to map function parameters",
                                )
                            })
                            .collect();

                    self.lower_function(name, &param_names, &bod);
                }
                _ => {
                    todo!("Global variables not implemented yet")
                }
            }
        }
        self.program
    }

    fn lower_function(&mut self, name: &str, params: &[usize], body: &Stmt) {
        // Setup a new CFG for this function
        let entry = format!("{}_entry", name);
        let exit = format!("{}_exit", name);

        let mut cfg = CFG::new(entry.clone(), exit.clone());
        cfg.add_block(BasicBlock::new(entry.clone()));
        cfg.add_block(BasicBlock::new(exit.clone()));

        self.current_cfg = Some(cfg);
        self.current_block = entry.clone();

        // Bind parameters to local variables
        for (index, param_id) in params.iter().enumerate() {
            self.emit(TACInstruction::new(
                Opcode::GetParam,
                Some(Operand::Var(*param_id)), // dest: local variable
                Some(Operand::ImmInt(index as i64)), // arg1: parameter index
                None,
            ));
        }

        // Lower the body
        self.lower_statement(body);

        // Fallthrough to exit if the last block didn't explicitly return
        let cur = self.current_block.clone();
        if cur != exit {
            self.emit(TACInstruction::new(
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

                let cond = self.lower_expression(condition);

                let cur = self.current_block.clone();
                // if !cond -> else
                self.emit(TACInstruction::new(
                    Opcode::BranchIfNot,
                    None,
                    Some(cond),
                    Some(Operand::Label(else_label.clone())),
                ));
                self.add_edge(&cur, &else_label);

                // otherwise -> then
                self.emit(TACInstruction::new(
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
                self.emit(TACInstruction::new(
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
                self.emit(TACInstruction::new(
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
                self.emit(TACInstruction::new(
                    Opcode::Jump,
                    None,
                    Some(Operand::Label(cond_label.clone())),
                    None,
                ));
                self.add_edge(&preheader, &cond_label);

                // cond block
                self.set_current_block(cond_label.clone());
                let cond = self.lower_expression(condition);

                // if !cond -> end
                self.emit(TACInstruction::new(
                    Opcode::BranchIfNot,
                    None,
                    Some(cond),
                    Some(Operand::Label(end_label.clone())),
                ));
                self.add_edge(&cond_label, &end_label);

                // else -> body
                self.emit(TACInstruction::new(
                    Opcode::Jump,
                    None,
                    Some(Operand::Label(body_label.clone())),
                    None,
                ));
                self.add_edge(&cond_label, &body_label);

                // body block
                self.set_current_block(body_label.clone());
                self.lower_statement(body);

                // back-edge body -> cond
                let body_end = self.current_block.clone();
                self.emit(TACInstruction::new(
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

                            if let DeclKind::Variable {
                                ty: crate::frontend::ast::CType::Array(_, Some(size)),
                                ..
                            } = &d.kind
                            {
                                // Record the array size for backend stack allocation.
                                // No TAC instructions needed — the array lives
                                // entirely on the stack.
                                self.array_sizes.insert(*stmt_id, *size);
                            } else if let DeclKind::Variable {
                                initializer: Some(init),
                                ..
                            } = &d.kind
                            {
                                // Scalar variable with initializer.
                                let rhs = self.lower_expression(init);
                                self.emit(TACInstruction::new(
                                    Opcode::Mov,
                                    Some(Operand::Var(*stmt_id)),
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
                let ret_val = if let Some(expr) = expr_opt {
                    Some(self.lower_expression(expr))
                } else {
                    None
                };

                self.emit(TACInstruction::new(Opcode::Ret, None, ret_val, None));

                let exit_label = self.current_cfg.as_ref().unwrap().exit.clone();

                let cur = self.current_block.clone();
                self.add_edge(&cur, &exit_label);

                let dead_block = self.create_block("unreachable_after_ret");
                self.set_current_block(dead_block);
            }

            // Not in current scope;
            StmtKind::For { .. } => {
                todo!("For loops are not implemented yet")
            }
        }
    }

    fn lower_expression(&mut self, expr: &Expr) -> Operand {
        match &expr.kind {
            ExprKind::Literal(Literal::Int(v)) => Operand::ImmInt(*v),

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
                let rhs_op = self.lower_expression(rhs);

                // Extract the pointer expression from the LHS dereference.
                let ptr_expr = match &lhs.kind {
                    ExprKind::Unary(UnaryOp::Deref, inner) => inner,
                    _ => unreachable!(),
                };

                let addr_op = self.lower_expression(ptr_expr);

                // Store: *addr_op = rhs_op
                self.emit(TACInstruction::new(
                    Opcode::Store,
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
                let rhs_op = self.lower_expression(rhs);

                // Extract the array base variable and index from the LHS.
                let (array_base, index_expr) = match &lhs.kind {
                    ExprKind::Index { array, index } => (array, index),
                    _ => unreachable!(),
                };

                let base_var = self.lower_expression(array_base);
                let idx_op = self.lower_expression(index_expr);

                // ArrayStore: dest=base_var, arg1=index, arg2=value
                self.emit(TACInstruction::new(
                    Opcode::ArrayStore,
                    Some(base_var.clone()),
                    Some(idx_op),
                    Some(rhs_op.clone()),
                ));

                rhs_op
            }

            // x = rhs
            ExprKind::Binary(BinaryOp::Assign, lhs, rhs) => {
                let rhs_op = self.lower_expression(rhs);
                let lhs_var = self.expect_lvalue_var(lhs);

                let lhs_id = self
                    .res_map
                    .expr_to_var
                    .get(&lhs_var)
                    .expect("Compiler Bug: Renamer failed to map lhs of expression");

                self.emit(TACInstruction::new(
                    Opcode::Mov,
                    Some(Operand::Var(*lhs_id)),
                    Some(rhs_op.clone()),
                    None,
                ));
                Operand::Var(*lhs_id)
            }

            // Arithmetic + comparisons used by if/while conditions.
            ExprKind::Binary(op, lhs, rhs) => {
                let l = self.lower_expression(lhs);
                let r = self.lower_expression(rhs);
                let t = self.fresh_temp();

                let opcode = match op {
                    BinaryOp::Add => Opcode::Add,
                    BinaryOp::Sub => Opcode::Sub,
                    BinaryOp::Mul => Opcode::Mul,
                    BinaryOp::Div => Opcode::Div,
                    BinaryOp::Eq => Opcode::Eq,
                    BinaryOp::Neq => Opcode::Neq,
                    BinaryOp::Lt => Opcode::Lt,
                    BinaryOp::Lte => Opcode::Lte,
                    BinaryOp::Gt => Opcode::Gt,
                    BinaryOp::Gte => Opcode::Gte,
                    _ => panic!("Binary op {:?} not supported in this lowering phase", op),
                };

                self.emit(TACInstruction::new(
                    opcode,
                    Some(t.clone()),
                    Some(l),
                    Some(r),
                ));
                t
            }

            ExprKind::Call { callee, args } => {
                // Lower arguments
                let mut arg_operands = Vec::with_capacity(args.len());
                for arg in args {
                    arg_operands.push(self.lower_expression(arg));
                }

                // Determine the target of the call.
                // If it's a direct identifier, its treated as a static Label.
                // Otherwise lower the expression is lowred (e.g., for function pointers).
                let callee_op = match &callee.kind {
                    ExprKind::Identifier(name) => Operand::Label(name.clone()),
                    _ => self.lower_expression(callee),
                };

                // Emit Param instructions
                for arg_op in arg_operands {
                    self.emit(TACInstruction::new(Opcode::Param, None, Some(arg_op), None));
                }

                // Emit the Call instruction
                let ret_temp = self.fresh_temp();
                self.emit(TACInstruction::new(
                    Opcode::Call,
                    Some(ret_temp.clone()),
                    Some(callee_op),
                    Some(Operand::ImmInt(args.len() as i64)),
                ));

                ret_temp
            }

            // arr[idx] — rvalue array element access
            ExprKind::Index { array, index } => {
                let base_var = self.lower_expression(array);
                let idx_op = self.lower_expression(index);
                let dest = self.fresh_temp();

                // ArrayLoad: dest = base_var[idx_op]
                self.emit(TACInstruction::new(
                    Opcode::ArrayLoad,
                    Some(dest.clone()),
                    Some(base_var),
                    Some(idx_op),
                ));

                dest
            }

            // &x -- address-of
            ExprKind::Unary(UnaryOp::AddressOf, inner) => {
                let inner_op = self.lower_expression(inner);
                let dest = self.fresh_temp();

                // AddrOf: dest = addr_of inner_op
                self.emit(TACInstruction::new(
                    Opcode::AddrOf,
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

                // Load: dest = *ptr_op
                self.emit(TACInstruction::new(
                    Opcode::Load,
                    Some(dest.clone()),
                    Some(ptr_op),
                    None,
                ));

                dest
            }

            _ => panic!("Expr {:?} not supported in this lowering phase", expr.kind),
        }
    }

    fn expect_lvalue_var(&self, expr: &Expr) -> u32 {
        match &expr.kind {
            ExprKind::Identifier(_) => expr.id,
            _ => panic!("Expected assignable identifier lvalue, got {:?}", expr.kind),
        }
    }
}
