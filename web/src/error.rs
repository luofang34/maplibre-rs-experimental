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
    EventLoop(#[from] maplibre::event_loop::EventLoopError),
    #[error("map window has no event loop")]
    MissingEventLoop,
}

impl From<JSError> for JsValue {
    fn from(val: JSError) -> Self {
        JsValue::from_str(&val.to_string())
    }
}
