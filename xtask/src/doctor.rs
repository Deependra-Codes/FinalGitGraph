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
        Err(format!(
            "{version}; rust-toolchain.toml pins {RUST_VERSION}"
        ))
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
        let vswhere = std::env::var_os("ProgramFiles(x86)")
            .map(|dir| Path::new(&dir).join("Microsoft Visual Studio/Installer/vswhere.exe"));
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

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{inside_any, parse_git_version};

    #[test]
    fn parses_git_versions_from_each_platform() {
        assert_eq!(
            parse_git_version("git version 2.39.5 (Apple Git-154)"),
            Some((2, 39))
        );
        assert_eq!(
            parse_git_version("git version 2.47.1.windows.1"),
            Some((2, 47))
        );
        assert_eq!(parse_git_version("git version 2.31.0"), Some((2, 31)));
        assert_eq!(parse_git_version("not git"), None);
    }

    #[test]
    fn detects_a_target_dir_inside_onedrive() {
        let roots = vec![PathBuf::from("/Users/a/OneDrive")];
        assert!(inside_any(
            Path::new("/Users/a/OneDrive/code/target"),
            &roots
        ));
        assert!(!inside_any(Path::new("/Users/a/code/target"), &roots));
        assert!(!inside_any(Path::new("/Users/a/code/target"), &[]));
    }
}
