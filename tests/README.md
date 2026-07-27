# C-Moon Test Suite

The suite follows the methodology of `rustc`'s own [`compiletest`]: a test is a
**C file on disk** that declares its own expectations in header directives. The
harness discovers those files, so adding a test means adding a `.c` file -- no
Rust code changes, no registration list to keep in sync.

[`compiletest`]: https://rustc-dev-guide.rust-lang.org/tests/intro.html

## Layout

```
tests/
  compiletest/       the harness itself
    main.rs          fixture discovery, one test per fixture and variant
    directives.rs    parsing of `//@` headers
    filecheck.rs     the `CHECK` matcher used by the codegen suite
    runner.rs        compiling, running and asserting a single fixture
  run-pass/          programs that must compile, link, run and exit correctly
  codegen/           programs whose emitted assembly is pinned down
  ui/                programs the compiler must reject, with `.stderr` snapshots
```

## Running

```bash
cargo test                             # everything
cargo test --test compiletest          # just the compiler suite
cargo test --test compiletest -- pointers   # only fixtures matching "pointers"
cargo test --test compiletest -- --list     # list every fixture
BLESS=1 cargo test --test compiletest       # rewrite the ui snapshots
```

Each fixture is an independently named, independently filterable test, and the
suite runs in parallel: every trial gets its own scratch directory under
`target/tmp/compiletest/`.

## The suites

### `run-pass`

Compile the fixture, link it with GCC, run it, and assert the exit status.

```c
// Multiplication binds tighter than addition.
//@ exit-code: 12

int main() {
    int a = 1;
    int b = 2;
    int c = 2;
    return c * b + 10 - a * 2;
}
```

**Every `run-pass` fixture runs twice: once without `--opt` and once with it.**
A single file therefore covers both the naive and the optimised pipeline, and
the two appear separately in the report as `[no-opt]` and `[opt]`. This is the
main reason the suite exists in this shape -- the previous one duplicated each
case by hand, which meant new tests silently skipped the optimiser whenever
someone forgot the second copy.

### `codegen`

Compile to assembly and match `CHECK` directives against the listing, the way
`rustc`'s `tests/codegen` suite works. Asserting on the generated code is how
an optimisation test proves the optimisation actually happened, rather than
just that the result is still correct.

```c
//@ compile-flags: --opt

// CHECK-LABEL: main:
// CHECK-NOT: add
// CHECK: mov rax, 42

int main() {
    int a = 20;
    int b = 22;
    return a + b;
}
```

| Directive     | Meaning                                                      |
|---------------|--------------------------------------------------------------|
| `CHECK`       | A later line contains the pattern.                            |
| `CHECK-NEXT`  | The immediately following line contains the pattern.          |
| `CHECK-NOT`   | No line between the previous and the next match has it.       |
| `CHECK-LABEL` | Like `CHECK`; used to anchor a region such as a function.     |

Matching is by substring on whitespace-normalised lines, so a pattern never
depends on the assembler's indentation. A fixture with no directives is an
error rather than a vacuous pass.

### `ui`

Compile a program the compiler must reject and compare the diagnostics against
a checked-in `.stderr` snapshot sitting next to the fixture. Machine-specific
paths are folded into `$DIR` and `$TMP`, so the snapshots are portable.

Snapshots are data, not hand-written text. After changing a diagnostic:

```bash
BLESS=1 cargo test --test compiletest
git diff tests/ui        # review the wording change as a diff
```

This makes the quality of the compiler's error messages a reviewable part of
every pull request.

## Directives

Directives are `//@` lines anywhere in a fixture. An unknown directive is a
hard error, so a typo cannot quietly disable a test.

| Directive             | Suites     | Meaning                                            |
|-----------------------|------------|----------------------------------------------------|
| `//@ exit-code: N`    | `run-pass` | Status the program must exit with (`0..=255`, default 0). |
| `//@ compile-flags: …`| all        | Extra flags for the compiler invocation.            |
| `//@ only: opt`       | `run-pass` | Run only the optimised variant.                     |
| `//@ only: no-opt`    | `run-pass` | Run only the unoptimised variant.                   |
| `//@ ignore: reason`  | all        | Report as ignored, with the reason shown.           |

`only:` and `exit-code:` are rejected in the suites where they have no meaning,
so a misplaced expectation cannot masquerade as a passing test.

## Adding a test

1. Drop a `.c` file into the right suite directory. Subdirectories are walked
   recursively and become part of the test's name (`pointers/deref [opt]`).
2. Give it a one-line comment saying what it exercises, and the directives it
   needs.
3. For a `ui` fixture, record its snapshot with `BLESS=1 cargo test` and commit
   the `.stderr` alongside it.
