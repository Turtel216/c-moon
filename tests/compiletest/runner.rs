//! Execution of a single fixture.
//!
//! Each [`TestCase`] is one entry in the test report: a fixture file paired
//! with the optimisation variant it runs under. The three suites differ only
//! in what they assert about the compiler's output.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use libtest_mimic::Failed;

use crate::directives::Directives;
use crate::filecheck;

/// Path to the compiler binary under test.
///
/// Cargo builds the binary before the harness runs and exposes its path here,
/// so the tests can never pick up a stale build.
const COMPILER: &str = env!("CARGO_BIN_EXE_c-moon");

/// Directory Cargo reserves for integration-test scratch files (`target/tmp`).
const SCRATCH_ROOT: &str = env!("CARGO_TARGET_TMPDIR");

/// Setting this environment variable rewrites `ui` snapshots from the run.
const BLESS_VAR: &str = "BLESS";

/// Directory, beside a fixture, holding the companions it names in
/// `aux-build`. Shared with fixture discovery, which skips it.
pub const AUXILIARY_DIR: &str = "auxiliary";

/// Placeholder substituted for the fixture's directory in snapshots.
const DIR_PLACEHOLDER: &str = "$DIR";

/// Placeholder substituted for the per-test scratch directory in snapshots.
const TMP_PLACEHOLDER: &str = "$TMP";

/// The three fixture suites, each with its own assertion style.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Suite {
    /// Compile, link and run; assert the program's exit status.
    RunPass,
    /// Compile to assembly; assert `CHECK` directives against the listing.
    Codegen,
    /// Compile expecting rejection; assert the diagnostics against a snapshot.
    Ui,
}

impl Suite {
    /// Every suite, in the order they are reported.
    pub const ALL: [Suite; 3] = [Suite::RunPass, Suite::Codegen, Suite::Ui];

    /// Directory under `tests/` holding the suite's fixtures.
    ///
    /// Doubles as the suite label shown in the test report.
    pub fn dir(self) -> &'static str {
        match self {
            Suite::RunPass => "run-pass",
            Suite::Codegen => "codegen",
            Suite::Ui => "ui",
        }
    }
}

/// Which optimisation setting a single trial runs under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    /// Flags come from the fixture's `compile-flags` alone.
    AsDeclared,
    /// Compiler invoked without `--opt`.
    NoOpt,
    /// Compiler invoked with `--opt`.
    Opt,
    /// Not this compiler at all: the fixture is built with `gcc -O0`, and its
    /// declared exit code checked against what a production compiler makes of
    /// the same program.
    Gcc,
}

impl Variant {
    /// The flag this variant adds to the compiler invocation, if any.
    fn flag(self) -> Option<&'static str> {
        match self {
            Variant::AsDeclared | Variant::NoOpt | Variant::Gcc => None,
            Variant::Opt => Some("--opt"),
        }
    }

    /// Suffix appended to the test's reported name.
    fn suffix(self) -> &'static str {
        match self {
            Variant::AsDeclared => "",
            Variant::NoOpt => " [no-opt]",
            Variant::Opt => " [opt]",
            Variant::Gcc => " [gcc]",
        }
    }

    /// Component used to keep variants' scratch directories apart.
    fn dir(self) -> &'static str {
        match self {
            Variant::AsDeclared => "as-declared",
            Variant::NoOpt => "no-opt",
            Variant::Opt => "opt",
            Variant::Gcc => "gcc",
        }
    }
}

/// A fixture paired with the variant it runs under: one entry in the report.
#[derive(Debug, Clone)]
pub struct TestCase {
    /// Suite the fixture belongs to.
    pub suite: Suite,
    /// Absolute path of the `.c` fixture.
    pub path: PathBuf,
    /// Fixture path relative to its suite directory, without the extension.
    pub relative: String,
    /// Optimisation variant for this trial.
    pub variant: Variant,
    /// Expectations declared in the fixture's header.
    pub directives: Directives,
}

impl TestCase {
    /// The name shown in the test report, e.g. `pointers/deref [opt]`.
    pub fn name(&self) -> String {
        format!("{}{}", self.relative, self.variant.suffix())
    }

    /// Compile the fixture and check whatever its suite asserts.
    ///
    /// # Errors
    ///
    /// Returns a rendered report describing the mismatch: an unexpected
    /// compiler status, a wrong exit code, an unsatisfied `CHECK` directive,
    /// or a diagnostic snapshot that drifted.
    pub fn run(self) -> Result<(), Failed> {
        let work_dir = self.work_dir();
        prepare_dir(&work_dir)?;

        // GCC writes the executable here and the compiler writes `<stem>.s`
        // alongside it.
        let stem = work_dir.join("a");

        // A fixture that declares `extern` names is only half a program. Its
        // companions are built first, by GCC, and linked into whichever build
        // follows -- this compiler's or the reference one's.
        let auxiliaries = self.build_auxiliaries(&work_dir)?;

        // The reference variant never invokes this compiler: it asks what the
        // program is supposed to do, not what this compiler does with it.
        if self.variant == Variant::Gcc {
            return self.check_against_gcc(&stem, &auxiliaries);
        }

        let compiled = self.compile(&stem, &auxiliaries)?;

        match self.suite {
            Suite::RunPass => self.check_run_pass(&compiled, &stem),
            Suite::Codegen => self.check_codegen(&compiled, &stem),
            Suite::Ui => self.check_ui(&compiled, &work_dir),
        }
    }

    /// Scratch directory for this trial, unique per suite, fixture and variant
    /// so the harness can run tests in parallel.
    fn work_dir(&self) -> PathBuf {
        Path::new(SCRATCH_ROOT)
            .join("compiletest")
            .join(self.suite.dir())
            .join(self.relative.replace('/', "-"))
            .join(self.variant.dir())
    }

    /// Invoke the compiler on the fixture, capturing its output.
    ///
    /// # Arguments
    ///
    /// * `stem` - the executable's path; the assembly listing sits beside it
    /// * `auxiliaries` - objects built from the fixture's companions, handed
    ///   to the compiler to link in
    fn compile(&self, stem: &Path, auxiliaries: &[PathBuf]) -> Result<Output, Failed> {
        let mut command = Command::new(COMPILER);
        command.arg(&self.path).arg("-o").arg(stem);

        for object in auxiliaries {
            command.arg("--link").arg(object);
        }

        // The codegen suite needs the assembly listing to survive the run.
        if self.suite == Suite::Codegen {
            command.arg("--asm");
        }

        command.args(self.variant.flag());
        command.args(&self.directives.compile_flags);

        // Diagnostics must be free of ANSI escapes to be snapshot-comparable.
        command.env("NO_COLOR", "1");

        command
            .output()
            .map_err(|e| Failed::from(format!("could not run the compiler: {}", e)))
    }

    /// `run-pass`: the program must build and exit with the declared status.
    fn check_run_pass(&self, compiled: &Output, stem: &Path) -> Result<(), Failed> {
        self.expect_compiled(compiled)?;
        self.expect_exit_code(stem)
    }

    /// Build the fixture with a production compiler and check that it agrees.
    ///
    /// This is the differential half of the suite. The other variants prove
    /// that every optimisation level of this compiler agrees with the declared
    /// exit code; this one proves the declared exit code is what the C program
    /// actually means, so an expectation cannot be wrong in the same direction
    /// as the compiler.
    fn check_against_gcc(&self, stem: &Path, auxiliaries: &[PathBuf]) -> Result<(), Failed> {
        // Warnings are silenced: this asks what the program does, and the
        // fixtures are written for a compiler that accepts a subset of C.
        let compiled = Command::new("gcc")
            .args(["-O0", "-w", "-o"])
            .arg(stem)
            .arg(&self.path)
            .args(auxiliaries)
            .output()
            .map_err(|e| Failed::from(format!("could not run gcc: {}", e)))?;

        if !compiled.status.success() {
            return Err(format!(
                "gcc rejected the fixture:\n{}",
                String::from_utf8_lossy(&compiled.stderr)
            )
            .into());
        }

        self.expect_exit_code(stem)
    }

    /// Build every companion the fixture declares, one object each.
    ///
    /// They are built with GCC rather than with the compiler under test, on
    /// purpose: an `extern` declaration is a claim about a translation unit
    /// this compiler never sees, so linking against one a production compiler
    /// produced is what proves the two agree about the ABI instead of only
    /// about this compiler's own conventions.
    ///
    /// # Arguments
    ///
    /// * `work_dir` - this trial's scratch directory, where the objects go
    ///
    /// # Errors
    ///
    /// Returns a report naming the companion GCC could not build.
    fn build_auxiliaries(&self, work_dir: &Path) -> Result<Vec<PathBuf>, Failed> {
        // Resolved once rather than per companion: every one of a fixture's
        // companions sits in the same directory.
        //
        // A subdirectory rather than the suite directory itself, because
        // discovery walks that one: a companion sitting next to the fixture
        // would be picked up as a fixture of its own.
        let sources = self
            .path
            .parent()
            .expect("a fixture always sits inside its suite directory")
            .join(AUXILIARY_DIR);

        self.directives
            .aux_builds
            .iter()
            .map(|name| Self::build_auxiliary(&sources.join(name), work_dir, name))
            .collect()
    }

    /// Compile one companion to an object file, returning where it landed.
    fn build_auxiliary(source: &Path, work_dir: &Path, name: &str) -> Result<PathBuf, Failed> {
        let object = work_dir.join(name).with_extension("o");

        let built = Command::new("gcc")
            .args(["-c", "-O0", "-w", "-o"])
            .arg(&object)
            .arg(source)
            .output()
            .map_err(|e| Failed::from(format!("could not run gcc: {}", e)))?;

        if !built.status.success() {
            return Err(format!(
                "gcc could not build the companion {}:\n{}",
                source.display(),
                indent(&String::from_utf8_lossy(&built.stderr))
            )
            .into());
        }

        Ok(object)
    }

    /// Run the built program and compare its status with the declared one.
    fn expect_exit_code(&self, stem: &Path) -> Result<(), Failed> {
        let run = Command::new(stem)
            .output()
            .map_err(|e| Failed::from(format!("could not run the compiled program: {}", e)))?;

        let expected = self.directives.exit_code.unwrap_or(0);
        match run.status.code() {
            Some(code) if code == expected => Ok(()),
            Some(code) => {
                Err(format!("exit code mismatch: expected {}, got {}", expected, code).into())
            }
            // No code means a signal killed the process -- typically a
            // miscompilation faulting at run time.
            None => Err(format!("the program was killed: {}", run.status).into()),
        }
    }

    /// `codegen`: the emitted assembly must satisfy the `CHECK` directives.
    fn check_codegen(&self, compiled: &Output, stem: &Path) -> Result<(), Failed> {
        self.expect_compiled(compiled)?;

        let listing_path = stem.with_extension("s");
        let listing = fs::read_to_string(&listing_path).map_err(|e| {
            Failed::from(format!(
                "could not read the assembly at {}: {}",
                listing_path.display(),
                e
            ))
        })?;

        let source = read_fixture(&self.path)?;
        let checks = filecheck::parse(&source).map_err(Failed::from)?;

        filecheck::run(&checks, &listing).map_err(Failed::from)
    }

    /// `ui`: the compiler must reject the fixture with the recorded
    /// diagnostics.
    fn check_ui(&self, compiled: &Output, work_dir: &Path) -> Result<(), Failed> {
        if compiled.status.success() {
            return Err(String::from("expected compilation to fail, but it succeeded").into());
        }

        let actual = normalize(&compiled.stderr, &self.path, work_dir);
        let snapshot = self.path.with_extension("stderr");

        if blessing() {
            return fs::write(&snapshot, &actual).map_err(|e| {
                Failed::from(format!(
                    "could not write the snapshot {}: {}",
                    snapshot.display(),
                    e
                ))
            });
        }

        let expected = fs::read_to_string(&snapshot).map_err(|_| {
            Failed::from(format!(
                "no snapshot at {}; re-run with {}=1 to record it.\nactual diagnostics:\n{}",
                snapshot.display(),
                BLESS_VAR,
                actual
            ))
        })?;

        if expected == actual {
            return Ok(());
        }

        Err(format!(
            "diagnostics do not match {}; re-run with {}=1 to update it.\n{}",
            snapshot.display(),
            BLESS_VAR,
            diff(&expected, &actual)
        )
        .into())
    }

    /// Require a clean, successful compilation.
    ///
    /// A successful compile is expected to be silent, so anything on stderr
    /// (an internal error, a stray warning) fails the test rather than
    /// scrolling past unnoticed.
    fn expect_compiled(&self, compiled: &Output) -> Result<(), Failed> {
        let stderr = String::from_utf8_lossy(&compiled.stderr);

        if !compiled.status.success() {
            return Err(format!(
                "compilation failed ({}):\n{}",
                compiled.status,
                indent(&stderr)
            )
            .into());
        }

        if !stderr.trim().is_empty() {
            return Err(format!(
                "compilation succeeded but wrote to stderr:\n{}",
                indent(&stderr)
            )
            .into());
        }

        Ok(())
    }
}

/// Read a fixture, reporting the path when it cannot be read.
pub fn read_fixture(path: &Path) -> Result<String, Failed> {
    fs::read_to_string(path)
        .map_err(|e| Failed::from(format!("could not read {}: {}", path.display(), e)))
}

/// Whether snapshots should be rewritten instead of compared.
fn blessing() -> bool {
    std::env::var_os(BLESS_VAR).is_some_and(|value| !value.is_empty() && value != "0")
}

/// Create an empty scratch directory, discarding anything left by a prior run.
fn prepare_dir(dir: &Path) -> Result<(), Failed> {
    // A missing directory is the normal case on a clean checkout, so only a
    // real removal failure is an error. The `&&` inside `if let` is a
    // let-chain: the guard is only evaluated when the pattern matched.
    if let Err(e) = fs::remove_dir_all(dir)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        return Err(format!("could not clear {}: {}", dir.display(), e).into());
    }

    fs::create_dir_all(dir)
        .map_err(|e| Failed::from(format!("could not create {}: {}", dir.display(), e)))
}

/// Replace machine-specific paths so snapshots are portable.
///
/// The fixture's directory and the scratch directory differ between checkouts
/// and between runs, so they are folded into stable placeholders.
fn normalize(raw: &[u8], source: &Path, work_dir: &Path) -> String {
    // `from_utf8_lossy` keeps a malformed byte from aborting the comparison;
    // the resulting replacement character shows up in the diff instead.
    let mut text = String::from_utf8_lossy(raw).into_owned();

    text = text.replace(&work_dir.to_string_lossy().into_owned(), TMP_PLACEHOLDER);
    if let Some(parent) = source.parent() {
        text = text.replace(&parent.to_string_lossy().into_owned(), DIR_PLACEHOLDER);
    }

    text
}

/// Render a line-oriented diff of two snapshots.
fn diff(expected: &str, actual: &str) -> String {
    let expected: Vec<&str> = expected.lines().collect();
    let actual: Vec<&str> = actual.lines().collect();

    let mut rendered = String::from("  --- expected\n  +++ actual\n");
    for index in 0..expected.len().max(actual.len()) {
        match (expected.get(index), actual.get(index)) {
            (Some(e), Some(a)) if e == a => {
                let _ = writeln!(rendered, "    {}", e);
            }
            (expected_line, actual_line) => {
                if let Some(line) = expected_line {
                    let _ = writeln!(rendered, "  - {}", line);
                }
                if let Some(line) = actual_line {
                    let _ = writeln!(rendered, "  + {}", line);
                }
            }
        }
    }

    rendered
}

/// Indent captured output so it reads as a block inside a failure report.
fn indent(text: &str) -> String {
    text.lines()
        .map(|line| format!("  {}\n", line))
        .collect::<String>()
}
