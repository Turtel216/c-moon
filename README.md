# C-Moon

A lightweight, optimizing C-to-x86 compiler built from scratch in Rust.

This project is an educational compiler designed to compile a strict subset of the C programming language into standard x86 assembly. It features a hand-coded recursive descent parser, an SSA-based optimizer, and a target-independent backend.

## Architecture

The compiler is a classic pipeline, with each stage handing off a representation suited to the work it does:

1. **Frontend:** A hand-rolled lexer (scanner plus a small preprocessor) and recursive descent parser build an Abstract Syntax Tree (AST), followed by semantic analysis for type and scope checking and a name resolution (renamer) pass. Every node carries a source span, which the diagnostics renderer turns into an annotated snippet.
2. **Middle-End:** The AST is lowered into a linear, architecture-independent Three-Address Code (TAC) form. TAC is a boundary format only: the compiler immediately rebuilds each function into Static Single Assignment (SSA) form, where memory locations that qualify (plain scalar variables, not arrays or anything with its address taken) are promoted to SSA values. All optimization runs on this SSA representation, after which it is destructed back into TAC for the backend.
3. **Backend:** Translates TAC into machine code. It is split into stages that are the same for every machine -- CFG linearization, liveness analysis, a linear scan register allocator and stack frame layout -- and one module per target that supplies the rest. A target is described by the `Target` trait: its register file, frame parameters, instruction selection and assembly syntax. x86-64 with the System V AMD64 ABI is the only one implemented, and everything before instruction selection is reused as it stands by any target added next.

## Optimizations

All optimizations run on the SSA form, to a fixed point, in the following order:

* **Sparse Conditional Constant Propagation (SCCP):** Propagates constants, folds the arithmetic they make constant, and prunes branches whose condition is known, all in one pass -- which is what lets each of the three make the others stronger.
* **Algebraic Simplification:** Identities that hold regardless of the unknown operand's value, such as `x * 1 -> x` and `x - x -> 0`.
* **Global Value Numbering (GVN):** Recognizes two instructions that compute the same operation over the same SSA values and reuses the first result instead of recomputing it, scoped by dominance.
* **Copy Propagation:** Replaces every read of a copy with the value it copies, including resolving trivial phi nodes.
* **Dead-Code Elimination (DCE):** A mark-and-sweep pass over the def-use graph that removes any instruction whose result is never observed.
* **Block Merging:** Joins a block into its unique predecessor when that predecessor is its only way in, removing the jump between them.

## Supported Language Subset

*Currently targeting an MVP subset of C to establish the full pipeline:*
* **Data Types:** `char` (8-bit), `int` (32-bit) and `long int` (64-bit), each in a signed and an `unsigned` form, arrays of any of them, pointers, and `struct`.
* **Control Flow:** `if` / `else`, `while` and `for` loops, `return`.
* **Operators:** Arithmetic (`+`, `-`, `*`, `/`), Relational (`==`, `!=`, `<`, `>`)
  and Logical (`&&`, `||`, `!`), the first two of which short-circuit.
* **Functions:** Declarations, definitions, and calls with arguments.
* **Preprocessor macros:** object-like macros such as `#define X 5` and simple non-recursive function-like macros.
* **Pointers:** referencing and dereferencing. Does not support pointer arithmetic yet.
* **Structs:** definitions and forward declarations, member access with `.` and
  `->`, nested structs, arrays of structs and array members, pointers to
  structs -- including a struct that points at its own type -- and whole-object
  assignment, which copies the representation the way C defines it. Members are
  laid out by the System V rules, so a member sits at the offset its own
  alignment demands and the object is rounded up to its widest member's; the
  test suite pins those offsets against GCC. A struct crosses a call boundary
  through a pointer: passing or returning one by value needs the ABI's
  argument-classification rules, which are not implemented.
* **Integer conversions:** the integer promotions -- a `char` operand becomes an `int` before any operator sees it -- the usual arithmetic conversions, including the rule that an `int` and an `unsigned int` meet as `unsigned int`, the implicit conversion an assignment, argument or `return` performs, and casts between the integer types. Widening sign-extends a signed value and zero-extends an unsigned one; division and ordering select the instruction that matches.

### Roadmap

* [ ] Additional data types (`float` and `double` are parsed but rejected by semantic analysis today).
* [ ] Passing and returning a `struct` by value, which needs the System V
  argument classification, and the array-to-pointer decay that would let an
  array be a parameter.
* [ ] Integer literal suffixes (`u`, `L`); a decimal literal too large for a `long int` is already an `unsigned long int`, as GCC makes it.
* [ ] Pointer arithmetic.
* [ ] Global variables.
* [ ] `extern` keyword and linking against GCC-compiled C programs and the standard library.
* [ ] Remaining operators already recognized by the parser but not yet lowered (`%`, bitwise and shift operators).

## Diagnostics

Errors are reported the way `rustc` reports them: a headline with a code, the
location, the offending line, and an underline saying what is wrong. Related
spans are quoted too, so the error and its cause are visible at once.

```text
error[E0203]: the name `a` is defined multiple times
 --> tests/ui/redeclared-variable.c:5:9
  |
4 |     int a = 1;
  |         - previous declaration of `a` here
5 |     int a = 2;
  |         ^ `a` redeclared here
  |
  = note: `a` must be declared only once in the same scope

error: aborting due to 1 previous error
```

Every diagnostic carries a stable `E0xxx` code, an unknown name is offered the
closest match in scope, and both the parser and the analyzer keep going after
an error so one run reports every problem it can. Colour is used on a
terminal and dropped when the output is redirected or when `NO_COLOR` is set.

## Getting Started

Build the project:

```bash
cargo build --release
```

The Compiler CLI:

```bash
Usage: c-moon [OPTIONS] <SOURCE_FILE>

Arguments:
  <SOURCE_FILE>  The C source file

Options:
  -o <OUTPUT_FILE>  The output file [default: output]
      --opt         Enable optimizations
      --printast    Pretty print AST
      --printir     Pretty print IR
      --asm         Output Assembly
  -h, --help        Print help
```

## Test Suite

The end-to-end suite follows the methodology of `rustc`'s own `compiletest`:
each test is a C file under `tests/` that declares its expectations in `//@`
header directives, split into three suites -- `run-pass` (compile, link, run,
check the exit status), `codegen` (`CHECK` directives matched against the
emitted assembly) and `ui` (diagnostics compared against checked-in `.stderr`
snapshots). Every `run-pass` fixture runs against both the unoptimized and the
optimized pipeline, and also against GCC as a ground truth for the expected
exit code.

```bash
cargo test                                  # everything
cargo test --test compiletest -- pointers   # filter by name
cargo test --test compiletest -- --list     # list every fixture
BLESS=1 cargo test --test compiletest       # re-record ui snapshots
```

See [`tests/README.md`](tests/README.md) for the full directive reference and
for how to add a test.
