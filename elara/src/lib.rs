//! Elara embeddable scripting language core library.

pub mod compiler;
pub mod gc;
pub mod lexer;
pub mod value;

/// Returns the canonical Elara runtime version string.
#[must_use]
pub const fn version() -> &'static str {
    "Elara 1.0.0"
}
