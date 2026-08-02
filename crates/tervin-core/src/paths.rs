//! Local-first storage locations.
//!
//! Everything Tervin persists lives under the user's own config and data
//! directories. There is no cloud path here by design.

use std::path::{Path, PathBuf};

/// Where configuration lives.
///
/// Platform-dependent: `~/Library/Application Support/tervin` on macOS,
/// `~/.config/tervin` on Linux. Never write the path down in documentation or the
/// interface — resolve it, so what a user is told matches what exists.
pub fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("tervin")
}

/// Where the workspace database, block index, and event log live.
pub fn data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("tervin")
}

/// The single local workspace database.
pub fn workspace_db() -> PathBuf {
    data_dir().join("workspace.sqlite3")
}

/// Generated shell-integration scripts.
pub fn shell_dir() -> PathBuf {
    config_dir().join("shell")
}

/// Sockets and generated per-session files, valid only while Tervin runs.
///
/// Separate from `data_dir` because nothing here should survive a restart, and
/// because it is created with owner-only permissions: a socket in here is a way to
/// answer permission questions on Tervin's behalf, so another user's process must
/// not be able to reach it.
pub fn runtime_dir() -> PathBuf {
    data_dir().join("run")
}

/// Ensure the directories Tervin writes to exist.
pub fn ensure_dirs() -> std::io::Result<()> {
    std::fs::create_dir_all(config_dir())?;
    std::fs::create_dir_all(data_dir())?;
    std::fs::create_dir_all(shell_dir())?;
    create_private_dir(&runtime_dir())?;
    Ok(())
}

/// Create a directory only its owner can enter.
///
/// The permissions are the authentication for the hook socket, so they are set
/// explicitly rather than left to the process umask.
pub fn create_private_dir(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Shorten a path for display by replacing the home prefix with `~`.
///
/// Display-only. Never use the result to open anything.
pub fn abbreviate(path: &Path) -> String {
    if let Some(home) = dirs::home_dir() {
        if let Ok(rest) = path.strip_prefix(&home) {
            if rest.as_os_str().is_empty() {
                return "~".to_string();
            }
            return format!("~/{}", rest.display());
        }
    }
    path.display().to_string()
}
