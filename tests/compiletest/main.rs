//! End-to-end test harness for the C-Moon compiler.
//!
//! The suite follows the methodology of `rustc`'s own `compiletest`: every
//! test is a real C file on disk that declares its expectations in `//@`
//! header directives, and the harness discovers those files rather than
//! requiring a hand-written `#[test]` per case. Adding a test means adding a
//! `.c` file -- no Rust changes.
//!
//! # Suites
//!
//! | Directory         | Asserts                                                |
//! |-------------------|--------------------------------------------------------|
//! | `tests/run-pass`  | The program compiles, links, runs and exits as declared.|
//! | `tests/codegen`   | The emitted assembly satisfies its `CHECK` directives.  |
//! | `tests/ui`        | The compiler rejects it with the recorded diagnostics.  |
//!
//! `run-pass` fixtures are run twice -- with and without `--opt` -- from a
//! single file, so the optimiser is covered by construction instead of by
//! duplicating each case by hand.
//!
//! # Usage
//!
//! ```text
//! cargo test --test compiletest                  # everything
//! cargo test --test compiletest -- pointers      # filter by name
//! cargo test --test compiletest -- --list        # list the fixtures
//! BLESS=1 cargo test --test compiletest          # rewrite ui snapshots
//! ```

mod directives;
mod filecheck;
mod runner;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use libtest_mimic::{Arguments, Trial};

use crate::directives::VariantPolicy;
use crate::runner::{Suite, TestCase, Variant};

/// Extension identifying a fixture.
const FIXTURE_EXTENSION: &str = "c";

fn main() -> ExitCode {
    let arguments = Arguments::from_args();

    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let trials = match collect(&root) {
        Ok(trials) => trials,
        Err(message) => {
            eprintln!("compiletest: {}", message);
            return ExitCode::FAILURE;
        }
    };

    libtest_mimic::run(&arguments, trials).exit_code()
}

/// Discover every fixture under `root` and turn it into one or more trials.
///
/// # Arguments
///
/// * `root` - The `tests/` directory holding the suite subdirectories.
///
/// # Returns
///
/// The trials in a stable order: by suite, then by fixture path, then by
/// variant.
///
/// # Errors
///
/// Returns a message when a suite directory cannot be read. A fixture whose
/// header is malformed does not abort discovery; it becomes a failing trial
/// so the error is reported against that file and the rest still run.
fn collect(root: &Path) -> Result<Vec<Trial>, String> {
    let mut trials = Vec::new();

    for suite in Suite::ALL {
        let suite_dir = root.join(suite.dir());
        if !suite_dir.is_dir() {
            continue;
        }

        for path in fixtures(&suite_dir)? {
            // `strip_prefix` cannot fail here: `path` came from walking
            // `suite_dir`.
            let relative = path
                .strip_prefix(&suite_dir)
                .map_err(|e| format!("{}: {}", path.display(), e))?
                .with_extension("")
                .to_string_lossy()
                .into_owned();

            trials.extend(trials_for(suite, path, relative));
        }
    }

    Ok(trials)
}

/// Build the trials for a single fixture.
///
/// Returns exactly one failing trial when the fixture's header is unusable,
/// and one trial per applicable variant otherwise.
fn trials_for(suite: Suite, path: PathBuf, relative: String) -> Vec<Trial> {
    let kind = suite.dir();

    let source = match fs::read_to_string(&path) {
        Ok(source) => source,
        Err(e) => {
            return vec![broken(
                kind,
                &relative,
                format!("cannot read fixture: {}", e),
            )];
        }
    };

    let directives = match directives::parse(&source) {
        Ok(directives) => directives,
        Err(e) => return vec![broken(kind, &relative, format!("bad directive: {}", e))],
    };

    if let Some(reason) = misused_directive(suite, &directives) {
        return vec![broken(kind, &relative, reason)];
    }

    if let Some(reason) = &directives.ignore {
        // Reported as ignored rather than silently dropped, so a fixture
        // parked behind a known limitation stays visible in the report.
        return vec![
            Trial::test(format!("{} ({})", relative, reason), || Ok(()))
                .with_kind(kind)
                .with_ignored_flag(true),
        ];
    }

    variants(suite, directives.variants)
        .into_iter()
        .map(|variant| {
            let case = TestCase {
                suite,
                path: path.clone(),
                relative: relative.clone(),
                variant,
                directives: directives.clone(),
            };

            Trial::test(case.name(), move || case.run()).with_kind(kind)
        })
        .collect()
}

/// Reject directives that have no meaning in the suite that declared them.
///
/// Catching this at discovery time keeps a misplaced expectation from looking
/// like a passing test.
fn misused_directive(suite: Suite, directives: &directives::Directives) -> Option<String> {
    if suite != Suite::RunPass {
        if directives.exit_code.is_some() {
            return Some(format!(
                "'exit-code' only applies to the run-pass suite, not {}",
                suite.dir()
            ));
        }

        if directives.variants != VariantPolicy::Both {
            return Some(format!(
                "'only' selects an optimisation variant, which the {} suite \
                 does not generate; use 'compile-flags' instead",
                suite.dir()
            ));
        }
    }

    None
}

/// The variants a fixture runs under.
///
/// Only `run-pass` fixtures are duplicated across optimisation levels; the
/// other suites assert on output that a fixture pins down exactly, so they run
/// once with whatever flags they declare.
fn variants(suite: Suite, policy: VariantPolicy) -> Vec<Variant> {
    match suite {
        Suite::RunPass => {
            let mut variants = match policy {
                VariantPolicy::Both => vec![Variant::NoOpt, Variant::Opt],
                VariantPolicy::NoOptOnly => vec![Variant::NoOpt],
                VariantPolicy::OptOnly => vec![Variant::Opt],
            };
            // The reference build is independent of which of this compiler's
            // levels a fixture asks for: it checks the expectation itself.
            variants.push(Variant::Gcc);
            variants
        }
        Suite::Codegen | Suite::Ui => vec![Variant::AsDeclared],
    }
}

/// A trial that reports a problem with the fixture itself.
fn broken(kind: &str, relative: &str, message: String) -> Trial {
    Trial::test(relative.to_owned(), move || Err(message.into())).with_kind(kind)
}

/// Collect every `.c` file under `dir`, recursively, in a stable order.
///
/// # Errors
///
/// Returns a message when a directory cannot be listed.
fn fixtures(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut entries: Vec<PathBuf> = fs::read_dir(dir)
        .map_err(|e| format!("cannot read {}: {}", dir.display(), e))?
        .map(|entry| {
            entry
                .map(|entry| entry.path())
                .map_err(|e| format!("cannot read an entry of {}: {}", dir.display(), e))
        })
        .collect::<Result<_, _>>()?;

    // `read_dir` yields entries in filesystem order, which varies between
    // machines; sorting keeps the report reproducible.
    entries.sort();

    let mut found = Vec::new();
    for entry in entries {
        if entry.is_dir() {
            found.extend(fixtures(&entry)?);
        } else if entry
            .extension()
            .is_some_and(|ext| ext == FIXTURE_EXTENSION)
        {
            found.push(entry);
        }
    }

    Ok(found)
}
