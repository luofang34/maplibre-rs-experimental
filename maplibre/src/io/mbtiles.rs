//! Runtime SQLite MBTiles 1.3 archive reader and [`HttpClient`] wrapper.
//!
//! A source names a local archive as `mbtiles://path/to/archive.mbtiles` (or
//! `mbtiles:///absolute/path/to/archive.mbtiles`). Fetching the archive URL returns a TileJSON
//! 3.0 document synthesized from `SELECT name, value FROM metadata` with `"scheme": "xyz"` and a
//! tile template `mbtiles://<archive>/{z}/{x}/{y}`. Fetching a tile URL translates XYZ `y` into
//! MBTiles TMS `tile_row = (1 << z) - 1 - y`, runs the SQLite query on the CPU worker pool, and
//! transparently decompresses gzip (`0x1f 0x8b`) or zlib (`0x78 ...`) vector tiles while leaving
//! PNG, JPEG, and WebP payloads untouched.

mod format;

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, PoisonError},
};

use async_trait::async_trait;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use thiserror::Error;

use self::format::{build_tile_json, decode_tile_payload, resolve_archive_path};
use crate::io::{
    cpu_work::run_cpu_bound,
    source_client::{
        ByteRange, HttpClient, HttpRequest, NetworkStatus, ResourceKind, SourceFetchError,
    },
};

const SCHEME: &str = "mbtiles://";
const MAX_POOLED_CONNECTIONS: usize = 8;

/// Parsed representation of an `mbtiles://` URL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MbtilesUrl {
    /// Requests the synthesized TileJSON document for an `.mbtiles` SQLite file.
    Archive(String),
    /// Requests a single `{z}/{x}/{y}` tile from an `.mbtiles` SQLite file.
    Tile {
        /// Archive path portion following `mbtiles://`.
        archive: String,
        /// Tile zoom level (`0..=31`).
        z: u8,
        /// XYZ tile column.
        x: u32,
        /// XYZ tile row (converted internally to TMS `tile_row`).
        y: u32,
    },
}

impl MbtilesUrl {
    /// Parses an `mbtiles://` URL into an archive metadata or tile request, or `None` for other schemes.
    pub fn parse(url: &str) -> Option<Self> {
        let rest = url.strip_prefix(SCHEME)?;
        let clean = rest.split(['?', '#']).next().unwrap_or(rest);
        if clean.is_empty() {
            return None;
        }
        let mut parts = clean.rsplitn(4, '/');
        let tile = (|| {
            let y_part = parts.next()?;
            let y = parse_tile_y_segment(y_part)?;
            let x: u32 = parts.next()?.parse().ok()?;
            let z: u8 = parts.next()?.parse().ok()?;
            let archive = parts.next()?;
            if archive.is_empty() {
                return None;
            }
            Some((archive.to_owned(), z, x, y))
        })();
        Some(match tile {
            Some((archive, z, x, y)) => Self::Tile { archive, z, x, y },
            None => Self::Archive(clean.to_owned()),
        })
    }
}

fn parse_tile_y_segment(segment: &str) -> Option<u32> {
    match segment.split_once('.') {
        None => segment.parse().ok(),
        Some((digits, ext)) => {
            let valid_ext = matches!(
                ext.to_ascii_lowercase().as_str(),
                "pbf" | "mvt" | "mlt" | "png" | "jpg" | "jpeg" | "webp"
            );
            if valid_ext {
                digits.parse().ok()
            } else {
                None
            }
        }
    }
}

/// Reason an `mbtiles://` archive or tile payload could not be read.
#[derive(Error, Debug)]
pub enum MbtilesFormatError {
    /// Opening or querying the SQLite file failed.
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// Decompressing a gzip- or zlib-encoded tile payload failed.
    #[error("failed to decompress {encoding} MBTiles tile: {source}")]
    Decompress {
        /// Detected compression stream (`"gzip"` or `"zlib"`).
        encoding: &'static str,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// Serializing the synthesized TileJSON document failed.
    #[error("failed to serialize MBTiles TileJSON: {0}")]
    Json(#[from] serde_json::Error),
    /// Dispatching blocking SQLite work to the CPU pool failed.
    #[error(transparent)]
    CpuPool(#[from] crate::io::cpu_work::CpuWorkError),
}

/// Top-level error reported when an `mbtiles://` archive operation fails.
#[derive(Error, Debug)]
#[error("cannot read MBTiles archive {archive}: {source}")]
pub struct MbtilesError {
    /// Archive identifier from the `mbtiles://` URL.
    pub archive: String,
    /// Specific SQLite, decompression, or JSON cause.
    #[source]
    pub source: MbtilesFormatError,
}

fn failed(archive: &str, source: MbtilesFormatError) -> SourceFetchError {
    SourceFetchError(Box::new(MbtilesError {
        archive: archive.to_owned(),
        source,
    }))
}

struct ArchiveState {
    path: PathBuf,
    tile_json: Vec<u8>,
    pool: Mutex<Vec<Connection>>,
}

impl ArchiveState {
    fn open(archive_id: &str, path: PathBuf) -> Result<Self, MbtilesFormatError> {
        let conn = open_readonly_connection(&path)?;
        let tile_json = build_tile_json(&conn, archive_id)?;
        Ok(Self {
            path,
            tile_json,
            pool: Mutex::new(vec![conn]),
        })
    }

    fn checkout(&self) -> Result<Connection, rusqlite::Error> {
        let cached = self
            .pool
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop();
        match cached {
            Some(conn) => Ok(conn),
            None => open_readonly_connection(&self.path),
        }
    }

    fn checkin(&self, conn: Connection) {
        let mut guard = self.pool.lock().unwrap_or_else(PoisonError::into_inner);
        if guard.len() < MAX_POOLED_CONNECTIONS {
            guard.push(conn);
        }
    }
}

fn open_readonly_connection(path: &Path) -> Result<Connection, rusqlite::Error> {
    Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
}

/// Runtime reader for `.mbtiles` SQLite databases, pooling read-only connections and metadata.
#[derive(Clone, Default)]
pub struct MbtilesReader {
    archives: Arc<Mutex<HashMap<String, Arc<ArchiveState>>>>,
}

impl MbtilesReader {
    /// Creates an empty reader with no opened SQLite archives.
    pub fn new() -> Self {
        Self::default()
    }

    /// Resolves `request` for `url` using default non-volatile request settings.
    pub async fn fetch(&self, url: &str, request: MbtilesUrl) -> Result<Vec<u8>, SourceFetchError> {
        let base = HttpRequest::new(url, ResourceKind::Unknown);
        self.fetch_with_context(&base, request).await
    }

    /// Resolves `request` while honoring `base.cache_policy` and `base.range`.
    pub async fn fetch_with_context(
        &self,
        base: &HttpRequest,
        request: MbtilesUrl,
    ) -> Result<Vec<u8>, SourceFetchError> {
        let bytes = match request {
            MbtilesUrl::Archive(archive) => {
                let opened = self
                    .open_archive(&archive, base.cache_policy.is_volatile())
                    .await?;
                opened.tile_json.clone()
            }
            MbtilesUrl::Tile { archive, z, x, y } => {
                if z >= 32 {
                    return Err(SourceFetchError::not_found(&base.url));
                }
                let span = 1_u64 << u32::from(z);
                if u64::from(x) >= span || u64::from(y) >= span {
                    return Err(SourceFetchError::not_found(&base.url));
                }
                let tms_row = (span - 1 - u64::from(y)) as u32;
                let opened = self
                    .open_archive(&archive, base.cache_policy.is_volatile())
                    .await?;
                let archive_label = archive.clone();
                let maybe_tile = run_cpu_bound(move || -> Result<Option<Vec<u8>>, MbtilesFormatError> {
                    let conn = opened.checkout()?;
                    let row_result: Result<Option<Vec<u8>>, rusqlite::Error> = conn
                        .query_row(
                            "SELECT tile_data FROM tiles WHERE zoom_level = ?1 AND tile_column = ?2 AND tile_row = ?3",
                            params![z, x, tms_row],
                            |row| row.get::<_, Vec<u8>>(0),
                        )
                        .optional();
                    opened.checkin(conn);
                    match row_result? {
                        Some(raw) => decode_tile_payload(raw).map(Some),
                        None => Ok(None),
                    }
                })
                .await
                .map_err(|err| failed(&archive_label, MbtilesFormatError::CpuPool(err)))?
                .map_err(|err| failed(&archive_label, err))?;

                match maybe_tile {
                    Some(tile) => tile,
                    None => return Err(SourceFetchError::not_found(&base.url)),
                }
            }
        };

        match base.range {
            Some(range) => range.slice(&base.url, &bytes).map(<[u8]>::to_vec),
            None => Ok(bytes),
        }
    }

    async fn open_archive(
        &self,
        archive: &str,
        volatile: bool,
    ) -> Result<Arc<ArchiveState>, SourceFetchError> {
        if !volatile {
            if let Some(existing) = self
                .archives
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get(archive)
                .cloned()
            {
                return Ok(existing);
            }
        }
        let path = resolve_archive_path(archive);
        let archive_owned = archive.to_owned();
        let opened_res = run_cpu_bound(move || ArchiveState::open(&archive_owned, path))
            .await
            .map_err(|err| failed(archive, MbtilesFormatError::CpuPool(err)))?;
        let opened = Arc::new(opened_res.map_err(|err| failed(archive, err))?);
        if !volatile {
            self.archives
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(archive.to_owned(), Arc::clone(&opened));
        }
        Ok(opened)
    }
}

/// [`HttpClient`] decorator that intercepts `mbtiles://` requests and delegates all other schemes
/// to an inner [`HttpClient`].
#[derive(Clone)]
pub struct MbtilesClient<HC: HttpClient> {
    inner: HC,
    reader: MbtilesReader,
}

impl<HC: HttpClient> MbtilesClient<HC> {
    /// Wraps `inner` with runtime SQLite `mbtiles://` support.
    pub fn new(inner: HC) -> Self {
        Self {
            inner,
            reader: MbtilesReader::new(),
        }
    }

    /// Borrows the wrapped inner [`HttpClient`].
    pub fn inner(&self) -> &HC {
        &self.inner
    }
}

impl<HC: HttpClient + Default> Default for MbtilesClient<HC> {
    fn default() -> Self {
        Self::new(HC::default())
    }
}

#[cfg_attr(not(feature = "thread-safe-futures"), async_trait(?Send))]
#[cfg_attr(feature = "thread-safe-futures", async_trait)]
impl<HC: HttpClient> HttpClient for MbtilesClient<HC> {
    async fn fetch(&self, url: &str) -> Result<Vec<u8>, SourceFetchError> {
        match MbtilesUrl::parse(url) {
            Some(req) => self.reader.fetch(url, req).await,
            None => self.inner.fetch(url).await,
        }
    }

    async fn fetch_range(&self, url: &str, range: ByteRange) -> Result<Vec<u8>, SourceFetchError> {
        match MbtilesUrl::parse(url) {
            Some(parsed) => {
                let req = HttpRequest::new(url, ResourceKind::Unknown).with_range(range);
                self.reader.fetch_with_context(&req, parsed).await
            }
            None => self.inner.fetch_range(url, range).await,
        }
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
        if let Some(parsed) = MbtilesUrl::parse(&req.url) {
            // Local SQLite archives remain readable regardless of online/offline status.
            return self.reader.fetch_with_context(req, parsed).await;
        }
        self.inner.fetch_request_with_status(req, status).await
    }
}

#[cfg(test)]
mod tests;
