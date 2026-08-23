//! Responsible for desugering the AST into the IR
//!
//! What comes out is unoptimised three-address code. Optimisation happens
//! afterwards, on the SSA form the middle-end builds from it -- see
//! [`ssa::passes`](crate::middle::ssa::passes).

use std::collections::BTreeMap;
use std::collections::HashMap;

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
    /// each one takes: `int a[3]` is twelve bytes and `long int a[3]` is
    /// twenty-four.
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

                // if !cond -> end
                self.emit(TACInstruction::new(
                    Opcode::BranchIfNot,
                    width,
                    None,
                    Some(cond),
                    Some(Operand::Label(end_label.clone())),
                ));
                self.add_edge(&cond_label, &end_label);

                // else -> body
                self.emit(TACInstruction::transfer(
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

                    // if !cond -> end
                    self.emit(TACInstruction::new(
                        Opcode::BranchIfNot,
                        width,
                        None,
                        Some(cond),
                        Some(Operand::Label(end_label.clone())),
                    ));
                    self.add_edge(&cond_label, &end_label);
                }
                // An omitted condition is always true, so `for (;;)` falls
                // straight through to the body.

                // else -> body
                self.emit(TACInstruction::transfer(
                    Opcode::Jump,
                    None,
                    Some(Operand::Label(body_label.clone())),
                    None,
                ));
                self.add_edge(&cond_label, &body_label);

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
        let from = self.width_of_expr(expr);
        let value = self.lower_expression(expr);
        self.convert(value, from, target)
    }

    /// Emit the conversion of `value` from one integer width to another.
    ///
    /// Converting a value to the width it already has is what most of the
    /// program does, so that case emits nothing at all.
    fn convert(&mut self, value: Operand, from: Width, to: Width) -> Operand {
        if from == to {
            return value;
        }
        // A literal travels through the IR sign-extended, so its conversion is
        // a matter of writing it down differently rather than of running an
        // instruction.
        if let Operand::ImmInt(constant) = value {
            return Operand::ImmInt(to.narrow(constant));
        }

        let dest = self.fresh_temp();
        self.emit(TACInstruction::new(
            Opcode::Convert,
            to,
            Some(dest.clone()),
            Some(value),
            None,
        ));
        dest
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

            // Arithmetic + comparisons used by if/while conditions.
            ExprKind::Binary(op, lhs, rhs) => {
                // C's usual arithmetic conversions: both operands are brought
                // to the type they have in common, and the operation is
                // computed at its width. A comparison narrows again on its own
                // -- its result is the 0 or 1 of an `int` -- which is why the
                // width recorded here is the one the operands are compared at.
                let types = self.types;
                let width = width_of(&Type::common(types.expr(lhs.id), types.expr(rhs.id)));
                let l = self.lower_converted(lhs, width);
                let r = self.lower_converted(rhs, width);
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

/// The machine width a value of type `ty` is held at.
///
/// An `int` is the one narrow type there is; everything else the compiler can
/// lower -- `long int`, a pointer, the address an array name decays to -- is a
/// full machine word. `void` never holds anything, and is given the width a
/// plain copy uses so that a call to a void function still has a destination
/// the backend can name.
pub fn width_of(ty: &Type) -> Width {
    match ty {
        Type::Int => Width::Bits32,
        Type::Long | Type::Pointer(_) | Type::Array(_, _) | Type::Void => Width::Bits64,
    }
}
