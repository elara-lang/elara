//! End-to-end demo suite regression (source vs AOT `.elac`, C99 FFI, runner).

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
        files.len() >= 7,
        "expected demos 01..07 present, found {}",
        files.len()
    );

    for src in &files {
        let name = src.file_stem().unwrap().to_string_lossy();
        // Keep `.elac` beside the source so relative `import` search dirs resolve.
        let bc = src.with_extension("elac");

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

#[test]
fn c_ffi_demo_compiles_and_passes_host_sandbox_assertions() {
    let root = repo_root();
    let status = Command::new("make")
        .arg("build")
        .current_dir(&root)
        .status()
        .expect("make build");
    assert!(status.success(), "make build failed");

    let lib = root.join("bin/libelara.a");
    let src = root.join("demos/08_c_ffi_sandbox_host.c");
    let out = std::env::temp_dir().join("elara_ffi_demo_test");
    let cc = std::env::var("CC").unwrap_or_else(|_| "cc".into());

    let compile = Command::new(&cc)
        .args([
            "-std=c99",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-I",
            root.join("elara/include").to_str().unwrap(),
            src.to_str().unwrap(),
            lib.to_str().unwrap(),
            "-lm",
            "-lpthread",
            "-ldl",
            "-o",
            out.to_str().unwrap(),
        ])
        .output()
        .unwrap_or_else(|e| panic!("cc: {e}"));
    assert!(
        compile.status.success(),
        "C FFI demo compile failed: {}",
        String::from_utf8_lossy(&compile.stderr)
    );

    let run = Command::new(&out)
        .output()
        .unwrap_or_else(|e| panic!("run ffi demo: {e}"));
    let _ = std::fs::remove_file(&out);
    assert!(
        run.status.success(),
        "C FFI demo failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(
        stdout.contains("c_ffi_sandbox_host: ok"),
        "unexpected stdout: {stdout}"
    );
}

#[test]
fn run_demos_script_succeeds_end_to_end() {
    let root = repo_root();
    let script = root.join("demos/run_demos.sh");
    let run = Command::new("bash")
        .arg(&script)
        .current_dir(&root)
        .output()
        .unwrap_or_else(|e| panic!("run_demos.sh: {e}"));
    assert!(
        run.status.success(),
        "run_demos.sh failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(
        stdout.contains("All 8 Elara demos passed (source + AOT .elac + C99 FFI)"),
        "missing success banner: {stdout}"
    );
}
