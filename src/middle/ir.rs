//! Intermediate Representation used for middle-end optimizations and code generation

use std::collections::{BTreeMap, HashMap, HashSet};

// ### TAC IR ###

/// TAC Operand
#[derive(Debug, Clone, PartialEq, Hash, Eq)]
pub enum Operand {
    /// TAC Variable
    Var(usize),
    /// TAC Temporary Variable
    Temp(String),
    /// TAC Ineger literal
    ImmInt(i64),
    /// TAC Label
    Label(String),
}

/// TAC Opcode
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Opcode {
    // Arithmetic
    /// TAC Addition e.g. %r1 + %r2
    Add,
    /// TAC subste.g. %r1 - %r2
    Sub,
    /// TAC Multiplication e.g. %r1 * %r2
    Mul,
    /// TAC Divition e.g. %r1 / %r2
    Div,

    // Relational / equality (result is 0/1)
    /// TAC Equalality e.g. %r1 == %r2
    Eq,
    /// TAC e.g. %r1 != %r2
    Neq,
    /// TAC Less then operator e.g. %r1 < %r2
    Lt,
    /// TAC Lees then or equal operator e.g. %r1 <= %r2
    Lte,
    /// TAC Greater then operator e.g. %r1 > %r2
    Gt,
    /// TAC Greater then or equal operator e.g. %r1 >= %r2
    Gte,

    // Data movement
    /// TAC Move operator e.g. dest = arg1
    Mov,

    // Control flow
    /// TAC Jump instruction goto arg1(label)
    Jump,
    /// TAC If branch if arg1 != 0 goto arg2(label)
    BranchIf,
    /// TAC If not branch if arg1 == 0 goto arg2(label)
    BranchIfNot,

    // Function calls and returns
    /// TAC Instruction to pass arguments to functions as parameters. pass arg1
    Param,
    /// TAC Instruction function calls. dest = call arg1 (func label), arg2 (number of args)
    Call,
    /// TAC Return instruction e.g. ret arg1
    Ret,
    /// Get incoming parameter at index e.g. dest = get_param 0
    GetParam,

    /// Store a value into an array element.
    /// dest = base array var, arg1 = index, arg2 = value
    ArrayStore,

    /// Load a value from an array element.
    /// dest = destination, arg1 = base array var, arg2 = index
    ArrayLoad,

    /// Load a value through a pointer: dest = *arg1
    Load,
    /// Store a value through a pointer: *arg1 = arg2
    Store,
    /// Take address of a variable: dest = addr_of arg1(Var)
    AddrOf,
}

/// TAC Instruction representation
#[derive(Debug, Clone, PartialEq)]
pub struct TACInstruction {
    /// Instruction operation
    pub opcode: Opcode,
    /// Instruction destination e.g. dest = 1 + 1
    pub dest: Option<Operand>,
    /// Instuctions first argument
    pub arg1: Option<Operand>,
    /// Instuctions first argument
    pub arg2: Option<Operand>,
}

impl TACInstruction {
    pub fn new(
        opcode: Opcode,
        dest: Option<Operand>,
        arg1: Option<Operand>,
        arg2: Option<Operand>,
    ) -> Self {
        Self {
            opcode,
            dest,
            arg1,
            arg2,
        }
    }

    /// Attempts to fold constant operands in place.
    /// Returns `true` if the instruction was successfully folded.
    pub fn fold_constants(&mut self) -> bool {
        // We only care about instructions where both arguments are immediate integers.
        let (val1, val2) = match (&self.arg1, &self.arg2) {
            (Some(Operand::ImmInt(v1)), Some(Operand::ImmInt(v2))) => (*v1, *v2),
            _ => return false,
        };

        // Compute the folded value based on the opcode
        let folded_val = match self.opcode {
            // Arithmetic
            // Wrapping operations to simulate standard x86 2's complement
            // arithmetic and avoid crashing the compiler on intentional/unintentional overflow.
            Opcode::Add => val1.wrapping_add(val2),
            Opcode::Sub => val1.wrapping_sub(val2),
            Opcode::Mul => val1.wrapping_mul(val2),
            Opcode::Div => {
                // Protect the compiler from a divide-by-zero panic during compilation.
                // TODO: Print warning of div by zero for user ?
                if val2 == 0 {
                    return false;
                }
                val1.wrapping_div(val2)
            }

            // Relational (mapping true to 1 and false to 0 per your IR spec)
            Opcode::Eq => i64::from(val1 == val2),
            Opcode::Neq => i64::from(val1 != val2),
            Opcode::Lt => i64::from(val1 < val2),
            Opcode::Lte => i64::from(val1 <= val2),
            Opcode::Gt => i64::from(val1 > val2),
            Opcode::Gte => i64::from(val1 >= val2),

            // Opcodes that cannot be folded via binary immediate operations
            _ => return false,
        };

        // Transform the instruction into a standard Move of the new constant
        self.opcode = Opcode::Mov;
        self.arg1 = Some(Operand::ImmInt(folded_val));
        self.arg2 = None;

        true
    }

    /// Attempts to apply algebraic identities to simplify the instruction in place.
    /// Returns `true` if the instruction was successfully simplified.
    pub fn simplify_algebraic(&mut self) -> bool {
        // check if an operand is a specific immediate integer
        let is_imm = |op: &Option<Operand>, val: i64| -> bool {
            matches!(op, Some(Operand::ImmInt(v)) if *v == val)
        };

        // check if two operands are exactly the same (e.g., %t1 and %t1)
        let are_equal = |op1: &Option<Operand>, op2: &Option<Operand>| -> bool {
            match (op1, op2) {
                (Some(a), Some(b)) => a == b,
                _ => false,
            }
        };

        let mut simplified = false;

        match self.opcode {
            Opcode::Add => {
                if is_imm(&self.arg1, 0) {
                    // 0 + x -> Mov x
                    self.opcode = Opcode::Mov;
                    self.arg1 = self.arg2.take();
                    simplified = true;
                } else if is_imm(&self.arg2, 0) {
                    // x + 0 -> Mov x
                    self.opcode = Opcode::Mov;
                    self.arg2 = None;
                    simplified = true;
                }
            }

            Opcode::Sub => {
                if is_imm(&self.arg2, 0) {
                    // x - 0 -> Mov x
                    self.opcode = Opcode::Mov;
                    self.arg2 = None;
                    simplified = true;
                } else if are_equal(&self.arg1, &self.arg2) {
                    // x - x -> Mov 0
                    self.opcode = Opcode::Mov;
                    self.arg1 = Some(Operand::ImmInt(0));
                    self.arg2 = None;
                    simplified = true;
                }
            }

            Opcode::Mul => {
                if is_imm(&self.arg1, 1) {
                    // 1 * x -> Mov x
                    self.opcode = Opcode::Mov;
                    self.arg1 = self.arg2.take();
                    simplified = true;
                } else if is_imm(&self.arg2, 1) {
                    // x * 1 -> Mov x
                    self.opcode = Opcode::Mov;
                    self.arg2 = None;
                    simplified = true;
                } else if is_imm(&self.arg1, 0) || is_imm(&self.arg2, 0) {
                    // 0 * x OR x * 0 -> Mov 0
                    self.opcode = Opcode::Mov;
                    self.arg1 = Some(Operand::ImmInt(0));
                    self.arg2 = None;
                    simplified = true;
                } else if is_imm(&self.arg2, 2) {
                    // Strength Reduction: x * 2 -> x + x
                    // Addition is cheaper on CPU cycles than multiplication ?
                    self.opcode = Opcode::Add;
                    self.arg2 = self.arg1.clone();
                    simplified = true;
                } else if is_imm(&self.arg1, 2) {
                    // Strength Reduction: 2 * x -> x + x
                    self.opcode = Opcode::Add;
                    self.arg1 = self.arg2.clone();
                    // arg2 is already x, so x + x is achieved
                    simplified = true;
                }
            }

            Opcode::Div => {
                if is_imm(&self.arg2, 1) {
                    // x / 1 -> Mov x
                    self.opcode = Opcode::Mov;
                    self.arg2 = None;
                    simplified = true;
                } else if are_equal(&self.arg1, &self.arg2) {
                    // x / x -> Mov 1
                    self.opcode = Opcode::Mov;
                    self.arg1 = Some(Operand::ImmInt(1));
                    self.arg2 = None;
                    simplified = true;
                }
            }

            Opcode::Eq | Opcode::Lte | Opcode::Gte => {
                if are_equal(&self.arg1, &self.arg2) {
                    // x == x, x <= x, x >= x -> Mov 1 (True)
                    self.opcode = Opcode::Mov;
                    self.arg1 = Some(Operand::ImmInt(1));
                    self.arg2 = None;
                    simplified = true;
                }
            }
            Opcode::Neq | Opcode::Lt | Opcode::Gt => {
                if are_equal(&self.arg1, &self.arg2) {
                    // x != x, x < x, x > x -> Mov 0 (False)
                    self.opcode = Opcode::Mov;
                    self.arg1 = Some(Operand::ImmInt(0));
                    self.arg2 = None;
                    simplified = true;
                }
            }

            _ => {}
        }

        simplified
    }
}

// ### CFG ###

/// Control Flow graph nod
#[derive(Debug, Clone)]
pub struct BasicBlock {
    pub label: String,
    pub instructions: Vec<TACInstruction>,
    pub predecessors: Vec<String>,
    pub successors: Vec<String>,
}

impl BasicBlock {
    pub fn new(label: String) -> Self {
        Self {
            label,
            instructions: Vec::new(),
            predecessors: Vec::new(),
            successors: Vec::new(),
        }
    }

    /// Propagates constants locally within the basic block.
    /// Returns true if any operands were replaced.
    pub fn propagate_constants(&mut self) -> bool {
        let mut changed = false;
        // Tracks variables with known constant integer values
        let mut known_constants: HashMap<Operand, i64> = HashMap::new();
        // Tracks variables whose address has been taken (they can be
        // modified indirectly through Store instructions).
        let mut addr_taken: Vec<Operand> = Vec::new();

        for instr in &mut self.instructions {
            // Record variables whose address is taken.
            if instr.opcode == Opcode::AddrOf {
                if let Some(ref arg1) = instr.arg1 {
                    addr_taken.push(arg1.clone());
                }
            }

            // When a Store or Call instruction is encountered, any
            // address-taken variable may have been modified (directly via
            // Store, or indirectly by a callee receiving a pointer).
            // Invalidate all of them.
            if instr.opcode == Opcode::Store || instr.opcode == Opcode::Call {
                for var in &addr_taken {
                    known_constants.remove(var);
                }
            }

            // Replace arguments if they are known constants.
            // Skip arg1 for AddrOf: arg1 is the variable whose address is
            // taken, not a value to propagate.
            if instr.opcode != Opcode::AddrOf {
                if let Some(arg1) = &instr.arg1 {
                    if let Some(&val) = known_constants.get(arg1) {
                        instr.arg1 = Some(Operand::ImmInt(val));
                        changed = true;
                    }
                }
            }
            if let Some(arg2) = &instr.arg2 {
                if let Some(&val) = known_constants.get(arg2) {
                    instr.arg2 = Some(Operand::ImmInt(val));
                    changed = true;
                }
            }

            // Update the known constants map based on the destination
            if let Some(dest) = &instr.dest {
                // If it's a direct move of an immediate, record it!
                if instr.opcode == Opcode::Mov {
                    if let Some(Operand::ImmInt(val)) = &instr.arg1 {
                        known_constants.insert(dest.clone(), *val);
                        continue;
                    }
                }

                // Otherwise, the destination is being overwritten with an UNKNOWN value.
                // Invalidate any previously known constant for this destination.
                known_constants.remove(dest);
            }
        }

        changed
    }

    pub fn emit(&mut self, instr: TACInstruction) {
        self.instructions.push(instr);
    }
}

/// Control Flow Graph representation
#[derive(Debug, Clone)]
pub struct CFG {
    pub entry: String,
    pub exit: String,
    pub blocks: BTreeMap<String, BasicBlock>,
}

impl CFG {
    pub fn new(entry: String, exit: String) -> Self {
        Self {
            entry,
            exit,
            blocks: BTreeMap::new(),
        }
    }

    /// Runs the optmizations passes on the current CFG and returns ``false`` if no further has been applied
    /// The optimizations applied to the CFG are:
    ///   - Constant folding
    ///   - Algebraic simplification
    ///   - Constant propagation
    ///   - Dead code elimination
    ///   - Unreachable code elimination
    pub fn run_optimizations(&mut self) -> bool {
        let mut changed_any = false;
        let mut loop_changed = true;

        // Keep running passes until no more changes are made
        while loop_changed {
            loop_changed = false;

            for block in self.blocks.values_mut() {
                // Fold constants (e.g. t1 = 5 + 5 -> t1 = 10)
                for instr in &mut block.instructions {
                    loop_changed |= instr.fold_constants();
                    loop_changed |= instr.simplify_algebraic();
                }

                // Propagate constants (e.g. replace t1 with 10 downstream)
                loop_changed |= block.propagate_constants();
            }

            loop_changed |= self.simplify_control_flow();

            loop_changed |= self.eliminate_dead_code();
            loop_changed |= self.merge_basic_blocks();
            changed_any |= loop_changed;
        }
        changed_any
    }

    /// Finds and merges code blocks what can me merged
    pub fn merge_basic_blocks(&mut self) -> bool {
        let mut merge_candidate = None;

        // Find a pair of blocks to merge
        for (label, block) in &self.blocks {
            if block.successors.len() == 1 {
                let succ_label = &block.successors[0];

                if let Some(succ_block) = self.blocks.get(succ_label) {
                    if succ_block.predecessors.len() == 1 && succ_block.predecessors[0] == *label {
                        if let Some(last_instr) = block.instructions.last() {
                            if last_instr.opcode == Opcode::Jump {
                                merge_candidate = Some((label.clone(), succ_label.clone()));
                                break;
                            }
                        }
                    }
                }
            }
        }

        // Apply the merge if we found a candidate
        if let Some((block_a_label, block_b_label)) = merge_candidate {
            // Extract Block B completely from the CFG
            let mut block_b = self.blocks.remove(&block_b_label).unwrap();

            // Clone the successors list to break the overlapping borrows
            let successors_to_update = block_b.successors.clone();

            // Scope the mutable borrow of Block A
            if let Some(block_a) = self.blocks.get_mut(&block_a_label) {
                block_a.instructions.pop();
                block_a.instructions.append(&mut block_b.instructions);
                block_a.successors = block_b.successors;
            }

            // Update the inherited successors safely
            for succ_label in successors_to_update {
                if let Some(succ_block) = self.blocks.get_mut(&succ_label) {
                    for p in &mut succ_block.predecessors {
                        if *p == block_b_label {
                            *p = block_a_label.clone();
                        }
                    }
                }
            }

            return true;
        }

        false
    }

    pub fn simplify_control_flow(&mut self) -> bool {
        let mut changed = false;

        // Keep track of edges we severe: (FromBlock, ToBlock)
        // This is needed so we can update the predecessors of the target blocks
        // without violating mutable aliasing rules in the first pass.
        let mut edges_to_remove: Vec<(String, String)> = Vec::new();

        for (label, block) in self.blocks.iter_mut() {
            let mut new_instructions = Vec::with_capacity(block.instructions.len());
            let mut block_truncated = false;
            let mut dead_successors = Vec::new();

            for instr in &block.instructions {
                // If a previous instruction became an unconditional jump,
                // anything after it in the same basic block is locally dead code.
                if block_truncated {
                    continue;
                }

                // Handle BrIfNot (Branch if False)
                if instr.opcode == Opcode::BranchIfNot {
                    // Extract the condition from arg1
                    if let Some(Operand::ImmInt(val)) = instr.arg1 {
                        // 2. Extract the target label from arg2
                        // (Change this to `instr.dest` if your parser puts labels there!)
                        if let Some(Operand::Label(ref target)) = instr.arg2 {
                            if val != 0 {
                                // Condition is True. "br_if_not 1" means DO NOT branch.
                                // The branch is dead. Drop the instruction.
                                dead_successors.push(target.clone());
                                changed = true;
                                continue; // Skip pushing this instruction
                            } else {
                                // Condition is False. "br_if_not 0" means ALWAYS branch.
                                // Rewrite into an unconditional Jump.
                                let mut new_jmp = instr.clone();
                                new_jmp.opcode = Opcode::Jump;
                                new_jmp.arg1 = Some(Operand::Label(target.clone())); // Jumps usually take target in arg1
                                new_jmp.arg2 = None;
                                new_instructions.push(new_jmp);

                                // Because we ALWAYS take this branch, any *other* successor
                                // (like the fallthrough) is now dead.
                                for succ in &block.successors {
                                    if succ != target {
                                        dead_successors.push(succ.clone());
                                    }
                                }

                                changed = true;
                                block_truncated = true;
                                continue;
                            }
                        }
                    }
                }

                // Handle BrIf (Branch if True)
                if instr.opcode == Opcode::BranchIf {
                    // Extract the condition from arg1
                    if let Some(Operand::ImmInt(val)) = instr.arg1 {
                        // Extract the target label from arg2
                        // (Change this to `instr.dest` if your parser puts labels there!)
                        if let Some(Operand::Label(ref target)) = instr.arg2 {
                            if val == 0 {
                                // Condition is True. "br_if 0" means DO NOT branch.
                                // The branch is dead. Drop the instruction.
                                dead_successors.push(target.clone());
                                changed = true;
                                continue; // Skip pushing this instruction
                            } else {
                                // Condition is False. "br_if 1" means ALWAYS branch.
                                // Rewrite into an unconditional Jump.
                                let mut new_jmp = instr.clone();
                                new_jmp.opcode = Opcode::Jump;
                                new_jmp.arg1 = Some(Operand::Label(target.clone())); // Jumps usually take target in arg1
                                new_jmp.arg2 = None;
                                new_instructions.push(new_jmp);

                                // Because we ALWAYS take this branch, any *other* successor
                                // (like the fallthrough) is now dead.
                                for succ in &block.successors {
                                    if succ != target {
                                        dead_successors.push(succ.clone());
                                    }
                                }

                                changed = true;
                                block_truncated = true;
                                continue;
                            }
                        }
                    }
                }

                new_instructions.push(instr.clone());
            }

            if changed {
                block.instructions = new_instructions;

                // Remove dead targets from this block's successors
                block.successors.retain(|s| !dead_successors.contains(s));

                // Queue the predecessor cleanup for Phase 2
                for dead_succ in dead_successors {
                    edges_to_remove.push((label.clone(), dead_succ));
                }
            }
        }

        // Clean up the predecessors of the orphaned blocks
        // This ensures the CFG is perfectly consistent before the DCE pass runs.
        for (from_block, to_block) in edges_to_remove {
            if let Some(target_block) = self.blocks.get_mut(&to_block) {
                target_block.predecessors.retain(|p| p != &from_block);
            }
        }

        changed
    }

    pub fn eliminate_dead_code(&mut self) -> bool {
        let mut changed = false;

        // TODO: useless ?
        // Unreachable Code Elimination (Control-Flow)
        // Perform reachability sweep starting from the CFG `entry` block
        let mut reachable = HashSet::new();
        let mut worklist = vec![self.entry.clone()];

        while let Some(block_label) = worklist.pop() {
            if reachable.insert(block_label.clone()) {
                if let Some(block) = self.blocks.get(&block_label) {
                    for succ in &block.successors {
                        if !reachable.contains(succ) {
                            worklist.push(succ.clone());
                        }
                    }
                }
            }
        }

        // Maintain a list to remove out-of-graph unreachable blocks
        let initial_len = self.blocks.len();
        self.blocks.retain(|label, _| reachable.contains(label));
        if initial_len != self.blocks.len() {
            changed = true;
        }

        // Clean up predecessors in remaining reachable blocks
        for block in self.blocks.values_mut() {
            let orig_len = block.predecessors.len();
            block.predecessors.retain(|p| reachable.contains(p));
            if orig_len != block.predecessors.len() {
                changed = true;
            }
        }

        // Global Data-Flow Analysis (Liveness)
        // A standard iterative data-flow algorithm to compute the `IN` and `OUT`
        // liveness sets for every BasicBlock.

        let mut use_sets: HashMap<String, HashSet<Operand>> = HashMap::new();
        let mut def_sets: HashMap<String, HashSet<Operand>> = HashMap::new();
        let mut in_sets: HashMap<String, HashSet<Operand>> = HashMap::new();
        let mut out_sets: HashMap<String, HashSet<Operand>> = HashMap::new();

        for (label, block) in &self.blocks {
            let mut b_use = HashSet::new();
            let mut b_def = HashSet::new();

            for instr in &block.instructions {
                // Determine `USE` within block before definitions
                let uses = vec![&instr.arg1, &instr.arg2]
                    .into_iter()
                    .filter_map(|arg| arg.as_ref().and_then(extract_var));

                for u in uses {
                    if !b_def.contains(&u) {
                        b_use.insert(u);
                    }
                }

                // Determine unconditional `DEF` within block
                if let Some(d) = instr.dest.as_ref().and_then(extract_var) {
                    b_def.insert(d);
                }
            }

            use_sets.insert(label.clone(), b_use);
            def_sets.insert(label.clone(), b_def);
            in_sets.insert(label.clone(), HashSet::new());
            out_sets.insert(label.clone(), HashSet::new());
        }

        let mut dataflow_changed = true;
        while dataflow_changed {
            dataflow_changed = false;

            for label in self.blocks.keys().cloned().collect::<Vec<_>>() {
                let mut new_out = HashSet::new();
                if let Some(block) = self.blocks.get(&label) {
                    // OUT[B] = Union over S in Successors(B) of IN[S]
                    for succ in &block.successors {
                        if let Some(succ_in) = in_sets.get(succ) {
                            new_out.extend(succ_in.iter().cloned());
                        }
                    }
                }

                if out_sets.get(&label) != Some(&new_out) {
                    out_sets.insert(label.clone(), new_out.clone());
                    dataflow_changed = true;
                }

                let mut new_in = use_sets.get(&label).unwrap().clone();
                // IN[B] = USE[B] U (OUT[B] - DEF[B])
                for d in new_out.difference(def_sets.get(&label).unwrap()) {
                    new_in.insert(d.clone());
                }

                if in_sets.get(&label) != Some(&new_in) {
                    in_sets.insert(label.clone(), new_in);
                    dataflow_changed = true;
                }
            }
        }

        // Local Dead Code Elimination
        // Backward sweeping local DCE pass within each block using the computed `OUT` sets
        for (label, block) in self.blocks.iter_mut() {
            let mut live = out_sets.get(label).unwrap().clone();
            let mut new_instructions = Vec::with_capacity(block.instructions.len());

            for instr in block.instructions.iter().rev() {
                let dest_var = instr.dest.as_ref().and_then(extract_var);

                let is_dead = dest_var.as_ref().map_or(false, |d| !live.contains(d));
                let has_side_effects = has_side_effects(&instr.opcode);

                // Instructions with side effects must never be eliminated
                if is_dead && !has_side_effects {
                    changed = true;
                    continue; // Eliminate the instruction
                }

                // If kept, update the live set.
                // Remove defined destination
                if let Some(d) = dest_var {
                    live.remove(&d);
                }

                // Add used arguments
                if let Some(u) = instr.arg1.as_ref().and_then(extract_var) {
                    live.insert(u);
                }
                if let Some(u) = instr.arg2.as_ref().and_then(extract_var) {
                    live.insert(u);
                }

                new_instructions.push(instr.clone());
            }

            // Restore the original order for the basic block
            new_instructions.reverse();
            block.instructions = new_instructions;
        }

        changed
    }

    /// Add a ``BasicBlock`` to the ``CFG``
    pub fn add_block(&mut self, block: BasicBlock) {
        self.blocks.insert(block.label.clone(), block);
    }

    /// AD an Edge to the ``CFG``
    pub fn add_edge(&mut self, from: &str, to: &str) {
        if let Some(f) = self.blocks.get_mut(from) {
            if !f.successors.iter().any(|s| s == to) {
                f.successors.push(to.to_string());
            }
        }
        if let Some(t) = self.blocks.get_mut(to) {
            if !t.predecessors.iter().any(|p| p == from) {
                t.predecessors.push(from.to_string());
            }
        }
    }
}

/// Function to get variables from an operand
fn extract_var(op: &Operand) -> Option<Operand> {
    match op {
        Operand::Var(_) | Operand::Temp(_) => Some(op.clone()),
        _ => None,
    }
}

/// Check if an instruction has side effects
fn has_side_effects(opcode: &Opcode) -> bool {
    matches!(
        opcode,
        Opcode::Jump
            | Opcode::BranchIf
            | Opcode::BranchIfNot
            | Opcode::Call
            | Opcode::Param
            | Opcode::Ret
            | Opcode::ArrayStore
            | Opcode::Store
    )
}
