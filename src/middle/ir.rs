//! Three-address code: the representation the front end lowers to and the
//! back end consumes.
//!
//! Nothing optimises here.  The middle-end rebuilds each function in SSA form
//! ([`ssa`](crate::middle::ssa)), optimises that, and translates back, so this
//! is a boundary format at both ends rather than something passes work on.

use std::collections::BTreeMap;
use std::fmt;

pub use crate::frontend::ast::Sign;

// ### TAC IR ###

/// How many bits of a value an operation is defined on.
///
/// C's integer types are not all the machine word: a `char` is 8 bits wide,
/// an `int` 32 and a `long int` 64, so `a + b` wraps at a different place
/// depending on which one it adds. The IR therefore records the width every
/// operation computes at, and the backend selects the instruction that
/// matches.
///
/// Only the low `bits` of a value are ever meaningful. Anything above them is
/// whatever the last instruction to write the register happened to leave
/// there, so every reader of a value has to read it at its own width -- which
/// is exactly what carrying the width on the instruction guarantees.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Width {
    /// 8 bits: the width of `char`.
    Bits8,
    /// 32 bits: the width of `int`.
    Bits32,
    /// 64 bits: the width of `long int`, of every pointer, and of an address.
    Bits64,
}

impl Width {
    /// How many bytes a value of this width occupies.
    pub const fn bytes(self) -> i32 {
        match self {
            Width::Bits8 => 1,
            Width::Bits32 => 4,
            Width::Bits64 => 8,
        }
    }

    /// The value `constant` takes on when it is held at this width.
    ///
    /// Constants travel through the IR as `i64`, so a narrower one is kept
    /// sign-extended: the representation of an `int` is always the `i64` with
    /// the same value, which is what lets a fold at one width be compared
    /// against a constant of the other without further conversion.
    ///
    /// Sign-extended, and not zero-extended, because every integer type this
    /// compiler has is signed -- plain `char` included, as the System V
    /// x86-64 ABI defines it.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(Width::Bits32.narrow(i64::from(i32::MAX) + 1), i64::from(i32::MIN));
    /// assert_eq!(Width::Bits8.narrow(321), 65);
    /// assert_eq!(Width::Bits64.narrow(i64::MAX), i64::MAX);
    /// ```
    pub const fn narrow(self, constant: i64) -> i64 {
        match self {
            Width::Bits8 => constant as i8 as i64,
            Width::Bits32 => constant as i32 as i64,
            Width::Bits64 => constant,
        }
    }

    /// The low bits of `constant` read as an unsigned number.
    ///
    /// Constants are stored the way [`Width::narrow`] leaves them -- the low
    /// bits, sign-extended -- because that is one representation for one bit
    /// pattern. An unsigned operation reads the same pattern differently, and
    /// this is that reading: the `-1` an `unsigned int` is stored as is the
    /// 4294967295 an unsigned comparison or division has to see.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(Width::Bits8.unsigned(-1), 255);
    /// assert_eq!(Width::Bits32.unsigned(-1), 4294967295);
    /// ```
    pub const fn unsigned(self, constant: i64) -> u64 {
        match self {
            Width::Bits8 => constant as u8 as u64,
            Width::Bits32 => constant as u32 as u64,
            Width::Bits64 => constant as u64,
        }
    }

    /// The value of the low bits of `constant` when read with `sign`.
    ///
    /// This is what a widening conversion of a constant computes: the source's
    /// signedness decides whether the bits above it are copies of its top one
    /// or zeroes, exactly as `movsx` and `movzx` decide it at run time.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(Width::Bits8.read(Sign::Signed, 255), -1);
    /// assert_eq!(Width::Bits8.read(Sign::Unsigned, 255), 255);
    /// ```
    pub const fn read(self, sign: Sign, constant: i64) -> i64 {
        match sign {
            Sign::Signed => self.narrow(constant),
            Sign::Unsigned => self.unsigned(constant) as i64,
        }
    }
}

impl fmt::Display for Width {
    /// Writes the width in bits, e.g. the `32` of `add.32`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Width::Bits8 => write!(f, "8"),
            Width::Bits32 => write!(f, "32"),
            Width::Bits64 => write!(f, "64"),
        }
    }
}

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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opcode {
    // Arithmetic
    /// TAC Addition e.g. %r1 + %r2
    Add,
    /// TAC subste.g. %r1 - %r2
    Sub,
    /// TAC Multiplication e.g. %r1 * %r2
    Mul,
    /// TAC Divition e.g. %r1 / %r2
    ///
    /// The one arithmetic operation whose answer depends on how its operands
    /// read: `-2 / 1` is -2 signed and a very large number unsigned, where an
    /// addition of the same bits gives the same bits either way.
    Div(Sign),

    // Relational / equality (result is 0/1)
    /// TAC Equalality e.g. %r1 == %r2
    Eq,
    /// TAC e.g. %r1 != %r2
    Neq,
    /// TAC Less then operator e.g. %r1 < %r2
    Lt(Sign),
    /// TAC Lees then or equal operator e.g. %r1 <= %r2
    Lte(Sign),
    /// TAC Greater then operator e.g. %r1 > %r2
    Gt(Sign),
    /// TAC Greater then or equal operator e.g. %r1 >= %r2
    Gte(Sign),

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

    /// Take the address of a place inside an object: dest = &arg1[arg2].
    ///
    /// The width is what scales the index into a byte offset -- for an array
    /// element, the element type, exactly as it does for an access to one. A
    /// width of [`Width::Bits8`] therefore makes the index a plain byte
    /// offset, which is how a struct member is reached: `&s.y` is the address
    /// of byte `offsetof(y)` of `s`.
    ///
    /// The value produced is an address, and so a full word; see
    /// [`Opcode::result_width`].
    ArrayAddr,

    /// Load a value through a pointer: dest = *arg1
    Load,
    /// Store a value through a pointer: *arg1 = arg2
    Store,
    /// Take address of a variable: dest = addr_of arg1(Var)
    AddrOf,

    /// Convert a value to another integer width: dest = (width) arg1.
    ///
    /// The instruction's width is the width of the *result*, and `from` is
    /// the width the operand is read at -- both are needed, there being three
    /// integer widths and no way to infer one from the other. `sign` is the
    /// operand's, which decides what a widening puts above it: copies of its
    /// top bit for a signed operand, zeroes for an unsigned one. Narrowing
    /// keeps the low bits and asks nothing.
    Convert { from: Width, sign: Sign },
}

impl Opcode {
    /// Whether this operation compares its operands.
    ///
    /// A comparison is the one kind of operation whose result is not as wide
    /// as what it reads: it answers a question, and in C the answer is the 0
    /// or 1 of an `int`.
    pub const fn is_relational(&self) -> bool {
        matches!(
            self,
            Opcode::Eq
                | Opcode::Neq
                | Opcode::Lt(_)
                | Opcode::Lte(_)
                | Opcode::Gt(_)
                | Opcode::Gte(_)
        )
    }

    /// The width of the value this operation defines, given the width it
    /// computes at.
    pub const fn result_width(&self, width: Width) -> Width {
        if self.is_relational() {
            return Width::Bits32;
        }
        // An address is a full word however narrow the object at it is: the
        // width an element address carries scales its index and says nothing
        // about the result.
        match matches!(self, Opcode::ArrayAddr) {
            true => Width::Bits64,
            false => width,
        }
    }
}

/// TAC Instruction representation
#[derive(Debug, Clone, PartialEq)]
pub struct TACInstruction {
    /// Instruction operation
    pub opcode: Opcode,
    /// The width this operation computes at.
    ///
    /// For arithmetic it is the width of the operands and of the result; for a
    /// comparison, the width the operands are compared at, the result being
    /// the 0 or 1 of an `int`; for a memory access, the size of the object
    /// touched.
    ///
    /// An opcode that only moves a value from one home to another is correct
    /// at any width -- a copy of the meaningful low bits is still a copy of
    /// them whatever else it drags along -- and takes [`Width::Bits64`] where
    /// the value's own width is not at hand; see
    /// [`TACInstruction::transfer`].
    pub width: Width,
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
        width: Width,
        dest: Option<Operand>,
        arg1: Option<Operand>,
        arg2: Option<Operand>,
    ) -> Self {
        Self {
            opcode,
            width,
            dest,
            arg1,
            arg2,
        }
    }

    /// An instruction that only moves a value between homes, a whole register
    /// at a time.
    ///
    /// This is for the transfers whose operand is a value the middle-end
    /// created rather than a variable the program declared: a jump, and the
    /// moves the middle-end emits when it leaves SSA form. Reading such a
    /// value at full width is always in bounds, because the compiler gave it a
    /// whole word to live in. Anything naming a declared variable carries that
    /// variable's own width instead -- see [`TACInstruction::width`].
    pub fn transfer(
        opcode: Opcode,
        dest: Option<Operand>,
        arg1: Option<Operand>,
        arg2: Option<Operand>,
    ) -> Self {
        Self::new(opcode, Width::Bits64, dest, arg1, arg2)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn narrowing_wraps_a_constant_the_way_the_hardware_would() {
        // Arrange / Act / Assert: one past the largest `int` is the smallest.
        assert_eq!(
            Width::Bits32.narrow(i64::from(i32::MAX) + 1),
            i64::from(i32::MIN)
        );
        // A value that differs from 2 only above bit 32 arrives as 2.
        assert_eq!(Width::Bits32.narrow(4294967298), 2);
        // `(char)321` keeps the low eight bits, which say 65.
        assert_eq!(Width::Bits8.narrow(321), 65);
        // A plain `char` is signed, so the top bit of the byte is the sign.
        assert_eq!(Width::Bits8.narrow(255), -1);
    }

    #[test]
    fn a_constant_that_fits_is_left_alone_at_either_width() {
        // Arrange / Act / Assert
        assert_eq!(Width::Bits32.narrow(-7), -7);
        assert_eq!(Width::Bits64.narrow(i64::MIN), i64::MIN);
    }

    #[test]
    fn an_element_address_is_a_full_word_however_narrow_the_element_is() {
        // Arrange / Act / Assert: the width of `&a[i]` scales the index, so
        // the address it produces is a machine word either way.
        assert_eq!(Opcode::ArrayAddr.result_width(Width::Bits32), Width::Bits64);
        assert_eq!(Opcode::ArrayAddr.result_width(Width::Bits64), Width::Bits64);
    }

    #[test]
    fn a_comparison_answers_with_an_int_however_wide_its_operands_are() {
        // Arrange / Act / Assert: `a < b` on two `long int`s is still an
        // `int`, while the arithmetic keeps the width it computed at.
        assert_eq!(
            Opcode::Lt(Sign::Signed).result_width(Width::Bits64),
            Width::Bits32
        );
        assert_eq!(Opcode::Add.result_width(Width::Bits64), Width::Bits64);
        assert_eq!(Opcode::Add.result_width(Width::Bits32), Width::Bits32);
    }

    #[test]
    fn one_bit_pattern_reads_two_ways() {
        // Arrange / Act / Assert: the byte an `unsigned char` holds 255 in is
        // the byte a plain `char` holds -1 in.
        assert_eq!(Width::Bits8.narrow(255), -1);
        assert_eq!(Width::Bits8.unsigned(255), 255);
        assert_eq!(Width::Bits8.read(Sign::Signed, 255), -1);
        assert_eq!(Width::Bits8.read(Sign::Unsigned, 255), 255);
        // A 64-bit value has nowhere wider to be read into, so both readings
        // are the same bits.
        assert_eq!(Width::Bits64.read(Sign::Unsigned, -1), -1);
        assert_eq!(Width::Bits32.unsigned(-1), 4294967295);
    }

    #[test]
    fn a_width_is_as_many_bytes_as_the_type_it_stands_for() {
        // Arrange / Act / Assert
        assert_eq!(Width::Bits8.bytes(), 1);
        assert_eq!(Width::Bits32.bytes(), 4);
        assert_eq!(Width::Bits64.bytes(), 8);
    }
}
