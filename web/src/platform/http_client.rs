use async_trait::async_trait;
use js_sys::{ArrayBuffer, Uint8Array};
use maplibre::io::source_client::{ByteRange, HttpClient, SourceFetchError};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use web_sys::{Request, RequestInit, Response, WorkerGlobalScope};

use crate::error::WebError;

#[derive(Clone, Default)]
pub struct WHATWGFetchHttpClient;

fn invalid_response(message: &'static str) -> SourceFetchError {
    SourceFetchError(Box::new(WebError::TypeError(message.into())))
}

impl WHATWGFetchHttpClient {
    /// The body and whether it is only the range asked for.
    async fn fetch_array_buffer(
        url: &str,
        range: Option<ByteRange>,
    ) -> Result<(JsValue, bool), SourceFetchError> {
        let opts = RequestInit::new();
        opts.set_method("GET");
        let request = Request::new_with_str_and_init(url, &opts)
            .map_err(|error| SourceFetchError(Box::new(WebError::from(error))))?;
        if let Some(range) = range {
            request
                .headers()
                .set("Range", &range.header())
                .map_err(|error| SourceFetchError(Box::new(WebError::from(error))))?;
        }

        // Tile workers and the main-thread TileJSON loader use the same transport.
        let global = js_sys::global();
        let promise = match global.dyn_into::<WorkerGlobalScope>() {
            Ok(scope) => scope.fetch_with_request(&request),
            Err(_) => web_sys::window()
                .ok_or_else(|| invalid_response("No worker or window scope to fetch from"))?
                .fetch_with_request(&request),
        };
        let response: Response = JsFuture::from(promise)
            .await
            .map_err(|error| SourceFetchError::temporary(WebError::from(error)))?
            .dyn_into()
            .map_err(|_| invalid_response("Unable to cast to Response"))?;
        if response.status() == 404 {
            return Err(SourceFetchError::not_found(url));
        }
        if !response.ok() {
            return Err(SourceFetchError::http_response(
                url,
                response.status(),
                WebError::FetchError(response.status_text().into()),
            ));
        }
        let partial = response.status() == 206;
        let buffer = response
            .array_buffer()
            .map_err(|error| SourceFetchError(Box::new(WebError::from(error))))?;
        let body = JsFuture::from(buffer)
            .await
            .map_err(|error| SourceFetchError::temporary(WebError::from(error)))?;
        Ok((body, partial))
    }

    async fn get(url: &str, range: Option<ByteRange>) -> Result<Vec<u8>, SourceFetchError> {
        let (body, partial) = Self::fetch_array_buffer(url, range).await?;
        let array_buffer: ArrayBuffer = body
            .dyn_into()
            .map_err(|_| invalid_response("Unable to cast to ArrayBuffer"))?;
        let body = Uint8Array::new(&array_buffer).to_vec();
        // A server that ignores the range answers with the whole body.
        match range {
            Some(range) if !partial => range.slice(url, &body).map(<[u8]>::to_vec),
            _ => Ok(body),
        }
    }
}

#[async_trait(?Send)]
impl HttpClient for WHATWGFetchHttpClient {
    async fn fetch(&self, url: &str) -> Result<Vec<u8>, SourceFetchError> {
        Self::get(url, None).await
    }

    async fn fetch_range(&self, url: &str, range: ByteRange) -> Result<Vec<u8>, SourceFetchError> {
        Self::get(url, Some(range)).await
    }
}
