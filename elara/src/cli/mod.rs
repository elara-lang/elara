//! CLI-only helpers for the `elara` binary. The VM does not read these files.

mod ini;

/// Search directories for one CLI run: anchor, `.`, `--lib`, local ini, `ELARA_PATH`, system ini.
pub fn module_search_dirs(anchor: &str, lib_flags: &[String]) -> Result<Vec<String>, String> {
    let mut dirs = vec![anchor.to_string()];
    if anchor != "." {
        dirs.push(".".to_string());
    }
    dirs.extend(lib_flags.iter().cloned());
    let local = PathBuf::from(anchor).join("elara.ini");
    dirs.extend(ini::load_lib_paths(&local)?);
    if let Ok(path) = std::env::var("ELARA_PATH") {
        for part in path.split(':') {
            if !part.is_empty() {
                dirs.push(part.to_string());
            }
        }
    }
    if let Some(system) = ini::system_ini_path() {
        dirs.extend(ini::load_lib_paths(&system)?);
    }
    Ok(dirs)
}

use std::path::PathBuf;
