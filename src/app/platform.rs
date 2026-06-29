use std::path::{Path, PathBuf};

/// Database location: a `data` folder beside the executable (a portable
/// install) or, when that location is not writable (e.g. /usr/bin on Linux
/// or /Applications on macOS), a folder in the user's home directory.
pub fn default_db_path() -> PathBuf {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));
    let portable = exe_dir.join("data");
    if dir_is_writable(&portable) {
        return portable.join("base_search.db");
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".base-search").join("base_search.db")
}

fn dir_is_writable(dir: &Path) -> bool {
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_nanos())
        .unwrap_or_default();
    let probe = dir.join(format!(
        ".base-search-write-test-{}-{stamp}.tmp",
        std::process::id()
    ));
    let result = std::fs::write(&probe, b"ok").and_then(|_| std::fs::remove_file(&probe));
    result.is_ok()
}

pub(super) fn open_parent_folder(path: &Path) -> Result<(), String> {
    let folder = path.parent().unwrap_or_else(|| Path::new("."));
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = std::process::Command::new("explorer");
        command.arg(folder);
        command
    };
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = std::process::Command::new("open");
        command.arg(folder);
        command
    };
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let mut command = {
        let mut command = std::process::Command::new("xdg-open");
        command.arg(folder);
        command
    };
    command
        .spawn()
        .map(|_| ())
        .map_err(|err| format!("{}: {err}", folder.display()))
}
