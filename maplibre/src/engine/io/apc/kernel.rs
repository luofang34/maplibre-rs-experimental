//! Worker-side kernel trait and serializable worker startup configuration.

use serde::{Deserialize, Serialize};

use crate::io::{
    resource_loader::SharedLoader,
    source_client::{AssetCache, HttpClient, ImageProviders, SourceClient},
};

/// Configuration passed across worker boundaries to construct source services.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct OffscreenKernelConfig {
    /// Filesystem cache directory for hosts that support persistent HTTP caching.
    /// `None` disables that cache; browser-only kernels may ignore the setting.
    pub cache_directory: Option<String>,
    /// Decoded glyphs and sprites shared by every call made from clones of this configuration.
    /// It is not serialized: a worker that receives the configuration over a channel starts
    /// with its own empty cache.
    #[serde(skip)]
    pub asset_cache: AssetCache,
    /// The providers of images labels name and no sprite supplies, shared like the asset
    /// cache. A worker that receives its configuration over a channel registers its own.
    #[serde(skip)]
    pub image_providers: ImageProviders,
    /// The loader the map thread uses, so in-process workers fetch through the same one.
    /// Like the asset cache it is not serialized: a worker in another process or a browser
    /// worker builds its platform's own loader.
    #[serde(skip)]
    pub loader: Option<SharedLoader>,
}

impl OffscreenKernelConfig {
    /// Decodes serialized worker config bytes without instantiating `serde::Deserialize` glue.
    pub fn from_json_slice(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        let map = parse_json_object(bytes, "config")?;
        Ok(Self {
            cache_directory: parse_cache_directory(&map)?,
            ..Self::default()
        })
    }
}

pub(super) fn parse_json_object(
    bytes: &[u8],
    context_field: &'static str,
) -> Result<serde_json::Map<String, serde_json::Value>, serde_json::Error> {
    let value: serde_json::Value = serde_json::from_slice(bytes)?;
    value
        .as_object()
        .cloned()
        .ok_or_else(|| crate::util::json::extract::field_err(context_field, "expected object"))
}

fn parse_cache_directory(
    map: &serde_json::Map<String, serde_json::Value>,
) -> Result<Option<String>, serde_json::Error> {
    crate::util::json::extract::json_opt_string(map, "cache_directory")
}

/// Worker-side services, independent of window and GPU ownership.
pub trait OffscreenKernel: Send + Sync + 'static {
    /// HTTP implementation used by this environment's source loader.
    type HttpClient: HttpClient;
    /// Constructs worker services from the configuration supplied at worker startup.
    fn create(config: OffscreenKernelConfig) -> Self;

    /// Creates a source loader using this worker's HTTP and cache configuration.
    fn source_client(&self) -> SourceClient<Self::HttpClient>;
}
