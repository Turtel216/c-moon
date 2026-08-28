//! x86-64 instruction set data structures.
//!
//! Every register, operand and instruction is a Rust type, so a
//! mis-encoding -- a `setcc` into memory, say -- is a compile error rather
//! than a broken `.s` file.  Assembly text exists only in
//! [`emit`](super::emit).

use std::fmt;

/// The 16 general-purpose 64-bit registers.
///
/// Declaration order is the encoding order, and `Ord` follows it, which is
/// what makes register sets print in a stable order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum X86Register {
    Rax,
    Rbx,
    Rcx,
    Rdx,
    Rsi,
    Rdi,
    Rbp,
    Rsp,
    R8,
    R9,
    R10,
    R11,
    R12,
    R13,
    R14,
    R15,
}

/// How wide a view of a register an instruction wants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegisterWidth {
    /// The full 64-bit register, e.g. `rax`.
    Quad,
    /// The low 32 bits, e.g. `eax`; writing it zero-extends into the full
    /// register, which is how `movzx` reaches 64 bits without a REX prefix.
    Double,
    /// The low 8 bits, e.g. `al`: the width of a `char`, and what `setcc`
    /// writes.
    Byte,
}

impl X86Register {
    /// The assembler name of this register at `width`.
    ///
    /// One table for all three widths keeps the names of a register together,
    /// so adding a width or a register is a single edit.
    ///
    /// The byte names are the ones a REX prefix gives -- `sil`, `dil`, `bpl`,
    /// `spl` and `r8b`-`r15b` -- and never `ah`, `bh`, `ch` or `dh`: the
    /// legacy high-byte registers cannot be encoded in an instruction that
    /// carries a REX prefix, which every instruction naming `sil` or an
    /// extended register does. Naming the low byte of RSI `ah` would also be
    /// a different register entirely.
    pub fn name(self, width: RegisterWidth) -> &'static str {
        let [quad, double, byte] = match self {
            Self::Rax => ["rax", "eax", "al"],
            Self::Rbx => ["rbx", "ebx", "bl"],
            Self::Rcx => ["rcx", "ecx", "cl"],
            Self::Rdx => ["rdx", "edx", "dl"],
            Self::Rsi => ["rsi", "esi", "sil"],
            Self::Rdi => ["rdi", "edi", "dil"],
            Self::Rbp => ["rbp", "ebp", "bpl"],
            Self::Rsp => ["rsp", "esp", "spl"],
            Self::R8 => ["r8", "r8d", "r8b"],
            Self::R9 => ["r9", "r9d", "r9b"],
            Self::R10 => ["r10", "r10d", "r10b"],
            Self::R11 => ["r11", "r11d", "r11b"],
            Self::R12 => ["r12", "r12d", "r12b"],
            Self::R13 => ["r13", "r13d", "r13b"],
            Self::R14 => ["r14", "r14d", "r14b"],
            Self::R15 => ["r15", "r15d", "r15b"],
        };

        match width {
            RegisterWidth::Quad => quad,
            RegisterWidth::Double => double,
            RegisterWidth::Byte => byte,
        }
    }
}

/// A condition code, used by `jcc` and `setcc`.
///
/// Ordering comes in two families, because the flags an ordering has to read
/// depend on how the operands read: a signed comparison looks at the sign and
/// overflow flags, an unsigned one at the carry flag. Equality is one code for
/// both -- two bit patterns are equal or they are not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConditionCode {
    /// Equal (ZF=1).
    E,
    /// Not equal (ZF=0).
    Ne,
    /// Signed less than (SF != OF).
    L,
    /// Signed less than or equal (ZF=1 or SF != OF).
    Le,
    /// Signed greater than (ZF=0 and SF=OF).
    G,
    /// Signed greater than or equal (SF=OF).
    Ge,
    /// Unsigned less than, "below" (CF=1).
    B,
    /// Unsigned less than or equal, "below or equal" (CF=1 or ZF=1).
    Be,
    /// Unsigned greater than, "above" (CF=0 and ZF=0).
    A,
    /// Unsigned greater than or equal, "above or equal" (CF=0).
    Ae,
}

impl ConditionCode {
    /// The mnemonic suffix, e.g. `ne` in `jne` and `setne`.
    pub fn suffix(self) -> &'static str {
        match self {
            Self::E => "e",
            Self::Ne => "ne",
            Self::L => "l",
            Self::Le => "le",
            Self::G => "g",
            Self::Ge => "ge",
            Self::B => "b",
            Self::Be => "be",
            Self::A => "a",
            Self::Ae => "ae",
        }
    }
}

/// Which way a shift moves its operand, and what fills the bits it vacates.
///
/// Like an ordering, a right shift comes in two forms because the answer
/// depends on how the operand reads: an arithmetic shift keeps a signed
/// value's sign by copying it down, where a logical one shifts zeroes in. A
/// left shift always vacates the bottom, which zeroes fill either way, so
/// there is one of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShiftOp {
    /// `shl` -- towards the top, zeroes in at the bottom.
    Left,
    /// `sar` -- towards the bottom, copies of the sign bit in at the top.
    ArithmeticRight,
    /// `shr` -- towards the bottom, zeroes in at the top.
    LogicalRight,
}

impl ShiftOp {
    /// The assembler mnemonic for this shift.
    pub const fn mnemonic(self) -> &'static str {
        match self {
            Self::Left => "shl",
            Self::ArithmeticRight => "sar",
            Self::LogicalRight => "shr",
        }
    }
}

/// A memory reference, `[base + index * scale + disp]`.
///
/// The scaled index is what lets an array access be a single instruction; it
/// is absent for the plain base-plus-displacement form that reaches a frame
/// slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryRef {
    pub base: X86Register,
    /// Index register and its scale, one of 1, 2, 4 or 8.
    pub index: Option<(X86Register, u8)>,
    pub disp: i32,
}

/// An operand of an x86-64 instruction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum X86Operand {
    /// A physical register, e.g. `rax`.
    Reg(X86Register),
    /// A memory reference, e.g. `[rbp - 8]`.
    Mem(MemoryRef),
    /// An immediate integer.
    Imm(i64),
    /// A symbol reference, for `call` and `lea`.
    Label(String),
}

impl X86Operand {
    /// `[base + disp]`.
    pub const fn mem(base: X86Register, disp: i32) -> Self {
        Self::Mem(MemoryRef {
            base,
            index: None,
            disp,
        })
    }

    /// `[base + index * scale + disp]`.
    pub const fn indexed(base: X86Register, index: X86Register, scale: u8, disp: i32) -> Self {
        Self::Mem(MemoryRef {
            base,
            index: Some((index, scale)),
            disp,
        })
    }
}

impl From<X86Register> for X86Operand {
    fn from(register: X86Register) -> Self {
        Self::Reg(register)
    }
}

/// A single x86-64 instruction.
///
/// Operand types are as narrow as the hardware: `setcc` and `movzx` take
/// registers because that is all they can encode here, and jumps take a label
/// because nothing else is emitted.
///
/// An instruction that could apply to either operand size carries the
/// [`RegisterWidth`] it applies to, which is what makes `int` arithmetic wrap
/// where C says it does and keeps a four-byte object from being written eight
/// bytes at a time.  The ones that are always full-word -- taking an address,
/// pushing an argument slot -- carry none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum X86Instruction {
    /// `mov dst, src`
    Mov(RegisterWidth, X86Operand, X86Operand),
    /// `lea dst, src` -- the address of `src`, not its contents.
    Lea(X86Operand, X86Operand),
    /// `add dst, src`
    Add(RegisterWidth, X86Operand, X86Operand),
    /// `sub dst, src`
    Sub(RegisterWidth, X86Operand, X86Operand),
    /// `imul dst, src` -- signed multiply.
    Imul(RegisterWidth, X86Operand, X86Operand),
    /// `cdq` / `cqo` -- sign-extend the accumulator into the high half of the
    /// dividend, which `idiv` requires.
    SignExtendAccumulator(RegisterWidth),
    /// `idiv src` -- signed divide the accumulator pair, quotient in RAX.
    Idiv(RegisterWidth, X86Operand),
    /// `div src` -- unsigned divide the accumulator pair, quotient in RAX.
    ///
    /// A separate instruction from `idiv` rather than a flag on it, because
    /// the two are separate instructions: they read the same bits and give
    /// different answers.
    Div(RegisterWidth, X86Operand),
    /// `and dst, src`
    And(RegisterWidth, X86Operand, X86Operand),
    /// `or dst, src`
    Or(RegisterWidth, X86Operand, X86Operand),
    /// `xor dst, src`
    Xor(RegisterWidth, X86Operand, X86Operand),
    /// `shl` / `sar` / `shr dst, count` -- shift `dst` by `count`, which the
    /// hardware takes either as an immediate or from CL and nowhere else.
    ///
    /// The count is read as a byte however wide the operand shifted is, which
    /// is why it is the one operand not written at the instruction's width.
    Shift(ShiftOp, RegisterWidth, X86Operand, X86Operand),
    /// `cmp lhs, rhs` -- flags from `lhs - rhs`.
    Cmp(RegisterWidth, X86Operand, X86Operand),
    /// `test lhs, rhs` -- flags from `lhs & rhs`.
    Test(RegisterWidth, X86Operand, X86Operand),
    /// `setcc dst` -- set the low byte of `dst` from the condition.
    SetCC(ConditionCode, X86Register),
    /// `movzx dst, src` -- zero-extend `src`, read at `from`, into `dst` at
    /// the wider `to`: how an `unsigned char` becomes an `int`, and how the
    /// byte `setcc` writes becomes the 0 or 1 of one.
    ///
    /// There is no 32-to-64 case, and no instruction for one: writing a
    /// 32-bit register already clears the whole of the 64-bit one, so a plain
    /// `mov` zero-extends for free.
    Movzx {
        to: RegisterWidth,
        from: RegisterWidth,
        destination: X86Register,
        source: X86Operand,
    },
    /// `movsx dst, src` -- sign-extend `src`, read at `from`, into `dst` at
    /// the wider `to`: how a `char` becomes an `int` and an `int` a `long
    /// int`.  Both widths are carried because the mnemonic the assembler
    /// picks (`movsbl`, `movsbq`, `movslq`) follows from the pair.
    Movsx {
        to: RegisterWidth,
        from: RegisterWidth,
        destination: X86Register,
        source: X86Operand,
    },
    /// `push src`
    Push(X86Operand),
    /// `pop dst`
    Pop(X86Operand),
    /// `jmp label`
    Jmp(String),
    /// `jcc label`
    Jcc(ConditionCode, String),
    /// `call label`
    Call(String),
    /// `ret`
    Ret,
    /// A label definition, e.g. `.L1:`.
    Label(String),
}

/// One compiled function.
#[derive(Debug, Clone)]
pub struct X86Function {
    pub name: String,
    pub instructions: Vec<X86Instruction>,
}

/// A complete compiled program.
#[derive(Debug, Clone)]
pub struct X86Program {
    pub functions: Vec<X86Function>,
}

impl fmt::Display for X86Register {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name(RegisterWidth::Quad))
    }
}

impl fmt::Display for ConditionCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.suffix())
    }
}
