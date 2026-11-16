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

/// Git that cannot see the caller's setup: hooks export `GIT_DIR`, machine config changes defaults.
pub fn git(repo: &std::path::Path) -> Command {
    let mut command = Command::new("git");
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("GIT_") {
            command.env_remove(key);
        }
    }
    let null_device = if cfg!(windows) { "NUL" } else { "/dev/null" };
    command
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", null_device)
        .arg("-C")
        .arg(repo);
    command
}
