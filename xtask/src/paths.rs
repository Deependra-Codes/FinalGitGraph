use std::path::{Path, PathBuf};

/// The repository root: the directory above `xtask/`.
pub fn repo_root() -> PathBuf {
    let xtask_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    xtask_dir.parent().unwrap_or(xtask_dir).to_path_buf()
}
