//! Outgoing HTTP request metadata, cache policies, and embedder transform hooks.

use std::sync::{Arc, PoisonError, RwLock};

use super::{ByteRange, SourceFetchError};

/// Caching policy attached to an [`HttpRequest`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum CachePolicy {
    /// Standard caching across in-memory and persistent disk or browser storage.
    #[default]
    Normal,
    /// Volatile source policy: bypasses persistent disk and browser `CacheStorage` across sessions.
    Volatile,
}

impl CachePolicy {
    /// Returns [`Self::Volatile`] when `volatile` is `true`, otherwise [`Self::Normal`].
    pub const fn from_volatile(volatile: bool) -> Self {
        if volatile {
            Self::Volatile
        } else {
            Self::Normal
        }
    }

    /// Whether persistent cross-session caching must be skipped for this request.
    pub const fn is_volatile(self) -> bool {
        matches!(self, Self::Volatile)
    }
}

/// Classification of the map resource being requested, exposed to [`RequestTransform`] hooks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ResourceKind {
    /// Root MapLibre style JSON document.
    Style,
    /// Source metadata such as TileJSON or an external GeoJSON document.
    Source,
    /// Vector, raster, or elevation tile payload.
    Tile,
    /// Signed-distance-field font glyph range (`{fontstack}/{range}.pbf`).
    Glyphs,
    /// Sprite atlas PNG image.
    SpriteImage,
    /// Sprite atlas JSON index document.
    SpriteJson,
    /// Standalone image source picture.
    Image,
    /// Unclassified URL request.
    #[default]
    Unknown,
}

impl ResourceKind {
    /// Best-effort classification from URL path conventions when a caller supplies only a URL.
    pub fn infer_from_url(url: &str) -> Self {
        let path = url.split('?').next().unwrap_or(url);
        let lower = path.to_ascii_lowercase();
        if lower.ends_with("sprite.json") || lower.ends_with("sprite@2x.json") {
            Self::SpriteJson
        } else if lower.ends_with("sprite.png") || lower.ends_with("sprite@2x.png") {
            Self::SpriteImage
        } else if lower.ends_with(".pbf") && looks_like_glyph_range(&lower) {
            Self::Glyphs
        } else if lower.ends_with("style.json") {
            Self::Style
        } else if lower.ends_with("tiles.json")
            || lower.ends_with("tilejson.json")
            || lower.ends_with(".geojson")
        {
            Self::Source
        } else if lower.ends_with(".pbf") || lower.ends_with(".mvt") || lower.ends_with(".mlt") {
            Self::Tile
        } else {
            Self::Unknown
        }
    }
}

fn looks_like_glyph_range(lower_path: &str) -> bool {
    let Some(stem) = lower_path.strip_suffix(".pbf") else {
        return false;
    };
    let file = stem.rsplit('/').next().unwrap_or(stem);
    let Some((start, end)) = file.split_once('-') else {
        return false;
    };
    !start.is_empty()
        && !end.is_empty()
        && start.bytes().all(|b| b.is_ascii_digit())
        && end.bytes().all(|b| b.is_ascii_digit())
}

/// Outgoing HTTP/resource request descriptor passed to [`RequestTransform`] and [`super::HttpClient`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpRequest {
    /// Target URL (before or after [`RequestTransform`] rewriting).
    pub url: String,
    /// Optional byte range for partial fetches (e.g. PMTiles headers and directories).
    pub range: Option<ByteRange>,
    /// Custom HTTP request headers `(name, value)` forwarded by native and web transports.
    pub headers: Vec<(String, String)>,
    /// Persistent cache policy (`Normal` vs `Volatile`).
    pub cache_policy: CachePolicy,
    /// Kind of map resource this request fetches.
    pub resource_kind: ResourceKind,
}

impl HttpRequest {
    /// Constructs a request for `url` and `resource_kind` with [`CachePolicy::Normal`] and no headers.
    pub fn new(url: impl Into<String>, resource_kind: ResourceKind) -> Self {
        Self {
            url: url.into(),
            range: None,
            headers: Vec::new(),
            cache_policy: CachePolicy::Normal,
            resource_kind,
        }
    }

    /// Attaches a [`ByteRange`] to request a partial response slice.
    pub fn with_range(mut self, range: ByteRange) -> Self {
        self.range = Some(range);
        self
    }

    /// Sets an optional [`ByteRange`] on the request.
    pub fn with_optional_range(mut self, range: Option<ByteRange>) -> Self {
        self.range = range;
        self
    }

    /// Appends a custom HTTP header `(name, value)` to the request.
    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// Sets the [`CachePolicy`] for the request.
    pub fn with_cache_policy(mut self, cache_policy: CachePolicy) -> Self {
        self.cache_policy = cache_policy;
        self
    }
}

/// Thread-safe callback that rewrites URLs or attaches custom HTTP headers prior to transport dispatch.
pub type RequestTransform = Arc<dyn Fn(HttpRequest) -> HttpRequest + Send + Sync>;

#[derive(Clone, Default)]
pub(crate) struct TransformSlot(Arc<RwLock<Option<RequestTransform>>>);

impl TransformSlot {
    pub(crate) fn set(&self, transform: RequestTransform) {
        *self.0.write().unwrap_or_else(PoisonError::into_inner) = Some(transform);
    }

    pub(crate) fn clear(&self) {
        *self.0.write().unwrap_or_else(PoisonError::into_inner) = None;
    }

    pub(crate) fn apply(&self, req: HttpRequest) -> HttpRequest {
        let hook = self
            .0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        match hook {
            Some(transform) => transform(req),
            None => req,
        }
    }
}

/// Whether `url` uses a remote `http://` or `https://` scheme that requires network access on cache miss.
pub fn is_remote_http_url(url: &str) -> bool {
    url.get(..7)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("http://"))
        || url
            .get(..8)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("https://"))
}

/// Malformed inline `data:` URI.
#[derive(thiserror::Error, Debug)]
#[error("invalid data URI: {reason}")]
pub struct DataUrlError {
    reason: &'static str,
}

/// Decodes a `data:[<mediatype>][;base64],<data>` URI locally without touching any network transport.
/// Returns `None` when `url` does not start with `data:`.
pub(crate) fn decode_data_url(
    url: &str,
    range: Option<ByteRange>,
) -> Option<Result<Vec<u8>, SourceFetchError>> {
    if !url
        .get(..5)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("data:"))
    {
        return None;
    }
    let rest = &url[5..];
    let Some((meta, payload)) = rest.split_once(',') else {
        return Some(Err(SourceFetchError(Box::new(DataUrlError {
            reason: "missing comma separator",
        }))));
    };
    let is_base64 = meta
        .rsplit(';')
        .next()
        .is_some_and(|token| token.trim().eq_ignore_ascii_case("base64"));
    let decoded = if is_base64 {
        decode_base64(payload).ok_or_else(|| {
            SourceFetchError(Box::new(DataUrlError {
                reason: "invalid base64 payload",
            }))
        })
    } else {
        Ok(percent_decode_bytes(payload))
    };
    Some(decoded.and_then(|bytes| match range {
        Some(range) => range.slice(url, &bytes).map(<[u8]>::to_vec),
        None => Ok(bytes),
    }))
}

fn percent_decode_bytes(input: &str) -> Vec<u8> {
    let raw = input.as_bytes();
    let mut out = Vec::with_capacity(raw.len());
    let mut idx = 0;
    while idx < raw.len() {
        if raw[idx] == b'%' && idx + 2 < raw.len() {
            if let (Some(hi), Some(lo)) = (
                (raw[idx + 1] as char).to_digit(16),
                (raw[idx + 2] as char).to_digit(16),
            ) {
                out.push(((hi << 4) | lo) as u8);
                idx += 3;
                continue;
            }
        }
        out.push(raw[idx]);
        idx += 1;
    }
    out
}

fn decode_base64(input: &str) -> Option<Vec<u8>> {
    let mut clean = Vec::with_capacity(input.len());
    for byte in input.bytes() {
        if !byte.is_ascii_whitespace() {
            clean.push(byte);
        }
    }
    if clean.is_empty() {
        return Some(Vec::new());
    }
    let mut out = Vec::with_capacity(clean.len() * 3 / 4);
    let mut buf = 0_u32;
    let mut bits = 0_u8;
    let mut seen_pad = false;
    for byte in clean {
        if byte == b'=' {
            seen_pad = true;
            continue;
        }
        if seen_pad {
            return None;
        }
        let val = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return None,
        };
        buf = (buf << 6) | u32::from(val);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xff) as u8);
        }
    }
    Some(out)
}
