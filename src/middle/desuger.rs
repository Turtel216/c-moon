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
    /// Maps variable id → the bytes each local aggregate occupies.
    /// Used by the backend to allocate stack space.
    ///
    /// An array and a struct are both objects several values live inside, so
    /// both need storage of their own rather than a register: `char a[3]` is
    /// three bytes, `int a[3]` twelve, and `struct Point { int x; int y; }` is
    /// eight. Being listed here is also what keeps a variable out of SSA
    /// form -- see [`promote`](crate::middle::ssa::promote).
    pub object_sizes: HashMap<usize, usize>,
    /// Maps variable id → the name it was declared with, for IR dumps.
    pub var_names: HashMap<usize, String>,
}

impl ProgramIr {
    pub fn new() -> Self {
        Self {
            functions: BTreeMap::new(),
            object_sizes: HashMap::new(),
            var_names: HashMap::new(),
        }
    }
}

/// Where an object sits, before its address has been computed.
///
/// A chain such as `s.inner.data[2]` names one object at one fixed distance
/// from something the compiler can already address. Walking the chain into a
/// single offset is what lets the whole of it cost the one instruction that
/// materialising the place emits, instead of an addition per link.
#[derive(Debug, Clone)]
enum Place {
    /// `offset` bytes into the frame storage of the aggregate variable `base`.
    Object { base: usize, offset: i64 },
    /// `offset` bytes past the address `base` holds.
    Pointee { base: Operand, offset: i64 },
}

impl Place {
    /// The same object seen `bytes` further in.
    fn offset_by(&self, bytes: i64) -> Self {
        match self {
            Place::Object { base, offset } => Place::Object {
                base: *base,
                offset: offset + bytes,
            },
            Place::Pointee { base, offset } => Place::Pointee {
                base: base.clone(),
                offset: offset + bytes,
            },
        }
    }
}

/// Where the two loop jumps go, for one enclosing loop.
///
/// `break` and `continue` name no label of their own: each one means a block
/// the loop's own lowering created, so the loop records them on its way in and
/// the jump reads them back off the innermost entry.
#[derive(Debug)]
struct LoopTargets {
    /// The block after the loop, which `break` jumps to.
    after: String,
    /// The block that leads back to the test -- the condition of a `while`,
    /// the step of a `for` -- which `continue` jumps to.
    next_iteration: String,
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
    /// Tracks the storage local aggregates need: var_id → bytes occupied.
    object_sizes: HashMap<usize, usize>,
    /// The loops enclosing the statement being lowered, innermost last.
    loops: Vec<LoopTargets>,
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
            object_sizes: HashMap::new(),
            loops: Vec::new(),
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

                // A struct definition declares no storage of its own: it fixes
                // a layout, which semantic analysis already recorded.
                DeclKind::Struct { .. } => {}

                DeclKind::Variable { .. } => {
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
        if self.current_block != exit {
            self.jump_to(&exit);
        }

        // Save the finished CFG into the Program
        if let Some(finished_cfg) = self.current_cfg.take() {
            self.program
                .functions
                .insert(name.to_string(), finished_cfg);
        }

        // Transfer the object storage metadata to ProgramIr
        for (var_id, size) in &self.object_sizes {
            self.program.object_sizes.insert(*var_id, *size);
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

    /// Emits an unconditional jump to `target`, and records the edge to it.
    ///
    /// The edge leaves whichever block is current, which is not always the one
    /// the construct started in: lowering a short-circuiting condition branches
    /// on its way to a value, so it ends somewhere else.
    fn jump_to(&mut self, target: &str) {
        self.emit(TACInstruction::transfer(
            Opcode::Jump,
            None,
            Some(Operand::Label(target.to_owned())),
            None,
        ));
        let from = self.current_block.clone();
        self.add_edge(&from, target);
    }

    /// Emits a conditional branch to `target`, and records the edge to it.
    ///
    /// Only the taken edge is recorded: control that does not branch falls out
    /// of the block into whatever comes next, which is an edge the caller adds
    /// itself.
    ///
    /// # Arguments
    ///
    /// * `opcode` - [`Opcode::BranchIf`] or [`Opcode::BranchIfNot`]
    /// * `width` - the width the condition is tested at
    /// * `condition` - the value whose truth decides the branch
    /// * `target` - the block reached when it does branch
    fn branch_to(&mut self, opcode: Opcode, width: Width, condition: Operand, target: &str) {
        self.emit(TACInstruction::new(
            opcode,
            width,
            None,
            Some(condition),
            Some(Operand::Label(target.to_owned())),
        ));
        let from = self.current_block.clone();
        self.add_edge(&from, target);
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

                // if !cond -> else, otherwise -> then
                self.branch_to(Opcode::BranchIfNot, width, cond, &else_label);
                self.jump_to(&then_label);

                // then branch
                self.set_current_block(then_label);
                self.lower_statement(then_branch);
                self.jump_to(&end_label);

                // else branch (or empty else)
                self.set_current_block(else_label);
                if let Some(e) = else_branch {
                    self.lower_statement(e);
                }
                self.jump_to(&end_label);

                self.set_current_block(end_label);
            }

            StmtKind::While { condition, body } => {
                let cond_label = self.create_block("while_cond");
                let body_label = self.create_block("while_body");
                let end_label = self.create_block("while_end");

                // preheader -> cond
                self.jump_to(&cond_label);

                // cond block. A short-circuiting condition branches on its way
                // to a value, so the test is emitted into the block it left
                // off in rather than the one it started in -- which is what
                // `branch_to` and `jump_to` take the edge from.
                self.set_current_block(cond_label.clone());
                let (cond, width) = self.lower_condition(condition);

                // if !cond -> end, else -> body
                self.branch_to(Opcode::BranchIfNot, width, cond, &end_label);
                self.jump_to(&body_label);

                // body block. `continue` re-tests the condition and `break`
                // leaves for good.
                self.set_current_block(body_label);
                self.lower_loop_body(body, &cond_label, &end_label);

                // back-edge body -> cond
                self.jump_to(&cond_label);

                // continue after loop
                self.set_current_block(end_label);
            }

            StmtKind::Block(items) => {
                for item in items {
                    match item {
                        BlockItem::Stmt(s) => self.lower_statement(s),
                        BlockItem::Decl(d) => self.lower_declaration(d),
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

                self.open_unreachable_block("unreachable_after_ret");
            }

            // `break` leaves the innermost loop and `continue` starts its next
            // iteration. Semantic analysis has already rejected either one
            // written outside a loop, so there is always an entry to read.
            StmtKind::Break => {
                let target = self.innermost_loop().after.clone();
                self.jump_to(&target);
                self.open_unreachable_block("unreachable_after_break");
            }

            StmtKind::Continue => {
                let target = self.innermost_loop().next_iteration.clone();
                self.jump_to(&target);
                self.open_unreachable_block("unreachable_after_continue");
            }

            // A `for` is lowered like a `while` with the init clause hoisted
            // into the preheader and the step appended to the body:
            //
            //   <init>; goto cond
            //   cond: if !condition goto end; goto body
            //   body: <body>; goto step
            //   step: <step>; goto cond
            //   end:
            //
            // The step gets a block of its own because `continue` has to run
            // it before the next test: jumping straight to the condition would
            // skip the `i = i + 1` of `for (i = 0; i < n; i = i + 1)` and loop
            // for ever. Merging it back into the body is the optimiser's job.
            StmtKind::For {
                init,
                condition,
                step,
                body,
            } => {
                let cond_label = self.create_block("for_cond");
                let body_label = self.create_block("for_body");
                let step_label = self.create_block("for_step");
                let end_label = self.create_block("for_end");

                // preheader: the init clause runs once, before the loop
                if let Some(init) = init {
                    self.lower_statement(init);
                }
                self.jump_to(&cond_label);

                // cond block
                self.set_current_block(cond_label.clone());
                if let Some(condition) = condition {
                    let (cond, width) = self.lower_condition(condition);

                    // if !cond -> end
                    self.branch_to(Opcode::BranchIfNot, width, cond, &end_label);
                }
                // An omitted condition is always true, so `for (;;)` falls
                // straight through to the body.

                // else -> body
                self.jump_to(&body_label);

                // body block. `continue` skips the rest of it and goes on to
                // the step, `break` leaves the loop for good.
                self.set_current_block(body_label);
                self.lower_loop_body(body, &step_label, &end_label);
                self.jump_to(&step_label);

                // step block, then the back-edge to the condition
                self.set_current_block(step_label);
                if let Some(step) = step {
                    let _ = self.lower_expression(step);
                }
                self.jump_to(&cond_label);

                // continue after loop
                self.set_current_block(end_label);
            }
        }
    }

    /// Lowers a loop's body with the blocks its `break` and `continue` mean.
    ///
    /// # Arguments
    ///
    /// * `body` - the loop body
    /// * `next_iteration` - where `continue` goes: the condition of a `while`,
    ///   the step of a `for`
    /// * `after` - the block following the loop, where `break` goes
    fn lower_loop_body(&mut self, body: &Stmt, next_iteration: &str, after: &str) {
        self.loops.push(LoopTargets {
            after: after.to_owned(),
            next_iteration: next_iteration.to_owned(),
        });
        self.lower_statement(body);
        self.loops.pop();
    }

    /// The loop a `break` or a `continue` acts on: the innermost one.
    ///
    /// # Panics
    ///
    /// Panics when there is no enclosing loop, which semantic analysis rejects
    /// long before lowering runs.
    fn innermost_loop(&self) -> &LoopTargets {
        self.loops
            .last()
            .expect("Compiler Bug: a jump outside any loop reached lowering")
    }

    /// Opens a fresh block for the statements after one that leaves for good.
    ///
    /// A `return`, a `break` and a `continue` all end their block, yet the
    /// statements written after them still have to be lowered somewhere. They
    /// go here, into a block nothing jumps to, which SSA construction deletes
    /// along with everything in it -- see
    /// [`retain_reachable`](crate::middle::ssa::Function::retain_reachable).
    fn open_unreachable_block(&mut self, prefix: &str) {
        let dead_block = self.create_block(prefix);
        self.set_current_block(dead_block);
    }

    /// Lower one declaration inside a block.
    ///
    /// An aggregate needs storage rather than code: recording its size is what
    /// makes the backend reserve frame space for it, and what keeps it out of
    /// SSA form. A scalar with an initializer is a single move.
    fn lower_declaration(&mut self, decl: &Decl) {
        // A struct definition inside a block declares no storage: the layout
        // it fixes was recorded by semantic analysis, and there is no variable
        // to bind.
        let DeclKind::Variable { initializer, .. } = &decl.kind else {
            return;
        };

        let variable = *self
            .res_map
            .decl_to_var
            .get(&decl.id)
            .expect("Compiler Bug: Renamer failed to map declaration");

        // The type table is copied out of `self` so that the object table can
        // be written while it is still being read; it outlives the borrow.
        let types = self.types;
        let declared = types.decl(decl.id);

        if declared.is_aggregate() {
            // The object lives entirely in the frame, so its declaration
            // reserves space rather than emitting an instruction.
            self.object_sizes
                .insert(variable, types.structs().size_of(declared));

            // Only a struct can be initialised as a whole; an array
            // initializer list is rejected by semantic analysis.
            if let Some(init) = initializer {
                let bytes = types.structs().size_of(declared);
                let destination = Place::Object {
                    base: variable,
                    offset: 0,
                };
                self.lower_object_copy(&destination, init, bytes);
            }
            return;
        }

        // A scalar without an initializer holds whatever was already there.
        let Some(init) = initializer else {
            return;
        };

        // With one, the value is converted to the declared type the way C
        // converts it.
        let width = width_of(declared);
        let rhs = self.lower_converted(init, width);
        self.emit(TACInstruction::new(
            Opcode::Mov,
            width,
            Some(Operand::Var(variable)),
            Some(rhs),
            None,
        ));
    }

    // ### Objects and their addresses ###

    /// The place the lvalue `expr` names.
    ///
    /// # Panics
    ///
    /// Panics for an expression that names no object. Semantic analysis
    /// accepts a member access, a subscript, a dereference and an identifier
    /// as lvalues and nothing else, so anything else here is a compiler bug.
    fn place_of(&mut self, expr: &Expr) -> Place {
        match &expr.kind {
            // A named aggregate is addressed through the frame storage it was
            // given; a scalar has no address until one is taken, which pins it
            // to a slot of its own.
            ExprKind::Identifier(_) => {
                let variable = self.variable_of(expr);
                match self.types.expr(expr.id).is_aggregate() {
                    true => Place::Object {
                        base: variable,
                        offset: 0,
                    },
                    false => Place::Pointee {
                        base: self.address_of_variable(variable),
                        offset: 0,
                    },
                }
            }

            // `*p` names whatever `p` points at.
            ExprKind::Unary(UnaryOp::Deref, pointer) => Place::Pointee {
                base: self.lower_expression(pointer),
                offset: 0,
            },

            // `s.m` sits a fixed distance into `s`; `p->m` the same distance
            // past what `p` points at.
            ExprKind::MemberAccess {
                base,
                member,
                is_arrow,
            } => {
                let offset = self.member_offset(base, member, *is_arrow);
                match is_arrow {
                    true => Place::Pointee {
                        base: self.lower_expression(base),
                        offset,
                    },
                    false => self.place_of(base).offset_by(offset),
                }
            }

            ExprKind::Index { array, index } => self.index_place(array, index),

            // An assignment is the one expression that names an object without
            // being an lvalue, and only a struct one does: lowering it copies
            // the object and hands back that object's address. That is what
            // makes `(a = b).x` read the member out of `a`.
            ExprKind::Binary(BinaryOp::Assign, _, _) if self.types.expr(expr.id).is_struct() => {
                Place::Pointee {
                    base: self.lower_expression(expr),
                    offset: 0,
                }
            }

            other => panic!("Compiler Bug: {:?} names no object", other),
        }
    }

    /// The place `array[index]` names.
    ///
    /// A constant index is a constant distance and folds into the offset the
    /// base is already reached by, so `s.items[2].x` is one address
    /// computation however deep it goes. A computed one is scaled into bytes
    /// and added, because an element of a struct type is not one of the sizes
    /// the machine can scale an index by.
    fn index_place(&mut self, array: &Expr, index: &Expr) -> Place {
        let types = self.types;
        let Type::Array(element, _) = types.expr(array.id) else {
            panic!("Compiler Bug: subscripted something that is not an array");
        };
        let size = types.structs().size_of(element) as i64;

        let base = self.place_of(array);
        if let ExprKind::Literal(Literal::Int(constant)) = index.kind {
            return base.offset_by(constant as i64 * size);
        }

        // The index counts elements and the address counts bytes, so one
        // multiplication converts between them. A one-byte element needs none.
        let counted = self.lower_index(index);
        let scaled = match size {
            1 => counted,
            _ => self.emit_binary(Opcode::Mul, Width::Bits64, counted, Operand::ImmInt(size)),
        };

        let address = self.address_of(&base);
        Place::Pointee {
            base: self.emit_binary(Opcode::Add, Width::Bits64, address, scaled),
            offset: 0,
        }
    }

    /// The address of the object `place` names.
    fn address_of(&mut self, place: &Place) -> Operand {
        match place {
            // Byte `n` of a named object is `n` bytes into its frame storage,
            // which the backend addresses in a single instruction: an element
            // address whose elements are bytes is exactly a byte offset.
            Place::Object { base, offset } => {
                let dest = self.fresh_temp();
                self.emit(TACInstruction::new(
                    Opcode::ArrayAddr,
                    Width::Bits8,
                    Some(dest.clone()),
                    Some(Operand::Var(*base)),
                    Some(Operand::ImmInt(*offset)),
                ));
                dest
            }

            // The object the pointer already points at needs no arithmetic.
            Place::Pointee { base, offset: 0 } => base.clone(),

            Place::Pointee { base, offset } => self.emit_binary(
                Opcode::Add,
                Width::Bits64,
                base.clone(),
                Operand::ImmInt(*offset),
            ),
        }
    }

    /// The address of the variable `variable`, which pins it to a frame slot.
    fn address_of_variable(&mut self, variable: usize) -> Operand {
        let dest = self.fresh_temp();
        // An address is a full word whatever it points at.
        self.emit(TACInstruction::new(
            Opcode::AddrOf,
            Width::Bits64,
            Some(dest.clone()),
            Some(Operand::Var(variable)),
            None,
        ));
        dest
    }

    /// Read the `width`-wide value at `place`.
    fn load_from(&mut self, place: &Place, width: Width) -> Operand {
        let address = self.address_of(place);
        let dest = self.fresh_temp();
        self.emit(TACInstruction::new(
            Opcode::Load,
            width,
            Some(dest.clone()),
            Some(address),
            None,
        ));
        dest
    }

    /// Write the `width`-wide `value` at `place`.
    fn store_into(&mut self, place: &Place, value: Operand, width: Width) {
        let address = self.address_of(place);
        self.emit(TACInstruction::new(
            Opcode::Store,
            width,
            None,
            Some(address),
            Some(value),
        ));
    }

    /// Copy the whole of the object `source` names into `destination`.
    ///
    /// C defines a struct assignment as copying the object's representation,
    /// and how much of it there is is known here, so the copy is unrolled into
    /// the widest moves that fit rather than left to a loop. Three widths are
    /// enough: a struct's size is a sum of its members' sizes and the padding
    /// between them, and every one of those is a multiple of one of the three.
    ///
    /// # Arguments
    ///
    /// * `destination` - the object written
    /// * `source` - the expression naming the object read
    /// * `bytes` - the size of both, which are of the same type
    fn lower_object_copy(&mut self, destination: &Place, source: &Expr, bytes: usize) {
        let source = self.place_of(source);

        let mut copied = 0;
        while copied < bytes {
            let width = widest_chunk(bytes - copied);
            let value = self.load_from(&source.offset_by(copied as i64), width);
            self.store_into(&destination.offset_by(copied as i64), value, width);
            copied += width.bytes() as usize;
        }
    }

    /// The byte offset of `member` within the struct `base` names.
    ///
    /// # Arguments
    ///
    /// * `base` - the struct, or the pointer to it when `is_arrow`
    /// * `member` - the name written after the operator
    /// * `is_arrow` - whether it was written `->` rather than `.`
    ///
    /// # Panics
    ///
    /// Panics unless `base` has the struct type the operator requires and the
    /// struct has such a member, both of which semantic analysis established.
    fn member_offset(&self, base: &Expr, member: &str, is_arrow: bool) -> i64 {
        let base_ty = self.types.expr(base.id);
        let struct_ty = match (is_arrow, base_ty) {
            (false, ty) => ty,
            (true, Type::Pointer(pointee)) => pointee.as_ref(),
            (true, other) => panic!("Compiler Bug: `->` applied to a {:?}", other),
        };

        let Type::Struct(tag) = struct_ty else {
            panic!("Compiler Bug: a member was read from a {:?}", struct_ty);
        };
        self.types
            .structs()
            .layout(tag)
            .expect("Compiler Bug: an object of an undefined struct type exists")
            .member(member)
            .expect("Compiler Bug: a member semantic analysis accepted is missing")
            .offset as i64
    }

    /// The variable the identifier expression `expr` resolves to.
    fn variable_of(&self, expr: &Expr) -> usize {
        *self
            .res_map
            .expr_to_var
            .get(&expr.id)
            .expect("Compiler Bug: Renamer failed to map an identifier")
    }

    /// Whether `expr` is `a[i]` on a named array of scalars.
    ///
    /// That is the shape the backend reaches in a single instruction, with the
    /// index scaled by the element's own width. Anything else -- an element of
    /// a struct type, or an array reached through a member -- goes through an
    /// address instead.
    fn is_direct_element(&self, expr: &Expr) -> bool {
        let ExprKind::Index { array, .. } = &expr.kind else {
            return false;
        };
        matches!(array.kind, ExprKind::Identifier(_)) && !self.types.expr(expr.id).is_aggregate()
    }

    /// Emit `dest = lhs <op> rhs` into a fresh temporary and return it.
    fn emit_binary(&mut self, opcode: Opcode, width: Width, lhs: Operand, rhs: Operand) -> Operand {
        let dest = self.fresh_temp();
        self.emit(TACInstruction::new(
            opcode,
            width,
            Some(dest.clone()),
            Some(lhs),
            Some(rhs),
        ));
        dest
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

            // `a = b` between structs. C copies the whole representation,
            // so the assignment is a fixed-size copy rather than a move of one
            // value -- however the two objects are named.
            ExprKind::Binary(BinaryOp::Assign, lhs, rhs) if self.types.expr(lhs.id).is_struct() => {
                let types = self.types;
                let bytes = types.structs().size_of(types.expr(lhs.id));
                let destination = self.place_of(lhs);
                self.lower_object_copy(&destination, rhs, bytes);

                // The value of a struct assignment is the object it wrote,
                // which the lowering passes around as that object's address.
                self.address_of(&destination)
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

            // arr[idx] = rhs, on a named array of scalars
            ExprKind::Binary(BinaryOp::Assign, lhs, rhs) if self.is_direct_element(lhs) => {
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

            // `s.m = rhs`, `p->m = rhs`, `s.items[i].m = rhs` -- a store at
            // the member's own address.
            ExprKind::Binary(BinaryOp::Assign, lhs, rhs)
                if matches!(
                    lhs.kind,
                    ExprKind::MemberAccess { .. } | ExprKind::Index { .. }
                ) =>
            {
                // The object written is as wide as its declared type, and the
                // value is converted to it before the store.
                let width = self.width_of_expr(lhs);
                let value = self.lower_converted(rhs, width);
                let place = self.place_of(lhs);
                self.store_into(&place, value.clone(), width);
                value
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

            // Arithmetic, bitwise and comparisons used by if/while conditions.
            ExprKind::Binary(op, lhs, rhs) => {
                // C's usual arithmetic conversions: both operands are brought
                // to the type they have in common, and the operation is
                // computed at its width. A comparison narrows again on its own
                // -- its result is the 0 or 1 of an `int` -- which is why the
                // width recorded here is the one the operands are compared at.
                //
                // A shift is the exception: its operands are promoted apart
                // from each other and the answer has the left one's type, so
                // that is the type the analyzer gave the whole expression.
                // The count is converted to it as well, which costs nothing --
                // only its low bits are ever read -- and leaves every binary
                // instruction in the IR with both operands at one width.
                let types = self.types;
                let operands_ty = match op {
                    BinaryOp::Shl | BinaryOp::Shr => types.expr(expr.id).clone(),
                    _ => Type::common(types.expr(lhs.id), types.expr(rhs.id)),
                };
                let (width, sign) = (width_of(&operands_ty), sign_of(&operands_ty));
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
                    BinaryOp::BitAnd => Opcode::And,
                    BinaryOp::BitOr => Opcode::Or,
                    BinaryOp::BitXor => Opcode::Xor,
                    BinaryOp::Shl => Opcode::Shl,
                    BinaryOp::Shr => Opcode::Shr(sign),
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

            // arr[idx] -- rvalue element of a named array of scalars
            ExprKind::Index { array, index } if self.is_direct_element(expr) => {
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

            // `s.m`, `p->m`, `s.items[i]` -- a read of an object the compiler
            // reaches through its address rather than by name.
            ExprKind::MemberAccess { .. } | ExprKind::Index { .. } => {
                let width = self.width_of_expr(expr);
                let place = self.place_of(expr);
                self.load_from(&place, width)
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

            // ~x -- every bit of `x` the other way round, which is what
            // exclusive-or against a mask of ones does. Like a negation and a
            // logical not it therefore needs no opcode of its own, and every
            // pass that already folds, simplifies and selects an exclusive-or
            // handles it as it stands.
            ExprKind::Unary(UnaryOp::BitNot, inner) => {
                // The operand is promoted before the operator sees it, so the
                // mask is as wide as the promoted type: -1 is the constant
                // whose low bits are ones at every width.
                let width = self.width_of_expr(expr);
                let operand = self.lower_converted(inner, width);
                let dest = self.fresh_temp();

                self.emit(TACInstruction::new(
                    Opcode::Xor,
                    width,
                    Some(dest.clone()),
                    Some(operand),
                    Some(Operand::ImmInt(-1)),
                ));

                dest
            }

            // &arr[i] -- the address of an array element, which is where
            // the element sits rather than where the array's name lives. The
            // backend addresses an element in one instruction, so the index
            // is handed to it as it stands instead of being turned into
            // arithmetic here.
            ExprKind::Unary(UnaryOp::AddressOf, inner) if self.is_direct_element(inner) => {
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

            // `&x`, `&s`, `&s.m` -- the address of whatever object the
            // operand names. A scalar is pinned to a frame slot so that it has
            // one; an aggregate already has storage, and a member of one sits
            // a known distance into it.
            ExprKind::Unary(UnaryOp::AddressOf, inner) => {
                let place = self.place_of(inner);
                self.address_of(&place)
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
        self.branch_to(settles, left_width, left, &end_label);
        self.jump_to(&rhs_label);

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

        self.jump_to(&end_label);

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

/// The widest move that fits in `remaining` bytes.
///
/// Used to unroll an object copy: the copy takes machine words until fewer
/// than eight bytes are left, then an `int` at a time, then single bytes, so it
/// never reads or writes past the object it is copying.
const fn widest_chunk(remaining: usize) -> Width {
    match remaining {
        0..=3 => Width::Bits8,
        4..=7 => Width::Bits32,
        _ => Width::Bits64,
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
        // A struct is several values at once, which no width describes. Every
        // read of one is a read of a member, at that member's width, and a
        // copy of a whole one is made a chunk at a time by `widest_chunk`.
        Type::Struct(_) => panic!("Compiler Bug: a struct is not held in a register"),
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
        // Nothing ever reads a whole struct as a number; see `width_of`.
        Type::Struct(_) => panic!("Compiler Bug: a struct has no bits to read"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::frontend::lexer::Lexer;
    use crate::frontend::parser::Parser;
    use crate::frontend::renamer::resolve_names;
    use crate::frontend::semantic::SemanticAnalyzer;

    /// Lowers an accepted translation unit and returns the IR of `main`.
    fn lower_main(source: &str) -> CFG {
        let mut parser = Parser::from_lexer(Lexer::new(source)).expect("lexing should succeed");
        let (decls, parse_errors) = parser.parse_translation_unit();
        assert!(parse_errors.is_empty(), "unexpected: {parse_errors:?}");

        let mut analyzer = SemanticAnalyzer::new();
        let errors = analyzer.analyze_program(&decls);
        assert!(errors.is_empty(), "unexpected: {errors:?}");

        let resolution = resolve_names(&decls).expect("name resolution should succeed");
        let types = analyzer.into_types();
        let program = LoweringContext::new(&resolution, &types).lower_program(&decls);

        program.functions["main"].clone()
    }

    /// The labels the block `label` jumps or branches to.
    fn successors_of(cfg: &CFG, label: &str) -> Vec<String> {
        cfg.blocks[label].successors.clone()
    }

    /// The one instruction of `cfg` with this opcode.
    fn only_instruction(cfg: &CFG, opcode: Opcode) -> TACInstruction {
        let mut matches = cfg
            .blocks
            .values()
            .flat_map(|block| &block.instructions)
            .filter(|instr| instr.opcode == opcode);
        let found = matches
            .next()
            .unwrap_or_else(|| panic!("no `{opcode:?}` instruction"));
        assert!(matches.next().is_none(), "more than one `{opcode:?}`");
        found.clone()
    }

    /// The one block whose label starts with `prefix`.
    fn block_named(cfg: &CFG, prefix: &str) -> String {
        let mut matches = cfg.blocks.keys().filter(|label| label.starts_with(prefix));
        let found = matches
            .next()
            .unwrap_or_else(|| panic!("no `{prefix}` block"));
        assert!(matches.next().is_none(), "more than one `{prefix}` block");
        found.clone()
    }

    #[test]
    fn a_break_leaves_for_the_block_after_the_loop() {
        // Arrange / Act
        let cfg = lower_main("int main() { while (1) { break; } return 0; }");

        // Assert: the body's only successor is the block past the loop, so
        // nothing takes the back-edge to the condition on that path.
        let body = block_named(&cfg, "while_body");
        assert_eq!(
            successors_of(&cfg, &body),
            vec![block_named(&cfg, "while_end")]
        );
    }

    #[test]
    fn a_continue_in_a_for_leaves_for_the_step_rather_than_the_condition() {
        // Arrange / Act: the `continue` is the whole body, so the body block
        // is where it jumps from.
        let cfg =
            lower_main("int main() { for (int i = 0; i < 1; i = i + 1) continue; return 0; }");

        // Assert: jumping to the condition instead would skip `i = i + 1` and
        // spin for ever.
        let body = block_named(&cfg, "for_body");
        assert_eq!(
            successors_of(&cfg, &body),
            vec![block_named(&cfg, "for_step")]
        );
    }

    #[test]
    fn a_jump_acts_on_the_innermost_loop_only() {
        // Arrange / Act
        let cfg = lower_main(
            "int main() {
                 while (1) { while (1) { break; } }
                 return 0;
             }",
        );

        // Assert: the inner loop is the one whose condition a body block
        // branches into -- the outer body is its preheader -- and the `break`
        // in that inner body leaves for that loop's exit, not the outer one's.
        let inner_cond = cfg
            .blocks
            .values()
            .find(|block| {
                block.label.starts_with("while_cond")
                    && block
                        .predecessors
                        .iter()
                        .any(|pred| pred.starts_with("while_body"))
            })
            .expect("the inner loop's condition is entered from the outer body")
            .label
            .clone();

        let target = |prefix: &str| {
            successors_of(&cfg, &inner_cond)
                .into_iter()
                .find(|label| label.starts_with(prefix))
                .unwrap_or_else(|| panic!("the condition reaches a `{prefix}` block"))
        };
        assert_eq!(
            successors_of(&cfg, &target("while_body")),
            vec![target("while_end")]
        );
    }

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
    fn an_object_copy_takes_the_widest_move_that_fits() {
        // Arrange / Act / Assert: eight bytes at a time until fewer than eight
        // are left, then four, then one -- so a twelve-byte struct is copied
        // by a word and an `int`, and never a byte past its end.
        assert_eq!(widest_chunk(12), Width::Bits64);
        assert_eq!(widest_chunk(12 - 8), Width::Bits32);
        assert_eq!(widest_chunk(8), Width::Bits64);
        assert_eq!(widest_chunk(7), Width::Bits32);
        assert_eq!(widest_chunk(3), Width::Bits8);
        assert_eq!(widest_chunk(1), Width::Bits8);
    }

    #[test]
    fn a_shift_is_as_wide_as_its_left_operand_alone() {
        // Arrange / Act: a count wider than the value being shifted, which
        // under the usual arithmetic conversions would have dragged the whole
        // expression up to 64 bits.
        let cfg = lower_main("int main() { int a = 2; long int c = 1; return a << c; }");

        // Assert: C promotes a shift's operands apart from each other and
        // gives the answer the left one's type, so this shifts an `int`. The
        // count comes down to that width with it, which leaves both operands
        // of every binary instruction in the IR at one width.
        let shift = only_instruction(&cfg, Opcode::Shl);
        assert_eq!(shift.width, Width::Bits32);
        assert_eq!(
            only_instruction(
                &cfg,
                Opcode::Convert {
                    from: Width::Bits64,
                    sign: Sign::Signed
                }
            )
            .width,
            Width::Bits32
        );
    }

    #[test]
    fn a_complement_is_an_exclusive_or_against_a_mask_of_ones() {
        // Arrange / Act
        let cfg = lower_main("int main() { int a = 2; return ~a; }");

        // Assert: `~a` needs no opcode of its own, and -1 is the constant
        // whose low bits are ones at every width.
        let complement = only_instruction(&cfg, Opcode::Xor);
        assert_eq!(complement.width, Width::Bits32);
        assert_eq!(complement.arg2, Some(Operand::ImmInt(-1)));
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
