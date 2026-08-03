//! End-to-end demo suite regression (source vs AOT `.elac`).

use std::path::PathBuf;
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("repo root")
        .to_path_buf()
}

fn demo_files() -> Vec<PathBuf> {
    let demos = repo_root().join("demos");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&demos)
        .expect("demos/")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.extension().and_then(|x| x.to_str()) == Some("ela")
                && p.file_name()
                    .and_then(|x| x.to_str())
                    .is_some_and(|n| n.starts_with('0'))
        })
        .collect();
    files.sort();
    files
}

#[test]
fn all_ela_demos_execute_identically_from_source_and_aot_elac_bytecode() {
    let elara = env!("CARGO_BIN_EXE_elara");
    let elarac = env!("CARGO_BIN_EXE_elarac");
    let files = demo_files();
    assert!(
        files.len() >= 4,
        "expected demos 01..04 present, found {}",
        files.len()
    );

    for src in &files {
        let name = src.file_stem().unwrap().to_string_lossy();
        let bc = std::env::temp_dir().join(format!("{name}.elac"));

        let from_src = Command::new(elara)
            .arg(src)
            .output()
            .unwrap_or_else(|e| panic!("run source {src:?}: {e}"));
        assert!(
            from_src.status.success(),
            "source failed {:?}: {}",
            src,
            String::from_utf8_lossy(&from_src.stderr)
        );

        let compile = Command::new(elarac)
            .args(["-o", bc.to_str().unwrap(), src.to_str().unwrap()])
            .output()
            .unwrap_or_else(|e| panic!("elarac {src:?}: {e}"));
        assert!(
            compile.status.success(),
            "elarac failed {:?}: {}",
            src,
            String::from_utf8_lossy(&compile.stderr)
        );

        let from_bc = Command::new(elara)
            .arg(&bc)
            .output()
            .unwrap_or_else(|e| panic!("run bytecode {bc:?}: {e}"));
        assert!(
            from_bc.status.success(),
            "bytecode failed {:?}: {}",
            bc,
            String::from_utf8_lossy(&from_bc.stderr)
        );
        assert_eq!(
            from_src.stdout, from_bc.stdout,
            "stdout mismatch for {name}"
        );
        let _ = std::fs::remove_file(&bc);
    }
}
