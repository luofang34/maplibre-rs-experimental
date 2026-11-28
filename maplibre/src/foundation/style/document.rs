//! Style document parsing, defaults, attribution, diffs, mutations, state templates,
//! transitions, and validation.

pub(crate) mod attribution;
pub(crate) mod default_style;
pub mod diff;
pub(crate) mod from_value;
pub mod mutation;
pub mod state;
#[cfg(test)]
mod tests;
pub mod transitions;
pub mod validation;
