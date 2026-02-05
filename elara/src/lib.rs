//! Elara embeddable scripting language core library.

pub mod lexer;

/// Returns the canonical Elara runtime version string.
#[must_use]
pub const fn version() -> &'static str {
    "Elara 1.0.0"
}
