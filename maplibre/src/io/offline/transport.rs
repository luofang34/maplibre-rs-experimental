//! Cache-first [`OfflineHttpClient`] decorator backed by an [`OfflineStore`].

use std::sync::Arc;

use async_trait::async_trait;

use super::store::OfflineStore;
use crate::io::source_client::{
    is_remote_http_url, ByteRange, HttpClient, HttpRequest, NetworkStatus, ResourceKind,
    SourceFetchError,
};

/// Cache-first [`HttpClient`] decorator that serves stored offline resources even when
/// [`NetworkStatus::Offline`] and optionally records online responses into the ambient offline cache.
pub struct OfflineHttpClient<HC: HttpClient, S: OfflineStore> {
    inner: HC,
    store: Arc<S>,
    ambient_caching: bool,
}

impl<HC: HttpClient, S: OfflineStore> Clone for OfflineHttpClient<HC, S> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            store: Arc::clone(&self.store),
            ambient_caching: self.ambient_caching,
        }
    }
}

impl<HC: HttpClient, S: OfflineStore> OfflineHttpClient<HC, S> {
    /// Wraps `inner` with cache-first lookups against `store` and ambient caching enabled.
    pub fn new(inner: HC, store: Arc<S>) -> Self {
        Self {
            inner,
            store,
            ambient_caching: true,
        }
    }

    /// Configures whether online `200 OK` responses are opportunistically written to ambient storage.
    pub fn with_ambient_caching(mut self, enabled: bool) -> Self {
        self.ambient_caching = enabled;
        self
    }

    /// Borrows the wrapped inner [`HttpClient`].
    pub fn inner(&self) -> &HC {
        &self.inner
    }

    /// Borrows the backing [`OfflineStore`].
    pub fn store(&self) -> &Arc<S> {
        &self.store
    }
}

#[cfg_attr(not(feature = "thread-safe-futures"), async_trait(?Send))]
#[cfg_attr(feature = "thread-safe-futures", async_trait)]
impl<HC: HttpClient, S: OfflineStore> HttpClient for OfflineHttpClient<HC, S> {
    async fn fetch(&self, url: &str) -> Result<Vec<u8>, SourceFetchError> {
        let req = HttpRequest::new(url, ResourceKind::infer_from_url(url));
        self.fetch_request_with_status(&req, NetworkStatus::Online)
            .await
    }

    async fn fetch_range(&self, url: &str, range: ByteRange) -> Result<Vec<u8>, SourceFetchError> {
        let req = HttpRequest::new(url, ResourceKind::infer_from_url(url)).with_range(range);
        self.fetch_request_with_status(&req, NetworkStatus::Online)
            .await
    }

    async fn fetch_request(&self, req: &HttpRequest) -> Result<Vec<u8>, SourceFetchError> {
        self.fetch_request_with_status(req, NetworkStatus::Online)
            .await
    }

    async fn fetch_request_with_status(
        &self,
        req: &HttpRequest,
        status: NetworkStatus,
    ) -> Result<Vec<u8>, SourceFetchError> {
        if !req.cache_policy.is_volatile() {
            if let Ok(Some(cached)) = self.store.get_resource(&req.url).await {
                return match req.range {
                    Some(range) => range.slice(&req.url, &cached).map(<[u8]>::to_vec),
                    None => Ok(cached),
                };
            }
        }
        let body = self.inner.fetch_request_with_status(req, status).await?;
        if self.ambient_caching
            && !req.cache_policy.is_volatile()
            && req.range.is_none()
            && is_remote_http_url(&req.url)
        {
            let _ = self
                .store
                .put_ambient_resource(&req.url, req.resource_kind, &body)
                .await;
        }
        Ok(body)
    }
}
