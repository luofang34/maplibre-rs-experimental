use crate::platform::singlethreaded::apc::wire::PassingContext;

pub mod apc;
pub mod image_provider;
pub mod transferables;
pub mod wasm_entries;

pub type UsedContext = PassingContext;
