//! Library search path, relative imports, and intrinsic shadowing.

#![allow(unsafe_op_in_unsafe_fn)]

use std::ffi::{CString, c_void};
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

use elara::compiler::compile_module;
use elara::ffi::c_api::{
    elara_eval, elara_is_nil, elara_last_error, elara_new_table, elara_register_native_module,
    elara_table_set, elara_val_i48,
};
use elara::ffi::{ELARA_CAP_NATIVE, ELARA_CAP_SAFE_DEFAULT};
use elara::value::Value;
use elara::vm::{ElaraState, ElaraStatus, serialize_elac};

static HOST_OPENS: AtomicU32 = AtomicU32::new(0);
static MATH_OPENS: AtomicU32 = AtomicU32::new(0);

unsafe extern "C" fn open_host_lib(
    state: *mut ElaraState,
    _ud: *mut c_void,
    out: *mut u64,
) -> ElaraStatus {
    HOST_OPENS.fetch_add(1, Ordering::SeqCst);
    let table = elara_new_table(state);
    if elara_is_nil(table) {
        return ElaraStatus::ErrOom;
    }
    let st = elara_table_set(state, table, c"n".as_ptr(), elara_val_i48(11));
    if st != ElaraStatus::Ok {
        return st;
    }
    *out = table;
    ElaraStatus::Ok
}

unsafe extern "C" fn open_math_shadow(
    state: *mut ElaraState,
    _ud: *mut c_void,
    out: *mut u64,
) -> ElaraStatus {
    MATH_OPENS.fetch_add(1, Ordering::SeqCst);
    *out = elara_new_table(state);
    ElaraStatus::Ok
}

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

#[test]
fn library_search_path_order_is_script_then_lib_flag_then_local_ini_then_env_then_system_ini() {
    let root = temp_dir("iniord");
    let proj = root.join("proj");
    let dot = root.join("dot");
    let lib = root.join("lib");
    let local = root.join("proj").join("libs");
    let env = root.join("env");
    let sys = root.join("sys");
    for dir in [&proj, &dot, &lib, &local, &env, &sys] {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::fs::write(proj.join("shadow.ela"), "export const id = \"script\"\n").unwrap();
    std::fs::write(dot.join("from_dot.ela"), "export const id = \"dot\"\n").unwrap();
    std::fs::write(lib.join("from_lib.ela"), "export const id = \"lib\"\n").unwrap();
    std::fs::write(local.join("from_lib.ela"), "export const id = \"local\"\n").unwrap();
    std::fs::write(
        local.join("from_local.ela"),
        "export const id = \"local\"\n",
    )
    .unwrap();
    std::fs::write(env.join("from_local.ela"), "export const id = \"env\"\n").unwrap();
    std::fs::write(env.join("from_env.ela"), "export const id = \"env\"\n").unwrap();
    std::fs::write(sys.join("from_env.ela"), "export const id = \"sys\"\n").unwrap();
    std::fs::write(sys.join("from_sys.ela"), "export const id = \"sys\"\n").unwrap();
    std::fs::write(proj.join("elara.ini"), "lib_path = libs\n").unwrap();
    let sys_ini = root.join("system.ini");
    std::fs::write(&sys_ini, format!("lib_path = {}\n", sys.display())).unwrap();
    let main = proj.join("main.ela");
    std::fs::write(
        &main,
        "import \"shadow\" as s\n\
         import \"from_dot\" as d\n\
         import \"from_lib\" as l\n\
         import \"from_local\" as c\n\
         import \"from_env\" as e\n\
         import \"from_sys\" as y\n\
         print(s.id)\n\
         print(d.id)\n\
         print(l.id)\n\
         print(c.id)\n\
         print(e.id)\n\
         print(y.id)\n",
    )
    .unwrap();

    let elara = env!("CARGO_BIN_EXE_elara");
    let out = Command::new(elara)
        .current_dir(&dot)
        .env("ELARA_PATH", env.as_os_str())
        .env("ELARA_SYS_INI", sys_ini.as_os_str())
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
        "script\ndot\nlib\nlocal\nenv\nsys\n"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn library_relative_lib_path_resolves_against_the_ini_directory() {
    let root = temp_dir("inirel");
    let proj = root.join("proj");
    let dot = root.join("dot");
    std::fs::create_dir_all(proj.join("libs")).unwrap();
    std::fs::create_dir_all(dot.join("libs")).unwrap();
    std::fs::write(proj.join("elara.ini"), "lib_path = libs\n").unwrap();
    std::fs::write(
        proj.join("libs/leaf.ela"),
        "export const id = \"from-ini\"\n",
    )
    .unwrap();
    std::fs::write(
        dot.join("libs/leaf.ela"),
        "export const id = \"from-cwd\"\n",
    )
    .unwrap();
    let main = proj.join("main.ela");
    std::fs::write(&main, "import \"leaf\" as m\nprint(m.id)\n").unwrap();

    let elara = env!("CARGO_BIN_EXE_elara");
    let out = Command::new(elara)
        .current_dir(&dot)
        .env("ELARA_SYS_INI", "")
        .env_remove("ELARA_PATH")
        .arg(main.as_os_str())
        .output()
        .expect("run elara");
    assert!(
        out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "from-ini\n");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn library_ini_syntax_error_stops_the_cli_before_the_script() {
    let root = temp_dir("inibad");
    std::fs::write(root.join("elara.ini"), "not_a_directive\n").unwrap();
    let main = root.join("main.ela");
    std::fs::write(&main, "print(1)\n").unwrap();
    let elara = env!("CARGO_BIN_EXE_elara");
    let out = Command::new(elara)
        .current_dir(&root)
        .env("ELARA_SYS_INI", "")
        .env_remove("ELARA_PATH")
        .arg(main.as_os_str())
        .output()
        .expect("run elara");
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    let ini = root.join("elara.ini");
    assert!(
        err.contains(&format!("{}:1:", ini.display())),
        "stderr={err}"
    );
    assert!(out.stdout.is_empty(), "script ran despite a bad ini");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn native_module_registered_by_host_loads_without_native_capability() {
    HOST_OPENS.store(0, Ordering::SeqCst);
    MATH_OPENS.store(0, Ordering::SeqCst);
    let mut state = ElaraState::new();
    state.capabilities = ELARA_CAP_SAFE_DEFAULT;
    assert_eq!(state.capabilities & ELARA_CAP_NATIVE, 0);
    let src = CString::new("import \"hostlib\" as h\nreturn h.n\n").unwrap();
    let mut out = 0u64;
    unsafe {
        elara_register_native_module(
            &mut state,
            c"hostlib".as_ptr(),
            Some(open_host_lib),
            core::ptr::null_mut(),
        );
        elara_register_native_module(
            &mut state,
            c"math".as_ptr(),
            Some(open_math_shadow),
            core::ptr::null_mut(),
        );
        let st = elara_eval(&mut state, c"main".as_ptr(), src.as_ptr(), &mut out);
        assert_eq!(st, ElaraStatus::Ok, "{}", last_error(&state));
    }
    assert_eq!(Value(out).to_i64(), Some(11));
    assert_eq!(HOST_OPENS.load(Ordering::SeqCst), 1);
    assert_eq!(MATH_OPENS.load(Ordering::SeqCst), 0);
}

#[test]
fn native_so_loads_when_capability_set_and_is_hidden_when_unset() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("repo root")
        .to_path_buf();
    let build = Command::new("make")
        .arg("build")
        .current_dir(&root)
        .output()
        .expect("make build");
    assert!(
        build.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr)
    );
    let so_dir = temp_dir("natso");
    let so = so_dir.join("native_demo.so");
    let cc = std::env::var("CC").unwrap_or_else(|_| "cc".to_string());
    let compiled = Command::new(cc)
        .args([
            "-std=c99", "-Wall", "-Wextra", "-Werror", "-shared", "-fPIC", "-I",
        ])
        .arg(root.join("elara/include"))
        .arg(root.join("demos/09_native_library.c"))
        .arg("-o")
        .arg(&so)
        .output()
        .expect("cc");
    assert!(
        compiled.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let elara = root.join("bin/elara");
    let script = root.join("demos/09_native_library.ela");
    let loaded = Command::new(&elara)
        .env("ELARA_SYS_INI", "")
        .env_remove("ELARA_PATH")
        .args(["--lib", so_dir.to_str().unwrap()])
        .arg(&script)
        .output()
        .expect("elara");
    assert!(
        loaded.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&loaded.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&loaded.stdout), "42\n");
    let hidden = Command::new(&elara)
        .env("ELARA_SYS_INI", "")
        .env_remove("ELARA_PATH")
        .args(["--sandbox", "--lib", so_dir.to_str().unwrap()])
        .arg(&script)
        .output()
        .expect("elara --sandbox");
    assert!(!hidden.status.success());
    let err = String::from_utf8_lossy(&hidden.stderr);
    assert!(
        err.contains("Module not found: 'native_demo'"),
        "stderr={err}"
    );
    let _ = std::fs::remove_dir_all(&so_dir);
}

fn last_error(state: &ElaraState) -> String {
    unsafe {
        let p = elara_last_error(state);
        if p.is_null() {
            String::new()
        } else {
            core::ffi::CStr::from_ptr(p).to_string_lossy().into_owned()
        }
    }
}
