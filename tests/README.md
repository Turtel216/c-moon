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
    extern/
      auxiliary/     companion translation units, named by `aux-build`
  codegen/           programs whose emitted assembly is pinned down
  ui/                programs the compiler must reject, with `.stderr` snapshots
```

An `auxiliary/` directory may sit beside the fixtures of any suite that links
(`run-pass` and `codegen`). Discovery skips it: what is inside is a
translation unit of its own, not a fixture.

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

**Every `run-pass` fixture runs three times: without `--opt`, with it, and
once through `gcc -O0`.** A single file therefore covers both the naive and the
optimised pipeline, and the three appear separately in the report as
`[no-opt]`, `[opt]` and `[gcc]`. This is the main reason the suite exists in
this shape -- the previous one duplicated each case by hand, which meant new
tests silently skipped the optimiser whenever someone forgot the second copy.

The `[gcc]` variant does not involve this compiler at all. It builds the same
fixture with a production compiler and checks the declared exit code against
it, which is what makes `//@ exit-code` an assertion about what the C program
*means* rather than about what this compiler currently does with it. Without
it, a fixture recorded from a miscompilation would pass for ever.

#### Linking against another translation unit

A fixture that declares an `extern` function names a definition it does not
contain. `//@ aux-build: helper.c` supplies it: the harness compiles
`auxiliary/helper.c` **with GCC** and links the object into the build, this
compiler's and the `[gcc]` reference one's alike.

```c
// A function defined in another translation unit.
//@ aux-build: arithmetic.c
//@ exit-code: 21

extern int triple(int x);

int main() {
    return triple(7);
}
```

The companion is deliberately built by a production compiler rather than by
this one. That is what makes the fixture an assertion about the System V ABI
-- argument registers, stack arguments, alignment at a `call` -- instead of an
assertion that this compiler agrees with itself. A companion may also call
back into the fixture, which exercises the boundary in the other direction.

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
paths are folded into `$DIR` and `$TMP`, so the snapshots are portable, and the
compiler is run with `NO_COLOR=1` so no escape sequence reaches the snapshot.

```text
error[E0201]: cannot find value `countr` in this scope
 --> $DIR/similar-name.c:5:12
  |
5 |     return countr;
  |            ^^^^^^ not found in this scope
  |
  = help: a variable with a similar name exists: `counter`

error: aborting due to 1 previous error
```

A fixture covers one diagnostic: the snapshot is the whole point of the test,
so a file that trips several errors at once makes a change to any of them look
like a change to all of them. `reports-every-error.c` is the deliberate
exception -- reporting more than one error *is* what it tests.

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
| `//@ aux-build: f.c`  | all but `ui` | Build `auxiliary/f.c` with GCC and link it in.    |
| `//@ only: opt`       | `run-pass` | Run only the optimised variant.                     |
| `//@ only: no-opt`    | `run-pass` | Run only the unoptimised variant.                   |
| `//@ ignore: reason`  | all        | Report as ignored, with the reason shown.           |

`only:`, `exit-code:` and `aux-build:` are rejected in the suites where they
have no meaning, so a misplaced expectation cannot masquerade as a passing
test.

## Adding a test

1. Drop a `.c` file into the right suite directory. Subdirectories are walked
   recursively and become part of the test's name (`pointers/deref [opt]`).
2. Give it a one-line comment saying what it exercises, and the directives it
   needs.
3. For a `ui` fixture, record its snapshot with `BLESS=1 cargo test` and commit
   the `.stderr` alongside it.
4. If it needs a definition from another translation unit, drop that file in an
   `auxiliary/` directory beside it and name it with `//@ aux-build:`.
