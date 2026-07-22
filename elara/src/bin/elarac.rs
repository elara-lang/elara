//! `elarac` AOT bytecode compiler CLI.

use std::env;
use std::fs;
use std::process::ExitCode;

use elara::compiler::{compile_script, disassemble_proto, set_compile_error};
use elara::ffi::c_api::{elara_close, elara_last_error, elara_open};
use elara::vm::{ElaraStatus, serialize_elac, verify_proto};

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() || args.iter().any(|a| a == "-h" || a == "--help") {
        print_help();
        return if args.is_empty() {
            ExitCode::from(2)
        } else {
            ExitCode::SUCCESS
        };
    }
    if args.iter().any(|a| a == "-v" || a == "--version") {
        println!("elarac ({})", elara::version());
        return ExitCode::SUCCESS;
    }

    let mut out_path: Option<String> = None;
    let mut strip = false;
    let mut check_only = false;
    let mut disasm = false;
    let mut input: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-o" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("elarac: -o requires a path");
                    return ExitCode::from(2);
                }
                out_path = Some(args[i].clone());
            }
            "--strip" => strip = true,
            "--check" => check_only = true,
            "--disasm" => disasm = true,
            other if other.starts_with('-') => {
                eprintln!("elarac: unknown option {other}");
                return ExitCode::from(2);
            }
            _ => {
                if input.is_some() {
                    eprintln!("elarac: unexpected argument {}", args[i]);
                    return ExitCode::from(2);
                }
                input = Some(args[i].clone());
            }
        }
        i += 1;
    }

    let Some(input) = input else {
        eprintln!("elarac: missing input file");
        return ExitCode::from(2);
    };

    let source = match fs::read_to_string(&input) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("elarac: cannot read {input}: {e}");
            return ExitCode::from(1);
        }
    };

    let state = unsafe { elara_open() };
    if state.is_null() {
        eprintln!("elarac: failed to open state");
        return ExitCode::from(1);
    }

    let status = unsafe {
        match compile_script(&mut *state, &input, &source) {
            Ok(proto) => {
                if let Err(msg) = verify_proto(&*proto, None) {
                    (*state).last_error = Some(msg);
                    ElaraStatus::ErrVerify
                } else {
                    if disasm {
                        print!("{}", disassemble_proto(&*proto));
                    }
                    if check_only {
                        ElaraStatus::Ok
                    } else {
                        let flags = u16::from(strip);
                        match serialize_elac(proto, flags) {
                            Ok(bytes) => {
                                let out = out_path.unwrap_or_else(|| default_out_path(&input));
                                match fs::write(&out, &bytes) {
                                    Ok(()) => ElaraStatus::Ok,
                                    Err(e) => {
                                        eprintln!("elarac: cannot write {out}: {e}");
                                        ElaraStatus::ErrRuntime
                                    }
                                }
                            }
                            Err(msg) => {
                                (*state).last_error = Some(msg);
                                ElaraStatus::ErrVerify
                            }
                        }
                    }
                }
            }
            Err(e) => {
                set_compile_error(&mut *state, &e);
                eprintln!("{}:{}", e.line, e.message);
                ElaraStatus::ErrSyntax
            }
        }
    };

    let exit = if status == ElaraStatus::Ok {
        ExitCode::SUCCESS
    } else {
        if status != ElaraStatus::ErrSyntax {
            let msg = unsafe {
                let p = elara_last_error(state);
                if p.is_null() {
                    String::new()
                } else {
                    core::ffi::CStr::from_ptr(p).to_string_lossy().into_owned()
                }
            };
            if !msg.is_empty() {
                eprintln!("elarac: {msg}");
            }
        }
        ExitCode::from(1)
    };
    unsafe { elara_close(state) };
    let _ = args;
    exit
}

fn default_out_path(input: &str) -> String {
    if let Some(stripped) = input.strip_suffix(".ela") {
        format!("{stripped}.elac")
    } else {
        format!("{input}.elac")
    }
}

fn print_help() {
    println!(
        "Usage: elarac [options] <input.ela>\n\
         \n\
         Options:\n\
           -o <out.elac>   Output path (default: replace .ela with .elac)\n\
           --strip         Set strip_debug_info flag in the .elac header\n\
           --check         Compile and verify without writing output\n\
           --disasm        Print a disassembly of the compiled prototype\n\
           -v, --version   Print version\n\
           -h, --help      Show this help"
    );
}
