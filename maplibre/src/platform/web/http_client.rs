//! The browser `fetch` transport, usable from a page or a dedicated worker.

use std::borrow::Cow;

use async_trait::async_trait;
use js_sys::{ArrayBuffer, Uint8Array};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{Request, RequestInit, Response, WorkerGlobalScope};

use crate::io::{
    pmtiles::PmtilesClient,
    source_client::{ByteRange, HttpClient, SourceFetchError},
};

/// A browser request failed before a usable body arrived.
#[derive(thiserror::Error, Debug)]
pub enum FetchError {
    /// JavaScript threw or rejected; a network or CORS failure surfaces as a `TypeError` here.
    #[error("JavaScript error: {0}")]
    Js(Cow<'static, str>),
    /// `fetch` rejected without a response. Browsers report a refused connection, an offline
    /// page and a response the server's CORS headers withhold from this origin alike.
    #[error(
        "no response from {url} ({reason}): the network failed or the server does not allow \
         this origin (CORS)"
    )]
    Network {
        /// The requested resource.
        url: String,
        /// What JavaScript reported.
        reason: Cow<'static, str>,
    },
    /// The value JavaScript returned is not the type `fetch` promises.
    #[error("unexpected fetch value: {0}")]
    InvalidResponse(&'static str),
    /// The server answered with a status other than success or not found.
    #[error("HTTP {status}: {status_text}")]
    Status {
        /// The response's status code.
        status: u16,
        /// The response's status text.
        status_text: String,
    },
}

/// What a thrown or rejected JavaScript value says about itself.
fn js_message(value: &JsValue) -> Cow<'static, str> {
    value
        .dyn_ref::<js_sys::Error>()
        .and_then(|error| error.message().as_string())
        .or_else(|| value.as_string())
        .map_or(Cow::Borrowed("unknown JavaScript value"), Cow::Owned)
}

impl From<JsValue> for FetchError {
    fn from(value: JsValue) -> Self {
        Self::Js(js_message(&value))
    }
}

/// Fetches with the global `fetch` of the window or the worker it runs in.
#[derive(Clone, Default)]
pub struct WHATWGFetchHttpClient;

/// The browser transport, also reading tiles out of PMTiles archives with range requests.
pub type WebHttpClient = PmtilesClient<WHATWGFetchHttpClient>;

/// A browser client with an empty archive cache.
pub fn web_http_client() -> WebHttpClient {
    PmtilesClient::new(WHATWGFetchHttpClient)
}

fn rejected(error: JsValue) -> SourceFetchError {
    SourceFetchError(Box::new(FetchError::from(error)))
}

fn invalid_response(message: &'static str) -> SourceFetchError {
    SourceFetchError(Box::new(FetchError::InvalidResponse(message)))
}

impl WHATWGFetchHttpClient {
    /// The body and whether it is only the range asked for.
    async fn fetch_array_buffer(
        url: &str,
        range: Option<ByteRange>,
    ) -> Result<(JsValue, bool), SourceFetchError> {
        let opts = RequestInit::new();
        opts.set_method("GET");
        let request = Request::new_with_str_and_init(url, &opts).map_err(rejected)?;
        if let Some(range) = range {
            request
                .headers()
                .set("Range", &range.header())
                .map_err(rejected)?;
        }

        // Tile workers and the main-thread TileJSON loader use the same transport.
        let global = js_sys::global();
        let promise = match global.dyn_into::<WorkerGlobalScope>() {
            Ok(scope) => scope.fetch_with_request(&request),
            Err(_) => web_sys::window()
                .ok_or_else(|| invalid_response("no worker or window scope to fetch from"))?
                .fetch_with_request(&request),
        };
        let response: Response = JsFuture::from(promise)
            .await
            .map_err(|error| {
                // Retried like any transient failure; a CORS refusal costs a backed-off retry.
                SourceFetchError::temporary(FetchError::Network {
                    url: url.to_owned(),
                    reason: js_message(&error),
                })
            })?
            .dyn_into()
            .map_err(|_| invalid_response("not a Response"))?;
        if response.status() == 404 {
            return Err(SourceFetchError::not_found(url));
        }
        if !response.ok() {
            return Err(SourceFetchError::http_response(
                url,
                response.status(),
                FetchError::Status {
                    status: response.status(),
                    status_text: response.status_text(),
                },
            ));
        }
        let partial = response.status() == 206;
        let buffer = response.array_buffer().map_err(rejected)?;
        let body = JsFuture::from(buffer)
            .await
            .map_err(|error| SourceFetchError::temporary(FetchError::from(error)))?;
        Ok((body, partial))
    }

    async fn get(url: &str, range: Option<ByteRange>) -> Result<Vec<u8>, SourceFetchError> {
        let (body, partial) = Self::fetch_array_buffer(url, range).await?;
        let array_buffer: ArrayBuffer = body
            .dyn_into()
            .map_err(|_| invalid_response("not an ArrayBuffer"))?;
        let body = Uint8Array::new(&array_buffer).to_vec();
        // A server that ignores the range answers with the whole body.
        match range {
            Some(range) if !partial => range.slice(url, &body).map(<[u8]>::to_vec),
            _ => Ok(body),
        }
    }
}

#[cfg_attr(not(feature = "thread-safe-futures"), async_trait(?Send))]
#[cfg_attr(feature = "thread-safe-futures", async_trait)]
impl HttpClient for WHATWGFetchHttpClient {
    async fn fetch(&self, url: &str) -> Result<Vec<u8>, SourceFetchError> {
        Self::get(url, None).await
    }

    async fn fetch_range(&self, url: &str, range: ByteRange) -> Result<Vec<u8>, SourceFetchError> {
        Self::get(url, Some(range)).await
    }
}
