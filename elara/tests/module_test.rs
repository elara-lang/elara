#![allow(unsafe_op_in_unsafe_fn)]

use std::sync::atomic::{AtomicUsize, Ordering};

use elara::compiler::{FunctionScope, Instruction, ObjProto, OpCode, module_basename};
use elara::ffi::ElaraModuleLoaderFn;
use elara::ffi::c_api::{elara_last_error, elara_set_module_loader};
use elara::value::{GcHeader, InternPool, ObjTable, ObjType, Value};
use elara::vm::{
    CallFrame, ElaraState, ElaraStatus, ObjClosure, ObjFiber, execute, serialize_elac,
};

#[test]
fn module_basename_strips_path_and_extensions() {
    assert_eq!(module_basename("math/vec"), "vec");
    assert_eq!(module_basename("math/vec.ela"), "vec");
    assert_eq!(module_basename("a/b/c.elac"), "c");
    assert_eq!(module_basename("solo"), "solo");
}

unsafe fn unlink_and_free_table(state: &mut ElaraState, t: *mut ObjTable) {
    let mut cursor = &mut state.all_objects;
    while !(*cursor).is_null() {
        if *cursor == t.cast() {
            *cursor = (*t.cast::<GcHeader>()).next;
            break;
        }
        cursor = &mut (*(*cursor)).next;
    }
    (*t).free_table(&mut state.alloc);
    state
        .alloc
        .free_bytes(t.cast::<u8>(), core::mem::size_of::<ObjTable>());
}

/// Builds a module that exports `answer = 42` and keeps a private local `hidden = 99`.
unsafe fn build_answer_module(state: &mut ElaraState) -> *mut ObjProto {
    let dedup = ObjTable::new(&mut state.alloc, &mut state.all_objects, 0, 1, 0, 8).expect("dedup");
    let proto = ObjProto::new(
        &mut state.alloc,
        &mut state.all_objects,
        0,
        core::ptr::null_mut(),
    )
    .expect("proto");
    let mut scope = FunctionScope::new(proto.as_ptr(), dedup.as_ptr());
    scope.begin_module(&mut state.alloc, 1).expect("begin");
    let priv_reg = scope.declare_local("hidden", false, false, 2).unwrap();
    scope
        .emit(
            &mut state.alloc,
            Instruction::make_asbx(OpCode::LoadI, priv_reg, 99),
            2,
        )
        .unwrap();
    scope.mark_initialized();
    let ans = scope.declare_local("answer", false, false, 3).unwrap();
    scope
        .emit(
            &mut state.alloc,
            Instruction::make_asbx(OpCode::LoadI, ans, 42),
            3,
        )
        .unwrap();
    scope.mark_initialized();
    scope
        .emit_export(
            &mut state.alloc,
            &mut state.all_objects,
            &mut state.strings,
            "answer",
            ans,
            3,
        )
        .unwrap();
    scope.finish_module(&mut state.alloc, 4).unwrap();
    unlink_and_free_table(state, dedup.as_ptr());
    let _ = scope;
    proto.as_ptr()
}

/// Module that only imports `dep` (no other exports).
unsafe fn build_import_only_module(state: &mut ElaraState, dep: &str) -> *mut ObjProto {
    let dedup = ObjTable::new(&mut state.alloc, &mut state.all_objects, 0, 1, 0, 8).expect("dedup");
    let proto = ObjProto::new(
        &mut state.alloc,
        &mut state.all_objects,
        0,
        core::ptr::null_mut(),
    )
    .expect("proto");
    // Leak dep string into a stable &'static for FunctionScope local name lifetime.
    let dep_owned: &'static str = Box::leak(dep.to_string().into_boxed_str());
    let mut scope = FunctionScope::new(proto.as_ptr(), dedup.as_ptr());
    scope.begin_module(&mut state.alloc, 1).unwrap();
    scope
        .compile_import(
            &mut state.alloc,
            &mut state.all_objects,
            &mut state.strings,
            dep_owned,
            None,
            2,
        )
        .unwrap();
    scope.finish_module(&mut state.alloc, 3).unwrap();
    unlink_and_free_table(state, dedup.as_ptr());
    let _ = scope;
    proto.as_ptr()
}

/// Main chunk: `import "spec" as alias` then optionally GetField answer into R[2].
unsafe fn build_main_import(
    state: &mut ElaraState,
    specs: &[(&str, Option<&str>)],
    fetch_answer_from: Option<u8>,
) -> *mut ObjProto {
    let dedup = ObjTable::new(&mut state.alloc, &mut state.all_objects, 0, 1, 0, 8).expect("dedup");
    let proto = ObjProto::new(
        &mut state.alloc,
        &mut state.all_objects,
        0,
        core::ptr::null_mut(),
    )
    .expect("proto");
    let mut scope = FunctionScope::new(proto.as_ptr(), dedup.as_ptr());
    // Not a module — plain chunk that returns imported values.
    for &(spec, alias) in specs {
        let spec_s: &'static str = Box::leak(spec.to_string().into_boxed_str());
        let alias_s = alias.map(|a| -> &'static str { Box::leak(a.to_string().into_boxed_str()) });
        scope
            .compile_import(
                &mut state.alloc,
                &mut state.all_objects,
                &mut state.strings,
                spec_s,
                alias_s,
                1,
            )
            .unwrap();
    }
    if let Some(reg) = fetch_answer_from {
        let key = elara::compiler::ensure_string_constant(
            &mut scope,
            &mut state.alloc,
            &mut state.all_objects,
            &mut state.strings,
            b"answer",
            2,
        )
        .unwrap();
        let key_u8 = u8::try_from(key).unwrap();
        let dest = scope.alloc_regs(1, 2).unwrap();
        scope
            .emit(
                &mut state.alloc,
                Instruction::make_abc(OpCode::GetField, dest, reg, key_u8),
                2,
            )
            .unwrap();
        scope
            .emit(
                &mut state.alloc,
                Instruction::make_abc(OpCode::Return, dest, 2, 0),
                3,
            )
            .unwrap();
    } else {
        // Return first import register (R[0]).
        scope
            .emit(
                &mut state.alloc,
                Instruction::make_abc(OpCode::Return, 0, 2, 0),
                2,
            )
            .unwrap();
    }
    unlink_and_free_table(state, dedup.as_ptr());
    let _ = scope;
    proto.as_ptr()
}

unsafe fn run_proto(state: &mut ElaraState, proto: *mut ObjProto) -> (ElaraStatus, Value) {
    let closure = ObjClosure::new(&mut state.alloc, &mut state.all_objects, 0, proto).expect("c");
    let stack = (1 + (*proto).max_stack_size as u32).max(16);
    let mut fiber =
        ObjFiber::new(&mut state.alloc, &mut state.all_objects, 0, stack, 8).expect("f");
    let f = fiber.as_mut();
    let cv = Value::from_obj(
        core::ptr::NonNull::new(closure.as_ptr().cast()).unwrap(),
        ObjType::Closure,
    );
    *f.stack = cv;
    f.open_top = 1;
    f.push_frame(&mut state.alloc, CallFrame::new(closure.as_ptr(), 1, 0, 1))
        .unwrap();
    let st = execute(state, fiber.as_ptr());
    let v = if st == ElaraStatus::Ok {
        *f.stack
    } else {
        Value::NIL
    };
    (st, v)
}

fn leak_bytes(buf: Vec<u8>) -> &'static [u8] {
    Box::leak(buf.into_boxed_slice())
}

#[test]
fn module_exports_functions_and_constants_while_isolating_private_locals() {
    let mut state = ElaraState::new();
    unsafe {
        // Codegen lock (prologue / export / return).
        let dedup =
            ObjTable::new(&mut state.alloc, &mut state.all_objects, 0, 1, 0, 8).expect("dedup");
        let proto = ObjProto::new(
            &mut state.alloc,
            &mut state.all_objects,
            0,
            core::ptr::null_mut(),
        )
        .expect("proto");
        let mut scope = FunctionScope::new(proto.as_ptr(), dedup.as_ptr());
        scope.begin_module(&mut state.alloc, 1).expect("begin");
        let priv_reg = scope.declare_local("hidden", false, false, 2).unwrap();
        assert_eq!(priv_reg, 1);
        scope
            .emit(
                &mut state.alloc,
                Instruction::make_asbx(OpCode::LoadI, priv_reg, 99),
                2,
            )
            .unwrap();
        scope.mark_initialized();
        let ans = scope.declare_local("answer", false, false, 3).unwrap();
        assert_eq!(ans, 2);
        scope
            .emit(
                &mut state.alloc,
                Instruction::make_asbx(OpCode::LoadI, ans, 42),
                3,
            )
            .unwrap();
        scope.mark_initialized();
        scope
            .emit_export(
                &mut state.alloc,
                &mut state.all_objects,
                &mut state.strings,
                "answer",
                ans,
                3,
            )
            .unwrap();
        scope.finish_module(&mut state.alloc, 4).unwrap();
        let p = proto.as_ref();
        assert_eq!(p.code_count, 5);
        assert_eq!(
            *p.code.add(0),
            Instruction::make_abc(OpCode::NewTable, 0, 0, 4)
        );
        assert_eq!(
            *p.code.add(4),
            Instruction::make_abc(OpCode::Return, 0, 2, 0)
        );

        // Runtime: load via host loader and observe exports / isolation.
        let mod_bytes = leak_bytes(serialize_elac(proto.as_ptr(), 0).expect("ser"));
        static MOD: AtomicUsize = AtomicUsize::new(0);
        // Store pointer in AtomicUsize for the loader (test-only).
        MOD.store(mod_bytes.as_ptr() as usize, Ordering::SeqCst);
        static LEN: AtomicUsize = AtomicUsize::new(0);
        LEN.store(mod_bytes.len(), Ordering::SeqCst);

        unsafe extern "C" fn loader(
            _s: *mut ElaraState,
            specifier: *const core::ffi::c_char,
            out_bytes: *mut *const u8,
            out_len: *mut usize,
            out_is_bc: *mut bool,
            _ud: *mut core::ffi::c_void,
        ) -> ElaraStatus {
            let name = core::ffi::CStr::from_ptr(specifier).to_bytes();
            if name != b"answer_mod" {
                return ElaraStatus::ErrModule;
            }
            *out_bytes = MOD.load(Ordering::SeqCst) as *const u8;
            *out_len = LEN.load(Ordering::SeqCst);
            *out_is_bc = true;
            ElaraStatus::Ok
        }

        elara_set_module_loader(&mut state, Some(loader), core::ptr::null_mut());
        let main = build_main_import(&mut state, &[("answer_mod", Some("m"))], Some(0));
        let (st, v) = run_proto(&mut state, main);
        assert_eq!(st, ElaraStatus::Ok);
        assert_eq!(v.to_i64(), Some(42));

        // Ensure "hidden" was never placed into the cached exports table.
        let key_hidden = state
            .strings
            .new_string(&mut state.alloc, &mut state.all_objects, 0, b"hidden")
            .unwrap();
        let key_mod = state
            .strings
            .new_string(&mut state.alloc, &mut state.all_objects, 0, b"answer_mod")
            .unwrap();
        let kv = Value::from_obj(key_mod.cast(), ObjType::String);
        let exports = (*state.modules_table).get(kv);
        assert!(exports.is_obj_type(ObjType::Table));
        let hidden = (*exports.as_obj().cast::<ObjTable>())
            .get(Value::from_obj(key_hidden.cast(), ObjType::String));
        assert!(hidden.is_nil());

        let _ = (scope, dedup, core::ptr::null_mut::<InternPool>());
    }
}

static CACHE_LOADS: AtomicUsize = AtomicUsize::new(0);
static CACHE_PTR: AtomicUsize = AtomicUsize::new(0);
static CACHE_LEN: AtomicUsize = AtomicUsize::new(0);

unsafe extern "C" fn cache_loader(
    _s: *mut ElaraState,
    specifier: *const core::ffi::c_char,
    out_bytes: *mut *const u8,
    out_len: *mut usize,
    out_is_bc: *mut bool,
    _ud: *mut core::ffi::c_void,
) -> ElaraStatus {
    let name = core::ffi::CStr::from_ptr(specifier).to_bytes();
    if name != b"once" {
        return ElaraStatus::ErrModule;
    }
    CACHE_LOADS.fetch_add(1, Ordering::SeqCst);
    *out_bytes = CACHE_PTR.load(Ordering::SeqCst) as *const u8;
    *out_len = CACHE_LEN.load(Ordering::SeqCst);
    *out_is_bc = true;
    ElaraStatus::Ok
}

#[test]
fn importing_same_module_twice_returns_cached_singleton_without_reexecuting() {
    let mut state = ElaraState::new();
    unsafe {
        // Codegen: two imports share K index.
        let dedup =
            ObjTable::new(&mut state.alloc, &mut state.all_objects, 0, 1, 0, 8).expect("dedup");
        let proto = ObjProto::new(
            &mut state.alloc,
            &mut state.all_objects,
            0,
            core::ptr::null_mut(),
        )
        .expect("proto");
        let mut scope = FunctionScope::new(proto.as_ptr(), dedup.as_ptr());
        scope.begin_module(&mut state.alloc, 1).unwrap();
        let r1 = scope
            .compile_import(
                &mut state.alloc,
                &mut state.all_objects,
                &mut state.strings,
                "math/vec.ela",
                None,
                2,
            )
            .unwrap();
        assert_eq!(r1, 1);
        assert_eq!(scope.locals[0].name, "vec");
        let r2 = scope
            .compile_import(
                &mut state.alloc,
                &mut state.all_objects,
                &mut state.strings,
                "math/vec.ela",
                Some("vec2"),
                3,
            )
            .unwrap();
        assert_eq!(r2, 2);
        scope.finish_module(&mut state.alloc, 4).unwrap();
        let p = proto.as_ref();
        let i1 = *p.code.add(1);
        let i2 = *p.code.add(2);
        assert_eq!(i1.opcode(), Some(OpCode::Import));
        assert_eq!(i2.opcode(), Some(OpCode::Import));
        assert_eq!(i1.bx(), i2.bx());

        // Runtime singleton cache.
        let mod_proto = build_answer_module(&mut state);
        let bytes = leak_bytes(serialize_elac(mod_proto, 0).expect("ser"));
        CACHE_LOADS.store(0, Ordering::SeqCst);
        CACHE_PTR.store(bytes.as_ptr() as usize, Ordering::SeqCst);
        CACHE_LEN.store(bytes.len(), Ordering::SeqCst);
        elara_set_module_loader(&mut state, Some(cache_loader), core::ptr::null_mut());

        let main = build_main_import(
            &mut state,
            &[("once", Some("a")), ("once", Some("b"))],
            None,
        );
        // Extend main to also return R[1] equality check via comparing pointers in test after exec.
        // Instead: after run, inspect modules_table and CACHE_LOADS.
        let (st, _v) = run_proto(&mut state, main);
        assert_eq!(st, ElaraStatus::Ok);
        assert_eq!(CACHE_LOADS.load(Ordering::SeqCst), 1);

        let key = state
            .strings
            .new_string(&mut state.alloc, &mut state.all_objects, 0, b"once")
            .unwrap();
        let exports = (*state.modules_table).get(Value::from_obj(key.cast(), ObjType::String));
        assert!(exports.is_obj_type(ObjType::Table));

        let _ = (scope, dedup);
    }
}

static CYCLE_A_PTR: AtomicUsize = AtomicUsize::new(0);
static CYCLE_A_LEN: AtomicUsize = AtomicUsize::new(0);
static CYCLE_B_PTR: AtomicUsize = AtomicUsize::new(0);
static CYCLE_B_LEN: AtomicUsize = AtomicUsize::new(0);

unsafe extern "C" fn cycle_loader(
    _s: *mut ElaraState,
    specifier: *const core::ffi::c_char,
    out_bytes: *mut *const u8,
    out_len: *mut usize,
    out_is_bc: *mut bool,
    _ud: *mut core::ffi::c_void,
) -> ElaraStatus {
    let name = core::ffi::CStr::from_ptr(specifier).to_bytes();
    *out_is_bc = true;
    match name {
        b"a" => {
            *out_bytes = CYCLE_A_PTR.load(Ordering::SeqCst) as *const u8;
            *out_len = CYCLE_A_LEN.load(Ordering::SeqCst);
            ElaraStatus::Ok
        }
        b"b" => {
            *out_bytes = CYCLE_B_PTR.load(Ordering::SeqCst) as *const u8;
            *out_len = CYCLE_B_LEN.load(Ordering::SeqCst);
            ElaraStatus::Ok
        }
        _ => ElaraStatus::ErrModule,
    }
}

#[test]
fn circular_import_cycle_a_to_b_to_a_fails_with_err_module() {
    let mut state = ElaraState::new();
    unsafe {
        let a = build_import_only_module(&mut state, "b");
        let b = build_import_only_module(&mut state, "a");
        let a_bytes = leak_bytes(serialize_elac(a, 0).expect("a"));
        let b_bytes = leak_bytes(serialize_elac(b, 0).expect("b"));
        CYCLE_A_PTR.store(a_bytes.as_ptr() as usize, Ordering::SeqCst);
        CYCLE_A_LEN.store(a_bytes.len(), Ordering::SeqCst);
        CYCLE_B_PTR.store(b_bytes.as_ptr() as usize, Ordering::SeqCst);
        CYCLE_B_LEN.store(b_bytes.len(), Ordering::SeqCst);

        elara_set_module_loader(&mut state, Some(cycle_loader), core::ptr::null_mut());
        let main = build_main_import(&mut state, &[("a", None)], None);
        let (st, _) = run_proto(&mut state, main);
        assert_eq!(st, ElaraStatus::ErrModule);
        let err = core::ffi::CStr::from_ptr(elara_last_error(&state))
            .to_str()
            .unwrap();
        assert_eq!(err, "Circular module dependency detected: 'a'");
    }
}

static HOST_PTR: AtomicUsize = AtomicUsize::new(0);
static HOST_LEN: AtomicUsize = AtomicUsize::new(0);

unsafe extern "C" fn host_loader(
    _s: *mut ElaraState,
    specifier: *const core::ffi::c_char,
    out_bytes: *mut *const u8,
    out_len: *mut usize,
    out_is_bc: *mut bool,
    _ud: *mut core::ffi::c_void,
) -> ElaraStatus {
    let name = core::ffi::CStr::from_ptr(specifier).to_bytes();
    if name != b"mem://hello" {
        return ElaraStatus::ErrModule;
    }
    *out_bytes = HOST_PTR.load(Ordering::SeqCst) as *const u8;
    *out_len = HOST_LEN.load(Ordering::SeqCst);
    *out_is_bc = true;
    ElaraStatus::Ok
}

#[test]
fn custom_host_module_loader_resolves_in_memory_modules_in_sandbox() {
    let mut state = ElaraState::new();
    // Cap down to safe default (still allows loader; no FS needed).
    state.capabilities = elara::ffi::ELARA_CAP_SAFE_DEFAULT;
    unsafe {
        let mod_proto = build_answer_module(&mut state);
        let bytes = leak_bytes(serialize_elac(mod_proto, 0).expect("ser"));
        HOST_PTR.store(bytes.as_ptr() as usize, Ordering::SeqCst);
        HOST_LEN.store(bytes.len(), Ordering::SeqCst);
        elara_set_module_loader(
            &mut state,
            Some(host_loader as ElaraModuleLoaderFn),
            core::ptr::null_mut(),
        );

        let main = build_main_import(&mut state, &[("mem://hello", Some("h"))], Some(0));
        let (st, v) = run_proto(&mut state, main);
        assert_eq!(st, ElaraStatus::Ok);
        assert_eq!(v.to_i64(), Some(42));
    }
}
