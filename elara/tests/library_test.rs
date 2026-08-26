//! Library search path, relative imports, and intrinsic shadowing.

#![allow(unsafe_op_in_unsafe_fn)]

use std::ffi::CString;
use std::path::Path;
use std::process::Command;

use elara::compiler::compile_module;
use elara::ffi::c_api::{elara_eval, elara_last_error};
use elara::value::Value;
use elara::vm::{ElaraState, ElaraStatus, serialize_elac};

fn eval_in(dir: &Path, source: &str) -> (ElaraStatus, Value, String) {
    let mut state = ElaraState::new();
    let dir_s = dir.to_string_lossy().into_owned();
    state.module_anchor = Some(dir_s.clone());
    state.module_search_dirs = vec![dir_s];
    let src = CString::new(source).unwrap();
    let mut out = 0u64;
    let st = unsafe { elara_eval(&mut state, c"main".as_ptr(), src.as_ptr(), &mut out) };
    let err = unsafe {
        let p = elara_last_error(&state);
        if p.is_null() {
            String::new()
        } else {
            core::ffi::CStr::from_ptr(p).to_string_lossy().into_owned()
        }
    };
    (st, Value(out), err)
}

fn temp_dir(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "elara_{label}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn library_logical_name_prefers_elac_then_ela_then_init() {
    let dir = temp_dir("pref");
    std::fs::write(dir.join("pref.ela"), "export const id = 1\n").unwrap();
    let elac = unsafe {
        let mut state = ElaraState::new();
        let proto = compile_module(&mut state, "pref", "export const id = 2\n").expect("compile");
        serialize_elac(proto, 0).expect("serialize")
    };
    std::fs::write(dir.join("pref.elac"), elac).unwrap();

    let (st, val, err) = eval_in(&dir, "import \"pref\" as p\nreturn p.id\n");
    assert_eq!(st, ElaraStatus::Ok, "{err}");
    assert_eq!(val.to_i64(), Some(2));

    std::fs::remove_file(dir.join("pref.elac")).unwrap();
    let (st, val, err) = eval_in(&dir, "import \"pref\" as p\nreturn p.id\n");
    assert_eq!(st, ElaraStatus::Ok, "{err}");
    assert_eq!(val.to_i64(), Some(1));

    std::fs::remove_file(dir.join("pref.ela")).unwrap();
    std::fs::create_dir(dir.join("pref")).unwrap();
    std::fs::write(dir.join("pref/init.ela"), "export const id = 3\n").unwrap();
    let (st, val, err) = eval_in(&dir, "import \"pref\" as p\nreturn p.id\n");
    assert_eq!(st, ElaraStatus::Ok, "{err}");
    assert_eq!(val.to_i64(), Some(3));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn library_relative_import_uses_same_cache_key_as_logical_name() {
    let dir = temp_dir("rel");
    std::fs::create_dir_all(dir.join("vec")).unwrap();
    std::fs::write(dir.join("vec/geom.ela"), "export const n = 7\n").unwrap();
    std::fs::write(
        dir.join("vec/init.ela"),
        "import \"./geom\" as g\nexport const geom = g\n",
    )
    .unwrap();
    let (st, val, err) = eval_in(
        &dir,
        "import \"vec/geom\" as a\nimport \"vec\" as v\nif a == v.geom { return a.n }\nreturn 0\n",
    );
    assert_eq!(st, ElaraStatus::Ok, "{err}");
    assert_eq!(val.to_i64(), Some(7));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn library_relative_import_rejects_escape_and_absolute_specifier() {
    let dir = temp_dir("esc");
    let (st, _, err) = eval_in(&dir, "import \"../secret\" as s\nreturn 1\n");
    assert_eq!(st, ElaraStatus::ErrModule);
    assert_eq!(err, "Module path traversal rejected");

    let (st, _, err) = eval_in(&dir, "import \"/abs\" as s\nreturn 1\n");
    assert_eq!(st, ElaraStatus::ErrModule);
    assert_eq!(err, "Module path is absolute: '/abs'");

    let (st, _, err) = eval_in(&dir, "import \"foo/../bar\" as s\nreturn 1\n");
    assert_eq!(st, ElaraStatus::ErrModule);
    assert_eq!(err, "Module path traversal rejected");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn library_search_path_order_is_script_then_lib_flag_then_elara_path() {
    let root = temp_dir("path");
    let proj = root.join("proj");
    let dot = root.join("dot");
    let lib = root.join("lib");
    let env = root.join("env");
    for dir in [&proj, &dot, &lib, &env] {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::fs::write(proj.join("shadow.ela"), "export const id = \"script\"\n").unwrap();
    std::fs::write(dot.join("shadow.ela"), "export const id = \"dot\"\n").unwrap();
    std::fs::write(dot.join("from_dot.ela"), "export const id = \"dot\"\n").unwrap();
    std::fs::write(lib.join("from_lib.ela"), "export const id = \"lib\"\n").unwrap();
    std::fs::write(env.join("from_lib.ela"), "export const id = \"env\"\n").unwrap();
    std::fs::write(env.join("from_env.ela"), "export const id = \"env\"\n").unwrap();
    let main = proj.join("main.ela");
    std::fs::write(
        &main,
        "import \"shadow\" as s\n\
         import \"from_dot\" as d\n\
         import \"from_lib\" as l\n\
         import \"from_env\" as e\n\
         print(s.id)\n\
         print(d.id)\n\
         print(l.id)\n\
         print(e.id)\n",
    )
    .unwrap();

    let elara = env!("CARGO_BIN_EXE_elara");
    let out = Command::new(elara)
        .current_dir(&dot)
        .env("ELARA_PATH", env.as_os_str())
        .args(["--lib", lib.to_str().unwrap(), main.to_str().unwrap()])
        .output()
        .expect("run elara");
    assert!(
        out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "script\ndot\nlib\nenv\n"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn intrinsic_math_shadows_math_ela_on_the_search_path() {
    let dir = temp_dir("math");
    std::fs::write(dir.join("math.ela"), "export const marker = 1\n").unwrap();
    let (st, val, err) = eval_in(
        &dir,
        "import \"math.ela\" as a\nimport \"./math\" as b\nif a == math { if b == math { return 1 } return 2 }\nreturn 3\n",
    );
    assert_eq!(st, ElaraStatus::Ok, "{err}");
    assert_eq!(val.to_i64(), Some(1));
    let _ = std::fs::remove_dir_all(&dir);
}
