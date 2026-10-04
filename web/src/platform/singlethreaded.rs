use crate::platform::{http_client::WebHttpClient, singlethreaded::apc::PassingContext};

pub mod apc;
pub mod image_provider;
pub mod transferables;
pub mod wasm_entries;

pub type UsedHttpClient = WebHttpClient;
pub type UsedContext = PassingContext;
