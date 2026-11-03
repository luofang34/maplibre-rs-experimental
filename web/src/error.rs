//! Browser map initialization and worker errors.

use std::borrow::Cow;

use js_sys::TypeError;
use maplibre::io::apc::{CallError, ProcedureError};
use thiserror::Error;
use wasm_bindgen::{JsCast, JsValue};

#[derive(Error, Debug)]
pub enum WebError {
    #[error("JS error type is unknown")]
    UnknownErrorType,
    /// Returned if the message is not valid, e.g. if it it is not valid UTF-8.
    #[error("message string in error is invalid")]
    InvalidMessage,
    /// TypeError like it is defined in JS
    #[error("JavaScript type error: {0}")]
    TypeError(Cow<'static, str>),
    #[error("fetching data failed: {0}")]
    FetchError(Cow<'static, str>),
    /// The server has no resource at the URL.
    #[error("no resource at {0}")]
    NotFound(String),
    /// Any other Error
    #[error("JavaScript error: {0}")]
    GenericError(Cow<'static, str>),
    /// The kernel configuration sent to the tile workers could not be encoded.
    #[error("encoding the tile workers' kernel configuration failed")]
    KernelConfigEncoding(#[source] serde_json::Error),
    /// A tile worker received a kernel configuration it cannot decode.
    #[error("decoding the tile worker's kernel configuration failed")]
    KernelConfigDecoding(#[source] serde_json::Error),
    /// A tile worker received a second kernel configuration.
    #[error("the tile worker already has a kernel configuration")]
    KernelConfigAlreadySet,
    /// A worker message arrived while the map was reading earlier ones.
    #[error("worker results are being received; the message cannot be stored")]
    ReceivedBusy,
}

impl From<JsValue> for WebError {
    fn from(value: JsValue) -> Self {
        if let Some(error) = value.dyn_ref::<TypeError>() {
            let Some(message) = error.message().as_string() else {
                return WebError::InvalidMessage;
            };

            WebError::TypeError(message.into())
        } else if let Some(error) = value.dyn_ref::<js_sys::Error>() {
            let Some(message) = error.message().as_string() else {
                return WebError::InvalidMessage;
            };

            WebError::GenericError(message.into())
        } else {
            WebError::UnknownErrorType
        }
    }
}

/// Errors returned to JavaScript by map startup and worker execution.
#[derive(Error, Debug)]
pub enum JSError {
    #[error("map kernel configuration failed")]
    Kernel(#[from] maplibre::kernel::KernelBuildError),
    #[error(transparent)]
    Procedure(#[from] ProcedureError),
    #[error(transparent)]
    Call(#[from] CallError),
    #[error(transparent)]
    Web(#[from] WebError),
    #[error("invalid map style: {0}")]
    InvalidStyle(#[source] serde_json::Error),
    #[error(transparent)]
    Map(#[from] maplibre::map::MapError),
    #[error(transparent)]
    Host(#[from] maplibre_winit::WinitHostError),
    #[error("event loop closed before map initialization completed")]
    ClosedBeforeReady,
}

impl From<JSError> for JsValue {
    fn from(val: JSError) -> Self {
        JsValue::from_str(&val.to_string())
    }
}
