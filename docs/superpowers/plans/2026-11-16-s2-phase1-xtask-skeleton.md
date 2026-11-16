# S2 Phase 1: xtask Skeleton, Doctor and Gate Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `cargo xtask` crate with `doctor` and `gate [--fast]`. `gate` must behave exactly like `scripts/quality-gate.ps1 -SkipPreflight`, and both commands must run on macOS, Linux and Windows CI.

**Architecture:** `xtask/` is a workspace member, reached through a cargo alias. It has no dependencies. `main.rs` only dispatches. `run.rs` spawns processes. `gate.rs` holds the step list and a stop-at-first-failure runner, which is unit-tested with an injected runner. `doctor.rs` replaces `scripts/preflight.ps1`, and its parsing helpers are unit-tested. The PowerShell scripts stay in place until Phase 4.

**Tech Stack:** Rust 1.94.0 (std only), cargo alias, GitHub Actions.

**Spec:** `docs/superpowers/specs/2026-11-16-cargo-xtask-migration-design.md`. Read `docs/superpowers/specs/2026-11-16-cleanup-program-roadmap.md` first.

## Global Constraints

- Toolchain: rustc 1.94.0, from `rust-toolchain.toml`.
- `xtask` uses `[lints] workspace = true`. That means `unsafe_code = "forbid"`, and `unwrap_used`/`expect_used` are denied outside tests. `clippy.toml` allows them in tests.
- No dependencies in Phase 1. (`regex` 1.12.3 arrives in Phase 2, for guardrails.)
- Arguments are parsed by hand with std. No logic in `main.rs` beyond dispatch.
- No PowerShell, `pwsh` or `powershell` invocation anywhere in `xtask/`.
- `git` must be version 2.32 or later.
- Do not add `TODO`, `FIXME`, `dbg!`, `#[allow(...)]` or duct-tape comments (AGENTS.md).
- Every task ends with `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace --locked` green.
- Shell setup on the owner's Mac: `. "$HOME/.cargo/env" && export PATH="$HOME/.local/bin:$PATH"`.

## Scope Notes

- **No guardrails step yet.** `gate` does not run guardrails in Phase 1, because `quality-gate.ps1` never did (`safe:*` ran them separately). Phase 2 adds the guardrails step once they are ported. No PowerShell shim is added in between.
- **Modules deferred.** `paths.rs` and `report.rs` from the spec are not created until a command needs them (Phase 3). Phase 1 has no use for them.
- **CI cost.** The repository is public, so macOS runner minutes are free.

## File Structure

| File | Responsibility |
|---|---|
| `xtask/Cargo.toml` | Crate manifest: no dependencies, workspace lints |
| `xtask/src/main.rs` | Argument dispatch and exit code |
| `xtask/src/run.rs` | `cargo()` command builder, `status()` and `stdout()` process helpers |
| `xtask/src/gate.rs` | Gate steps, plus a runner that stops at the first failure |
| `xtask/src/doctor.rs` | Environment checks that replace `preflight.ps1` |
| `Cargo.toml` (modify) | Add `xtask` to `members` |
| `.cargo/config.toml` (modify) | Add the `xtask` alias |
| `Cargo.lock` (modify) | New `xtask` package entry |
| `.github/workflows/rust-ci.yml` (modify) | Add a `macos-latest` leg, plus `xtask doctor` and `xtask gate` steps |
| `docs/adr/0020-portable-automation-cargo-xtask.md` (create) | Decision record and parity results |
| `docs/adr/README.md` (modify) | List ADR 0020 (required by the guardrail) |

---

### Task 1: xtask crate with `gate`

**Files:**
- Create: `xtask/Cargo.toml`, `xtask/src/main.rs`, `xtask/src/run.rs`, `xtask/src/gate.rs`
- Modify: `Cargo.toml` (workspace `members`), `.cargo/config.toml` (`[alias]`), `Cargo.lock`

**Interfaces:**
- Produces:
  - `run::cargo() -> std::process::Command`
  - `run::status(&mut Command) -> Result<(), String>`
  - `run::stdout(&mut Command) -> Result<String, String>`
  - `gate::run(fast: bool) -> Result<(), String>`
  - `gate::steps(fast: bool) -> Vec<&'static [&'static str]>`

- [ ] **Step 1: Register the crate and alias**

`Cargo.toml`, change `members` (leave `default-members` unchanged):

```toml
members = [
  "apps/desktop/src-tauri",
  "xtask",
]
```

`.cargo/config.toml`, add this line to `[alias]`:

```toml
xtask = "run --quiet --package xtask --"
```

`xtask/Cargo.toml`:

```toml
[package]
name = "xtask"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
publish = false

[lints]
workspace = true
```

- [ ] **Step 2: Write the failing gate tests**

`xtask/src/gate.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::{run_steps, steps};

    #[test]
    fn full_gate_runs_quality_gate_ps1_steps_in_order() {
        assert_eq!(
            steps(false),
            vec![
                &["fmt", "--all", "--", "--check"][..],
                &["clippy", "--workspace", "--all-targets", "--", "-D", "warnings"][..],
                &["check", "--workspace", "--locked"][..],
                &["test", "--workspace", "--locked"][..],
            ]
        );
    }

    #[test]
    fn fast_gate_skips_tests() {
        assert!(steps(true).iter().all(|step| step[0] != "test"));
        assert_eq!(steps(true).len(), 3);
    }

    #[test]
    fn gate_stops_at_the_first_failing_step() {
        let mut ran = Vec::new();
        let result = run_steps(&steps(false), |step| {
            ran.push(step[0].to_string());
            if step[0] == "clippy" {
                Err("clippy failed".to_string())
            } else {
                Ok(())
            }
        });
        assert_eq!(result, Err("clippy failed".to_string()));
        assert_eq!(ran, ["fmt", "clippy"]);
    }
}
```

`xtask/src/main.rs`, minimal so the crate compiles:

```rust
//! Repository automation, run as `cargo xtask <command>`. Replaces the PowerShell scripts (ADR 0020).

mod gate;
mod run;

fn main() {}
```

- [ ] **Step 3: Run the tests and watch them fail**

Run: `cargo test -p xtask`
Expected: a compile error, because `steps` and `run_steps` are not found.

- [ ] **Step 4: Implement `run.rs` and `gate.rs`**

`xtask/src/run.rs`:

```rust
use std::process::Command;

/// The cargo that launched xtask, so nested runs use the same toolchain.
pub fn cargo() -> Command {
    Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
}

/// Runs with inherited stdio; fails on spawn error or non-zero exit.
pub fn status(command: &mut Command) -> Result<(), String> {
    let shown = format!("{command:?}");
    let status = command
        .status()
        .map_err(|error| format!("{shown}: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{shown} exited with {status}"))
    }
}

/// Runs and returns trimmed stdout; fails on spawn error or non-zero exit.
pub fn stdout(command: &mut Command) -> Result<String, String> {
    let shown = format!("{command:?}");
    let output = command
        .output()
        .map_err(|error| format!("{shown}: {error}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(format!(
            "{shown} exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}
```

Put this above the tests module in `xtask/src/gate.rs`:

```rust
use crate::run;

/// The steps `scripts/quality-gate.ps1` ran, in the same order; `--fast` skips tests.
pub fn steps(fast: bool) -> Vec<&'static [&'static str]> {
    let mut steps: Vec<&'static [&'static str]> = vec![
        &["fmt", "--all", "--", "--check"],
        &["clippy", "--workspace", "--all-targets", "--", "-D", "warnings"],
        &["check", "--workspace", "--locked"],
    ];
    if !fast {
        steps.push(&["test", "--workspace", "--locked"]);
    }
    steps
}

pub fn run(fast: bool) -> Result<(), String> {
    run_steps(&steps(fast), |args| run::status(run::cargo().args(args)))?;
    println!("\nQuality gate passed.");
    Ok(())
}

fn run_steps(
    steps: &[&[&str]],
    mut runner: impl FnMut(&[&str]) -> Result<(), String>,
) -> Result<(), String> {
    for step in steps {
        println!("\n> cargo {}", step.join(" "));
        runner(step)?;
    }
    Ok(())
}
```

Replace `xtask/src/main.rs` with:

```rust
//! Repository automation, run as `cargo xtask <command>`. Replaces the PowerShell scripts (ADR 0020).

mod gate;
mod run;

use std::process::ExitCode;

const USAGE: &str = "usage: cargo xtask <gate [--fast]>";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match args.as_slice() {
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
```

- [ ] **Step 5: Update the lockfile and run the tests**

Run: `cargo check -p xtask` (without `--locked`, so `Cargo.lock` gains the `xtask` entry), then `cargo test -p xtask`.
Expected: 3 passed.

- [ ] **Step 6: Run the gate end to end**

Run: `cargo xtask gate --fast`, then `cargo xtask gate`
Expected: each step prints `> cargo ...`, and the run ends with `Quality gate passed.` and exit code 0. Also run `cargo xtask bogus`. Expected: the usage line and exit code 2.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml Cargo.lock .cargo/config.toml xtask
git commit -m "feat(xtask): add cargo xtask gate matching quality-gate.ps1"
```

---

### Task 2: `cargo xtask doctor`

**Files:**
- Create: `xtask/src/doctor.rs`
- Modify: `xtask/src/main.rs` (dispatch `doctor`)

**Interfaces:**
- Consumes: `run::stdout(&mut Command) -> Result<String, String>`
- Produces: `doctor::run() -> Result<(), String>`

- [ ] **Step 1: Write the failing tests**

`xtask/src/doctor.rs`:

```rust
#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{inside_any, parse_git_version};

    #[test]
    fn parses_git_versions_from_each_platform() {
        assert_eq!(parse_git_version("git version 2.39.5 (Apple Git-154)"), Some((2, 39)));
        assert_eq!(parse_git_version("git version 2.47.1.windows.1"), Some((2, 47)));
        assert_eq!(parse_git_version("git version 2.31.0"), Some((2, 31)));
        assert_eq!(parse_git_version("not git"), None);
    }

    #[test]
    fn detects_a_target_dir_inside_onedrive() {
        let roots = vec![PathBuf::from("/Users/a/OneDrive")];
        assert!(inside_any(Path::new("/Users/a/OneDrive/code/target"), &roots));
        assert!(!inside_any(Path::new("/Users/a/code/target"), &roots));
        assert!(!inside_any(Path::new("/Users/a/code/target"), &[]));
    }
}
```

Add `mod doctor;` to `xtask/src/main.rs`.

- [ ] **Step 2: Run the tests and watch them fail**

Run: `cargo test -p xtask doctor`
Expected: a compile error, because `parse_git_version` and `inside_any` are not found.

- [ ] **Step 3: Implement the doctor**

Put this above the tests module in `xtask/src/doctor.rs`:

```rust
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::run;

const RUST_VERSION: &str = "1.94.0";
const MIN_GIT: (u32, u32) = (2, 32);

/// Replaces `scripts/preflight.ps1`: checks what a build and the gates need on every OS.
pub fn run() -> Result<(), String> {
    let checks = [
        ("rustc", check_rustc()),
        ("git", check_git()),
        ("C compiler", check_c_compiler()),
        ("Cargo.lock", check_lockfile()),
    ];
    let mut failed = 0;
    for (name, result) in &checks {
        match result {
            Ok(detail) => println!("ok    {name}: {detail}"),
            Err(problem) => {
                failed += 1;
                println!("FAIL  {name}: {problem}");
            }
        }
    }
    if let Some(warning) = onedrive_warning() {
        println!("warn  target dir: {warning}");
    }
    if failed == 0 {
        Ok(())
    } else {
        Err(format!("{failed} doctor check(s) failed"))
    }
}

fn repo_root() -> PathBuf {
    let xtask_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    xtask_dir.parent().unwrap_or(xtask_dir).to_path_buf()
}

fn check_rustc() -> Result<String, String> {
    let version = run::stdout(Command::new("rustc").arg("--version"))?;
    if version.starts_with(&format!("rustc {RUST_VERSION} ")) {
        Ok(version)
    } else {
        Err(format!("{version}; rust-toolchain.toml pins {RUST_VERSION}"))
    }
}

fn check_git() -> Result<String, String> {
    let version = run::stdout(Command::new("git").arg("--version"))?;
    match parse_git_version(&version) {
        Some(found) if found >= MIN_GIT => Ok(version),
        Some(_) => Err(format!(
            "{version}; need git >= {}.{} (GIT_CONFIG_GLOBAL)",
            MIN_GIT.0, MIN_GIT.1
        )),
        None => Err(format!("cannot parse `{version}`")),
    }
}

fn parse_git_version(text: &str) -> Option<(u32, u32)> {
    let mut parts = text.strip_prefix("git version ")?.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    Some((major, minor))
}

/// lmdb-sys and libmimalloc-sys compile C, so every OS needs a working C compiler.
fn check_c_compiler() -> Result<String, String> {
    if cfg!(windows) {
        // cc-rs locates MSVC through vswhere, which every Visual Studio install ships.
        let vswhere = std::env::var_os("ProgramFiles(x86)").map(|dir| {
            Path::new(&dir).join("Microsoft Visual Studio/Installer/vswhere.exe")
        });
        return match vswhere {
            Some(path) if path.is_file() => Ok(path.display().to_string()),
            _ => Err("vswhere.exe not found; install Visual Studio Build Tools (C++)".to_string()),
        };
    }
    let compiler = std::env::var("CC").unwrap_or_else(|_| "cc".to_string());
    run::stdout(Command::new(&compiler).arg("--version"))
        .map(|output| output.lines().next().unwrap_or_default().to_string())
        .map_err(|error| format!("{error}; install a C compiler (Xcode CLT, build-essential)"))
}

fn check_lockfile() -> Result<String, String> {
    let lockfile = repo_root().join("Cargo.lock");
    if lockfile.is_file() {
        Ok(lockfile.display().to_string())
    } else {
        Err(format!("{} is missing", lockfile.display()))
    }
}

/// Windows App Control blocks binaries built inside OneDrive; warn instead of failing.
fn onedrive_warning() -> Option<String> {
    if !cfg!(windows) {
        return None;
    }
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| repo_root().join("target"));
    let roots: Vec<PathBuf> = ["OneDrive", "OneDriveCommercial", "OneDriveConsumer"]
        .iter()
        .filter_map(std::env::var_os)
        .map(PathBuf::from)
        .collect();
    inside_any(&target, &roots).then(|| {
        format!(
            "{} is inside OneDrive; set CARGO_TARGET_DIR outside it",
            target.display()
        )
    })
}

fn inside_any(path: &Path, roots: &[PathBuf]) -> bool {
    roots.iter().any(|root| path.starts_with(root))
}
```

In `xtask/src/main.rs`, change the usage line and add the dispatch arm:

```rust
const USAGE: &str = "usage: cargo xtask <doctor | gate [--fast]>";
```

```rust
        ["doctor"] => doctor::run(),
```

- [ ] **Step 4: Run the tests, then the doctor**

Run: `cargo test -p xtask`. Expected: 5 passed.
Run: `cargo xtask doctor`. Expected on the owner's Mac: four `ok` lines (rustc 1.94.0, git 2.39.5, Apple clang, Cargo.lock) and exit code 0.

- [ ] **Step 5: Commit**

```bash
git add xtask
git commit -m "feat(xtask): add cargo xtask doctor replacing preflight.ps1"
```

---

### Task 3: Gate parity, CI legs and ADR 0020

**Files:**
- Modify: `.github/workflows/rust-ci.yml`, `docs/adr/README.md`
- Create: `docs/adr/0020-portable-automation-cargo-xtask.md`

**Interfaces:**
- Consumes: `cargo xtask gate [--fast]` and `cargo xtask doctor` from Tasks 1 and 2.

- [ ] **Step 1: Run old and new gates on 4 trees and record the exit codes**

For each tree below, run both gates. Record both exit codes, then restore the tree with `git checkout -- <file>` (and `rm` for the new file in tree 4).

```bash
pwsh -NoProfile -File scripts/quality-gate.ps1 -SkipPreflight; echo "old=$?"
cargo xtask gate; echo "new=$?"
```

| Tree | How to make it | Expected (old, new) |
|---|---|---|
| clean | nothing | 0, 0 |
| fmt violation | change `fn main() -> ExitCode {` in `xtask/src/main.rs` to `fn main()   -> ExitCode {` | both fail at fmt |
| clippy warning | append the clippy probe below to `xtask/src/run.rs` (`clippy::clone_on_copy`) | both fail at clippy |
| failing test | create `apps/desktop/src-tauri/tests/parity_probe.rs` with the test probe below | both fail at test; with `-Fast` / `--fast`, both pass |

The probes are formatted already, so `rustfmt` passes and the intended step is the one that fails.

Clippy probe:

```rust

pub fn parity_probe(value: u32) -> u32 {
    value.clone()
}
```

Test probe:

```rust
#[test]
fn parity_probe() {
    assert!(std::env::args().count() == 0);
}
```

Expected: the old and new exit codes match on every row.

- [ ] **Step 2: Add the CI legs**

In `.github/workflows/rust-ci.yml`, job `backend-quality`, change the matrix:

```yaml
        os: [ubuntu-latest, windows-latest, macos-latest]
```

After the `Unit Tests` step, add:

```yaml
      - name: xtask doctor
        run: cargo xtask doctor

      - name: xtask gate (parity with the steps above)
        run: cargo xtask gate
```

- [ ] **Step 3: Write ADR 0020 and list it**

Create `docs/adr/0020-portable-automation-cargo-xtask.md`. Base it on the spec's Decisions table and Phases table. Add a "Phase 1 results" section containing the parity table from Step 1, with the real exit codes, and the CI run ID from Step 5. Status: `Accepted (Phase 1 of 4)`. State that the guardrail mapping table is added in Phase 2.

Append to `docs/adr/README.md`, after the `0019-...` line:

```md
21. `0020-portable-automation-cargo-xtask.md`
```

(Renumber the `0026-...` line to 22.)

- [ ] **Step 4: Full local verification**

Run: `cargo xtask doctor && cargo xtask gate && pwsh -NoProfile -File scripts/enforce-guardrails.ps1`
Expected: all of them exit 0, and the guardrails print `Guardrail check passed.`

- [ ] **Step 5: Commit, push and watch CI**

```bash
git add .github/workflows/rust-ci.yml docs/adr
git commit -m "ci: run xtask doctor and gate on macOS, Linux and Windows"
git push
gh run watch <run-id> --exit-status
```

Expected: Backend Quality passes on ubuntu-latest, windows-latest and macos-latest, including both xtask steps. Put the run ID into ADR 0020. That edit is docs-only, so it goes in with the Phase 2 commit (the anti-tamper rule blocks a docs-only commit).
