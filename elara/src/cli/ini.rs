//! `elara.ini` parser for library search directories. Used by the `elara` CLI only.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

/// Reads `lib_path` entries from `path`.
///
/// A missing file yields an empty list. A read or syntax error names `path` and a 1-based line.
pub fn load_lib_paths(path: &Path) -> Result<Vec<String>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => {
            return Err(format!("elara: {}:1: {err}", path.display()));
        }
    };
    parse_lib_paths(path, &text)
}

fn parse_lib_paths(path: &Path, text: &str) -> Result<Vec<String>, String> {
    let base = path
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut dirs = Vec::new();
    for (idx, raw) in text.lines().enumerate() {
        let line_no = idx as u32 + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            return Err(format!(
                "elara: {}:{}: section headers are not allowed",
                path.display(),
                line_no
            ));
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(format!(
                "elara: {}:{}: expected key = value",
                path.display(),
                line_no
            ));
        };
        let key = key.trim();
        let value = value.trim();
        if key != "lib_path" {
            return Err(format!(
                "elara: {}:{}: unknown key '{key}'",
                path.display(),
                line_no
            ));
        }
        if value.is_empty() {
            return Err(format!(
                "elara: {}:{}: lib_path requires a directory",
                path.display(),
                line_no
            ));
        }
        dirs.push(resolve_dir(base, value));
    }
    Ok(dirs)
}

fn resolve_dir(base: &Path, value: &str) -> String {
    let path = Path::new(value);
    if path.is_absolute() {
        value.to_string()
    } else {
        base.join(path).to_string_lossy().into_owned()
    }
}

/// System ini path. `ELARA_SYS_INI` empty skips it; unset uses `/etc/elara/elara.ini`.
pub fn system_ini_path() -> Option<PathBuf> {
    match std::env::var("ELARA_SYS_INI") {
        Err(_) => Some(PathBuf::from("/etc/elara/elara.ini")),
        Ok(path) if path.is_empty() => None,
        Ok(path) => Some(PathBuf::from(path)),
    }
}
