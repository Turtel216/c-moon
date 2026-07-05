# C-Moon 

A lightweight, optimizing C-to-x86 compiler built from scratch in Rust.

This project is an educational compiler designed to compile a strict subset of the C programming language into standard x86 assembly. It features a hand-coded recursive descent parser, a custom Three-Address Code (TAC) intermediate representation, and implemented optimization passes.

## Architecture

The compiler is structured as a classic three-phase pipeline to separate language semantics from machine architecture:

1. **Frontend:** A hand-rolled Lexer and Recursive Descent Parser that construct an Abstract Syntax Tree (AST), followed by semantic analysis for type and scope checking and a .
2. **Middle-End:** Lowers the AST into a linear, architecture-independent Three-Address Code (TAC) IR. This phase is responsible for target-independent optimizations.
3. **Backend:** Translates the optimized IR into x86 assembly, utilizing a linear scan register allocator and managing x86 calling conventions.

## Development Roadmap

**Phase 1: The Frontend (Done)**
- [x] **Lexical Analysis (Scanner):** Tokenization of C source code.
- [x] **Syntax Analysis (Parser):** Hand-rolled recursive descent parser building an AST.
- [x] **AST Visualization:** Debug tooling to print the AST structure to the console.
- [x] **Semantic Analysis:** Symbol table generation, variable scoping, and basic type checking.
- [x] **Renamer:** Name resolution (Scope Renamer) pass.

**Phase 2: The Middle-End (Done)**
- [x] **IR Generation:** Lowering the AST into Three-Address Code (TAC).
- [x] **Control Flow Graph (CFG):** Building basic blocks for optimization analysis.
- [x] **Optimization - Constant Folding:** Evaluating static expressions at compile time.
- [x] **Optimization - Algebraic Simplification:** Replacing complex arithmetic with simpler, equivalent operations or identities.
- [x] **Optimization - Constant Propagation:** Replacing variables with known constant values downstream.
- [x] **Optimization - Dead Code Elimination:** Pruning instructions that compute unused values.
- [x] **Optimization - Unreachable Code Elimination:** Removing basic blocks that have no incoming execution paths.

**Phase 3: The Backend (Done)**
- [x] **Instruction Selection:** Mapping TAC operations to x86 instructions.
- [x] **Register Allocation:** Implementing a Linear Scan Register Allocator.
- [x] **Code Emission:** Generating valid `.s` files assembled via GCC.

**Phase 4: Support more C language features (In Progress)**
- [x] Preprocessor macros (Partially)
- [x] Static sized arrays
- [x] Pointers
- [ ] `char` data type
- [ ] `strings`
- [ ] `extern` keyword and linking to GCC compiled C programs and standard library

## Supported Language Subset

*Currently targeting an MVP subset of C to establish the full pipeline:*
* **Data Types:** `int`, `int[size]`;
* **Control Flow:** `if` / `else`, `while` loops, `return`
* **Operators:** Arithmetic (`+`, `-`, `*`, `/`), Relational (`==`, `!=`, `<`, `>`)
* **Functions:** Declarations, definitions, and calls with arguments.
* **Preprocessor macros** object like macros such as ``#define X 5`` and simple non-recursive function like macros.
* **Pointers**: Support for referencing and dereferencing. Does not support pointer arithmetic yet.

## Getting Started

Build the project:

```bash
cargo build --release
```

Run unit tests:
```bash
cargo test
```

The Compiler CLI:

``` bash
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

