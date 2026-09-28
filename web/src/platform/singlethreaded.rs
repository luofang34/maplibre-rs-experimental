use crate::platform::{http_client::WHATWGFetchHttpClient, singlethreaded::apc::PassingContext};

pub mod apc;
pub mod transferables;
pub mod wasm_entries;

pub type UsedHttpClient = WHATWGFetchHttpClient;
pub type UsedContext = PassingContext;
