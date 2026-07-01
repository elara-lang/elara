//! Opcode histogram, per-function counters, and DAP stackTrace/variables JSON.

#![allow(unsafe_op_in_unsafe_fn)]
#![allow(clippy::missing_safety_doc)]

use crate::compiler::ObjProto;
use crate::compiler::opcode::Instruction;
use crate::value::{ObjType, Value};
use crate::vm::debug::hooks::{frame_at, proto_of};
use crate::vm::error::frame_pc_index;
use crate::vm::frame::CallFrame;
use crate::vm::state::ElaraState;

/// Call / inclusive-instruction counters for one function prototype.
#[derive(Clone, Copy, Debug)]
pub struct FnCounters {
    pub proto: *const ObjProto,
    pub calls: u64,
    pub inclusive_insns: u64,
}

/// Enables profiling and zeroes existing counters.
pub fn enable_profiler(state: &mut ElaraState) {
    reset_profiler(state);
    state.debug.profiling = true;
}

/// Clears histogram and function counters; leaves `profiling` flag unchanged.
pub fn reset_profiler(state: &mut ElaraState) {
    state.debug.opcode_counts = [0; 64];
    state.debug.fn_counters.clear();
}

fn bump_fn(state: &mut ElaraState, proto: *const ObjProto, calls: u64, insns: u64) {
    if proto.is_null() {
        return;
    }
    if let Some(slot) = state
        .debug
        .fn_counters
        .iter_mut()
        .find(|c| c.proto == proto)
    {
        slot.calls = slot.calls.saturating_add(calls);
        slot.inclusive_insns = slot.inclusive_insns.saturating_add(insns);
    } else {
        state.debug.fn_counters.push(FnCounters {
            proto,
            calls,
            inclusive_insns: insns,
        });
    }
}

/// Records one executed instruction into the opcode histogram and inclusive counters.
#[inline]
pub unsafe fn record_instruction(state: &mut ElaraState, frame: &CallFrame, inst: Instruction) {
    let op = inst.opcode_raw() as usize;
    if op < 64 {
        state.debug.opcode_counts[op] = state.debug.opcode_counts[op].saturating_add(1);
    }
    if let Some(p) = proto_of(frame) {
        bump_fn(state, p as *const ObjProto, 0, 1);
    }
}

/// Records a function call against the callee frame's prototype.
#[inline]
pub unsafe fn record_call(state: &mut ElaraState, frame: &CallFrame) {
    if let Some(p) = proto_of(frame) {
        bump_fn(state, p as *const ObjProto, 1, 0);
    }
}

fn json_escape(s: &str, out: &mut String) {
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                let _ = core::fmt::Write::write_fmt(out, format_args!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
}

fn push_json_str(out: &mut String, s: &str) {
    out.push('"');
    json_escape(s, out);
    out.push('"');
}

unsafe fn source_name_of(p: &ObjProto) -> &str {
    if p.source_name.is_null() {
        "<chunk>"
    } else {
        (*p.source_name).as_str()
    }
}

fn format_value(v: Value) -> (String, &'static str) {
    if v.is_nil() {
        ("nil".into(), "nil")
    } else if v.is_bool() {
        (if v.as_bool() { "true" } else { "false" }.into(), "bool")
    } else if let Some(i) = v.to_i64() {
        (i.to_string(), "int")
    } else if v.is_float() {
        let f = v.as_f64();
        (format!("{f}"), "float")
    } else if v.is_obj_type(ObjType::String) {
        let s = unsafe { (*v.as_obj().cast::<crate::value::ObjString>()).as_str() };
        let mut quoted = String::from("\"");
        json_escape(s, &mut quoted);
        quoted.push('"');
        (quoted, "string")
    } else if v.is_obj_type(ObjType::Table) {
        ("<table>".into(), "table")
    } else if v.is_obj_type(ObjType::Closure) {
        ("<closure>".into(), "closure")
    } else {
        ("<value>".into(), "object")
    }
}

/// Serializes the active fiber's call stack and register scopes as DAP-shaped JSON.
///
/// Shape: `{ "stackFrames": [...], "totalFrames": N, "variables": [ { "frameId", "variables": [...] } ] }`.
pub unsafe fn format_dap_stack_trace(state: &ElaraState) -> String {
    let mut out = String::with_capacity(512);
    out.push_str("{\"stackFrames\":[");

    let fiber = state.current_fiber;
    let frame_count = if fiber.is_null() {
        0u32
    } else {
        (*fiber).frame_count
    };

    let mut vars_json = String::new();
    vars_json.push_str("\"variables\":[");

    for depth in 0..frame_count {
        if depth > 0 {
            out.push(',');
            vars_json.push(',');
        }
        let fp = match frame_at(fiber, depth) {
            Some(p) => p,
            None => break,
        };
        let frame = &*fp;
        let (name, path, line, num_locals) = if let Some(p) = proto_of(frame) {
            let pc = frame_pc_index(frame).saturating_sub(1);
            (
                "<script>".to_string(),
                source_name_of(p).to_string(),
                p.get_line(pc),
                u16::from(p.max_stack_size),
            )
        } else {
            ("<native>".to_string(), String::new(), 0u32, 0u16)
        };

        out.push('{');
        out.push_str("\"id\":");
        out.push_str(&depth.to_string());
        out.push_str(",\"name\":");
        push_json_str(&mut out, &name);
        out.push_str(",\"line\":");
        out.push_str(&line.to_string());
        out.push_str(",\"column\":1");
        if !path.is_empty() {
            out.push_str(",\"source\":{\"path\":");
            push_json_str(&mut out, &path);
            out.push('}');
        }
        out.push('}');

        // Variables for this frame.
        vars_json.push_str("{\"frameId\":");
        vars_json.push_str(&depth.to_string());
        vars_json.push_str(",\"variables\":[");
        if !fiber.is_null() && num_locals > 0 {
            let base = (*fiber).stack.add(frame.base_idx as usize);
            for r in 0..num_locals {
                if r > 0 {
                    vars_json.push(',');
                }
                let v = *base.add(r as usize);
                let (val_s, ty) = format_value(v);
                vars_json.push('{');
                vars_json.push_str("\"name\":");
                push_json_str(&mut vars_json, &format!("R{r}"));
                vars_json.push_str(",\"value\":");
                push_json_str(&mut vars_json, &val_s);
                vars_json.push_str(",\"type\":");
                push_json_str(&mut vars_json, ty);
                vars_json.push_str(",\"variablesReference\":0}");
            }
        }
        vars_json.push_str("]}");
    }

    out.push_str("],\"totalFrames\":");
    out.push_str(&frame_count.to_string());
    out.push(',');
    vars_json.push(']');
    out.push_str(&vars_json);
    out.push('}');
    out
}
