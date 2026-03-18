//! Compile-time constant folding for unary and binary operators.

use core::cmp::Ordering;

use crate::compiler::expr::{BinOp, UnOp};
use crate::value::Value;

/// Folds unary `op v` when both are compile-time constants; `None` leaves evaluation to runtime.
#[must_use]
pub fn fold_unop_value(op: UnOp, v: Value) -> Option<Value> {
    match op {
        UnOp::Neg => {
            if let Some(i) = v.to_i64() {
                int_value(i.wrapping_neg())
            } else if v.is_float() {
                Some(Value::from_f64(-v.as_f64()))
            } else {
                None
            }
        }
        UnOp::BNot => v.to_i64().and_then(|i| int_value(!i)),
        UnOp::Not => None,
    }
}

/// Folds binary `lv op rv` when both are compile-time constants.
/// Division/modulo by zero returns `None` (left to runtime).
#[must_use]
pub fn fold_binop_value(op: BinOp, lv: Value, rv: Value) -> Option<Value> {
    if matches!(op, BinOp::Div | BinOp::Mod) && is_numeric_zero(rv) {
        return None;
    }

    match op {
        BinOp::Add => fold_add(lv, rv),
        BinOp::Sub => fold_sub(lv, rv),
        BinOp::Mul => fold_mul(lv, rv),
        BinOp::Div => fold_div(lv, rv),
        BinOp::Mod => fold_mod(lv, rv),
        BinOp::Pow => fold_pow(lv, rv),
        BinOp::Band => fold_bitwise(lv, rv, |a, b| a & b),
        BinOp::Bor => fold_bitwise(lv, rv, |a, b| a | b),
        BinOp::Bxor => fold_bitwise(lv, rv, |a, b| a ^ b),
        BinOp::Shl => fold_bitwise(lv, rv, |a, b| a.wrapping_shl((b as u32) & 63)),
        BinOp::Shr => fold_bitwise(lv, rv, |a, b| a.wrapping_shr((b as u32) & 63)),
        BinOp::Eq => Some(Value::from_bool(lv.val_equal(rv))),
        BinOp::Ne => Some(Value::from_bool(!lv.val_equal(rv))),
        BinOp::Lt => fold_cmp(lv, rv, |o| o == Ordering::Less),
        BinOp::Le => fold_cmp(lv, rv, |o| matches!(o, Ordering::Less | Ordering::Equal)),
        BinOp::Gt => fold_cmp(lv, rv, |o| o == Ordering::Greater),
        BinOp::Ge => fold_cmp(lv, rv, |o| matches!(o, Ordering::Greater | Ordering::Equal)),
    }
}

fn is_numeric_zero(v: Value) -> bool {
    match v.to_i64() {
        Some(0) => true,
        Some(_) => false,
        None if v.is_float() => v.as_f64() == 0.0,
        None => false,
    }
}

fn fold_add(l: Value, r: Value) -> Option<Value> {
    match (l.to_i64(), r.to_i64()) {
        (Some(a), Some(b)) => int_value(a.wrapping_add(b)),
        _ => match (num_f64(l), num_f64(r)) {
            (Some(a), Some(b)) => Some(Value::from_f64(a + b)),
            _ => None,
        },
    }
}

fn fold_sub(l: Value, r: Value) -> Option<Value> {
    match (l.to_i64(), r.to_i64()) {
        (Some(a), Some(b)) => int_value(a.wrapping_sub(b)),
        _ => match (num_f64(l), num_f64(r)) {
            (Some(a), Some(b)) => Some(Value::from_f64(a - b)),
            _ => None,
        },
    }
}

fn fold_mul(l: Value, r: Value) -> Option<Value> {
    match (l.to_i64(), r.to_i64()) {
        (Some(a), Some(b)) => int_value(a.wrapping_mul(b)),
        _ => match (num_f64(l), num_f64(r)) {
            (Some(a), Some(b)) => Some(Value::from_f64(a * b)),
            _ => None,
        },
    }
}

fn fold_div(l: Value, r: Value) -> Option<Value> {
    let (Some(a), Some(b)) = (num_f64(l), num_f64(r)) else {
        return None;
    };
    if b == 0.0 {
        return None;
    }
    Some(Value::from_f64(a / b))
}

fn fold_mod(l: Value, r: Value) -> Option<Value> {
    match (l.to_i64(), r.to_i64()) {
        (Some(a), Some(b)) if b != 0 => int_value(a.wrapping_rem(b)),
        _ => None,
    }
}

fn fold_pow(l: Value, r: Value) -> Option<Value> {
    let (Some(a), Some(b)) = (num_f64(l), num_f64(r)) else {
        return None;
    };
    Some(Value::from_f64(a.powf(b)))
}

fn fold_bitwise(l: Value, r: Value, f: impl FnOnce(i64, i64) -> i64) -> Option<Value> {
    match (l.to_i64(), r.to_i64()) {
        (Some(a), Some(b)) => int_value(f(a, b)),
        _ => None,
    }
}

fn fold_cmp(l: Value, r: Value, pred: impl FnOnce(Ordering) -> bool) -> Option<Value> {
    let ord = match (l.to_i64(), r.to_i64()) {
        (Some(a), Some(b)) => a.cmp(&b),
        _ => match (num_f64(l), num_f64(r)) {
            (Some(a), Some(b)) => a.partial_cmp(&b)?,
            _ => return None,
        },
    };
    Some(Value::from_bool(pred(ord)))
}

fn num_f64(v: Value) -> Option<f64> {
    if let Some(i) = v.to_i64() {
        Some(i as f64)
    } else if v.is_float() {
        Some(v.as_f64())
    } else {
        None
    }
}

fn int_value(i: i64) -> Option<Value> {
    if Value::fits_i48(i) {
        Some(Value::from_i48(i))
    } else {
        None
    }
}
