//! 0-based `CallContext` for native function argument and return exchange.

#![allow(unsafe_op_in_unsafe_fn)]
#![allow(clippy::missing_safety_doc)]

use crate::value::Value;

/// Call frame for a host native function: explicit 0-based args and returns.
#[repr(C)]
pub struct CallContext {
    pub argc: u16,
    pub max_results: u16,
    pub nresults: u16,
    pub reserved: u16,
    pub args: *const Value,
    pub results: *mut Value,
    pub userdata: *mut core::ffi::c_void,
}

impl CallContext {
    /// Number of arguments passed to the native.
    #[inline]
    #[must_use]
    pub const fn argc(&self) -> u16 {
        self.argc
    }

    /// Returns argument at 0-based `index`, or `Value::NIL` if out of range.
    #[inline]
    #[must_use]
    pub unsafe fn arg(&self, index: u16) -> Value {
        if index >= self.argc || self.args.is_null() {
            return Value::NIL;
        }
        unsafe { *self.args.add(index as usize) }
    }

    /// Writes a single return value at slot 0 (clears further result slots up to `max_results`).
    #[inline]
    pub unsafe fn return_one(&mut self, val: Value) {
        if self.results.is_null() || self.max_results == 0 {
            self.nresults = 0;
            return;
        }
        unsafe {
            *self.results = val;
            for i in 1..self.max_results as usize {
                *self.results.add(i) = Value::NIL;
            }
        }
        self.nresults = 1;
    }

    /// Writes `count` return values (truncated to `max_results`).
    #[inline]
    pub unsafe fn return_multi(&mut self, vals: *const Value, count: u16) {
        if self.results.is_null() || self.max_results == 0 || vals.is_null() {
            self.nresults = 0;
            return;
        }
        let n = count.min(self.max_results) as usize;
        unsafe {
            for i in 0..n {
                *self.results.add(i) = *vals.add(i);
            }
            for i in n..self.max_results as usize {
                *self.results.add(i) = Value::NIL;
            }
        }
        self.nresults = n as u16;
    }

    /// Opaque userdata pointer associated with the `ObjNativeFn`.
    #[inline]
    #[must_use]
    pub const fn context_userdata(&self) -> *mut core::ffi::c_void {
        self.userdata
    }
}
