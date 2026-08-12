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

/// Write a file only its owner can read.
///
/// For the files that name credentials or invite the user to add them: `agents.toml`
/// says which account an agent runs as, and `mcp.json` carries an `env` block per
/// server, which is where a server's API token ends up. The mode is set explicitly
/// rather than left to the process umask, which on a default macOS or Linux install
/// leaves a new file readable by every other account on the machine.
pub fn write_private(path: &Path, contents: impl AsRef<[u8]>) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        // `mode` applies only when this call creates the file, so a new file is never
        // briefly wider than 0o600. `set_permissions` afterwards is what narrows a
        // file that already exists — including one written before this function did.
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(contents.as_ref())?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, contents)
    }
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn scratch() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tervin-paths-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("could not create a scratch directory");
        dir
    }

    fn mode_of(path: &Path) -> u32 {
        std::fs::metadata(path)
            .expect("the file should exist")
            .permissions()
            .mode()
    }

    #[test]
    fn a_private_write_is_unreadable_by_other_accounts() {
        let dir = scratch();
        let path = dir.join("agents.toml");
        write_private(&path, "contents").expect("write failed");
        assert_eq!(
            mode_of(&path) & 0o077,
            0,
            "group and other must have no access at all"
        );
        assert_eq!(
            std::fs::read_to_string(&path).expect("read failed"),
            "contents",
            "narrowing the mode must not cost the contents"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_private_write_narrows_a_file_that_already_existed() {
        // The upgrade case, and the reason `set_permissions` runs even when the file
        // was opened with a mode: an `agents.toml` written by an earlier Tervin is
        // already on disk at whatever the umask gave it.
        let dir = scratch();
        let path = dir.join("agents.toml");
        std::fs::write(&path, "old").expect("write failed");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))
            .expect("chmod failed");

        write_private(&path, "new").expect("write failed");
        assert_eq!(
            mode_of(&path) & 0o077,
            0,
            "a world-readable file must not stay that way"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
