mod backend;
mod driver;
mod frontend;
mod middle;
mod printer;

use std::process::ExitCode;

/// Returning [`ExitCode`] from `main` lets the driver's status reach the
/// shell: a rejected program exits non-zero, a compiled one exits zero.
fn main() -> ExitCode {
    driver::run()
}
