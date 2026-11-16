//! Repository automation, run as `cargo xtask <command>`. Replaces the PowerShell scripts (ADR 0020).

mod doctor;
mod gate;
mod run;

use std::process::ExitCode;

const USAGE: &str = "usage: cargo xtask <doctor | gate [--fast]>";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match args.as_slice() {
        ["doctor"] => doctor::run(),
        ["gate"] => gate::run(false),
        ["gate", "--fast"] => gate::run(true),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("xtask: {error}");
            ExitCode::FAILURE
        }
    }
}
