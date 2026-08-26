//! `elara` interpreter / REPL CLI.

use std::env;
use std::fs;
use std::io::{self, Write};
use std::process::ExitCode;

use elara::compiler::{compile_script, disassemble_proto, set_compile_error};
use elara::ffi::c_api::{
    elara_call, elara_close, elara_last_error, elara_load_bytecode, elara_open_with_config,
};
use elara::ffi::config::ElaraConfig;
use elara::ffi::sandbox::{ELARA_CAP_ALL, ELARA_CAP_SAFE_DEFAULT, ELARA_CAP_STDIO};
use elara::vm::{ElaraStatus, serialize_elac, verify_proto};

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        print_help();
        return ExitCode::SUCCESS;
    }
    if args.iter().any(|a| a == "-v" || a == "--version") {
        println!("{}", elara::version());
        return ExitCode::SUCCESS;
    }

    let mut gas: Option<u64> = None;
    let mut mem_limit: Option<usize> = None;
    let mut sandbox = false;
    let mut disasm = false;
    let mut lib_dirs: Vec<String> = Vec::new();
    let mut eval_code: Option<String> = None;
    let mut file: Option<String> = None;
    let mut script_args: Vec<String> = Vec::new();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-e" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("elara: -e requires a code string");
                    return ExitCode::from(2);
                }
                eval_code = Some(args[i].clone());
            }
            "--gas" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("elara: --gas requires a value");
                    return ExitCode::from(2);
                }
                match args[i].parse::<u64>() {
                    Ok(n) => gas = Some(n),
                    Err(_) => {
                        eprintln!("elara: invalid --gas value");
                        return ExitCode::from(2);
                    }
                }
            }
            "--mem-limit" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("elara: --mem-limit requires a value");
                    return ExitCode::from(2);
                }
                match args[i].parse::<usize>() {
                    Ok(n) => mem_limit = Some(n),
                    Err(_) => {
                        eprintln!("elara: invalid --mem-limit value");
                        return ExitCode::from(2);
                    }
                }
            }
            "--sandbox" => sandbox = true,
            "--disasm" => disasm = true,
            "--lib" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("elara: --lib requires a directory");
                    return ExitCode::from(2);
                }
                lib_dirs.push(args[i].clone());
            }
            other if other.starts_with('-') => {
                eprintln!("elara: unknown option {other}");
                return ExitCode::from(2);
            }
            _ => {
                file = Some(args[i].clone());
                script_args = args[i + 1..].to_vec();
                break;
            }
        }
        i += 1;
    }

    let mut cfg = ElaraConfig::default_config();
    cfg.capabilities = if sandbox {
        ELARA_CAP_SAFE_DEFAULT | ELARA_CAP_STDIO
    } else {
        ELARA_CAP_ALL
    };
    if let Some(m) = mem_limit {
        cfg.mem_limit_bytes = m;
    }
    if let Some(g) = gas {
        cfg.gas_limit = g;
    }

    let state = unsafe { elara_open_with_config(&cfg) };
    if state.is_null() {
        eprintln!("elara: failed to open state");
        return ExitCode::from(1);
    }
    let anchor = if eval_code.is_some() {
        ".".to_string()
    } else if let Some(ref path) = file {
        script_anchor(path)
    } else {
        ".".to_string()
    };
    let mut dirs = vec![anchor.clone()];
    if anchor != "." {
        dirs.push(".".to_string());
    }
    dirs.extend(lib_dirs);
    if let Ok(path) = env::var("ELARA_PATH") {
        for part in path.split(':') {
            if !part.is_empty() {
                dirs.push(part.to_string());
            }
        }
    }
    unsafe {
        (*state).cli_args = script_args;
        (*state).module_anchor = Some(anchor);
        (*state).module_search_dirs = dirs;
    }

    let status = if let Some(code) = eval_code {
        run_source(state, "=stdin", &code, disasm)
    } else if let Some(path) = file {
        run_file(state, &path, disasm)
    } else {
        run_repl(state)
    };

    if let Some(code) = unsafe { (*state).exit_code } {
        unsafe { elara_close(state) };
        return ExitCode::from(code as u8);
    }

    let exit = match status {
        ElaraStatus::Ok => ExitCode::SUCCESS,
        ElaraStatus::ErrGas => {
            eprintln!("elara: {}", unsafe { error_msg(state) });
            ExitCode::from(1)
        }
        other => {
            eprintln!("elara: {:?} — {}", other, unsafe { error_msg(state) });
            ExitCode::from(1)
        }
    };
    unsafe { elara_close(state) };
    exit
}

fn script_anchor(path: &str) -> String {
    match std::path::Path::new(path).parent() {
        Some(parent) => {
            let text = parent.to_string_lossy();
            if text.is_empty() {
                ".".to_string()
            } else {
                text.into_owned()
            }
        }
        None => ".".to_string(),
    }
}

unsafe fn error_msg(state: *mut elara::vm::ElaraState) -> String {
    unsafe {
        let p = elara_last_error(state);
        if p.is_null() {
            return String::new();
        }
        core::ffi::CStr::from_ptr(p).to_string_lossy().into_owned()
    }
}

fn run_file(state: *mut elara::vm::ElaraState, path: &str, disasm: bool) -> ElaraStatus {
    let data = match fs::read(path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("elara: cannot read {path}: {e}");
            return ElaraStatus::ErrRuntime;
        }
    };
    if path.ends_with(".elac") || looks_like_elac(&data) {
        return run_bytecode(state, &data, disasm);
    }
    let src = match String::from_utf8(data) {
        Ok(s) => s,
        Err(_) => {
            eprintln!("elara: source is not valid UTF-8");
            return ElaraStatus::ErrSyntax;
        }
    };
    run_source(state, path, &src, disasm)
}

fn looks_like_elac(data: &[u8]) -> bool {
    data.len() >= 6 && data[0] == 0x1B && &data[1..6] == b"ELARA"
}

fn run_source(
    state: *mut elara::vm::ElaraState,
    name: &str,
    source: &str,
    disasm: bool,
) -> ElaraStatus {
    unsafe {
        let proto = match compile_script(&mut *state, name, source) {
            Ok(p) => p,
            Err(e) => {
                set_compile_error(&mut *state, &e);
                return ElaraStatus::ErrSyntax;
            }
        };
        if let Err(msg) = verify_proto(&*proto, None) {
            (*state).last_error = Some(msg);
            return ElaraStatus::ErrVerify;
        }
        if disasm {
            print!("{}", disassemble_proto(&*proto));
        }
        match serialize_elac(proto, 0) {
            Ok(bytes) => run_bytecode(state, &bytes, false),
            Err(msg) => {
                (*state).last_error = Some(msg);
                ElaraStatus::ErrVerify
            }
        }
    }
}

fn run_bytecode(state: *mut elara::vm::ElaraState, bytes: &[u8], _disasm: bool) -> ElaraStatus {
    unsafe {
        let mut closure = 0u64;
        let st = elara_load_bytecode(state, bytes.as_ptr(), bytes.len(), &mut closure);
        if st != ElaraStatus::Ok {
            return st;
        }
        let mut n = 0u16;
        elara_call(
            state,
            closure,
            core::ptr::null(),
            0,
            core::ptr::null_mut(),
            0,
            &mut n,
        )
    }
}

fn run_repl(state: *mut elara::vm::ElaraState) -> ElaraStatus {
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut buffer = String::new();
    let mut brace_depth: i32 = 0;
    loop {
        let prompt = if brace_depth > 0 { "....> " } else { "elara> " };
        let _ = write!(stdout, "{prompt}");
        let _ = stdout.flush();
        let mut line = String::new();
        match stdin.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(e) => {
                eprintln!("elara: read error: {e}");
                return ElaraStatus::ErrRuntime;
            }
        }
        for ch in line.chars() {
            match ch {
                '{' => brace_depth += 1,
                '}' => brace_depth -= 1,
                _ => {}
            }
        }
        buffer.push_str(&line);
        if brace_depth > 0 {
            continue;
        }
        if buffer.trim().is_empty() {
            buffer.clear();
            continue;
        }
        if buffer.trim() == "exit" || buffer.trim() == "quit" {
            break;
        }
        let st = run_source(state, "=repl", &buffer, false);
        if st != ElaraStatus::Ok {
            eprintln!("{}", unsafe { error_msg(state) });
        }
        buffer.clear();
        brace_depth = 0;
    }
    ElaraStatus::Ok
}

fn print_help() {
    println!(
        "Usage: elara [options] [file.ela|file.elac] [args...]\n\
         \n\
         Options:\n\
           -e <code>          Evaluate inline code and exit\n\
           --gas <N>          Instruction gas limit\n\
           --mem-limit <N>    Heap memory limit in bytes\n\
           --sandbox          Restrict capabilities (safe default + stdio)\n\
           --lib <dir>        Add a library search directory\n\
           --disasm           Disassemble bytecode before running\n\
           -v, --version      Print version\n\
           -h, --help         Show this help"
    );
}
