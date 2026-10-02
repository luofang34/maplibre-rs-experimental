//! Tiles read out of a PMTiles archive with HTTP range requests.
//!
//! As with GL JS's `pmtiles` protocol, a source names an archive as `pmtiles://<archive URL>`.
//! That URL answers with a TileJSON document built from the archive's header and metadata,
//! whose tile template `pmtiles://<archive URL>/{z}/{x}/{y}` then answers with each tile. The
//! header and directories are read once per archive and kept; a tile costs one range request.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, PoisonError},
};

use async_trait::async_trait;
use thiserror::Error;

pub use self::format::PmtilesFormatError;
use self::format::{
    decompress, find, parse_directory, tile_id, Entry, Found, Header, HEADER_BYTES,
};
use crate::io::source_client::{ByteRange, HttpClient, SourceFetchError};

mod format;

const SCHEME: &str = "pmtiles://";
/// Root, leaf and leaf-of-leaf: the deepest the format nests directories.
const MAX_DEPTH: usize = 3;

/// What a `pmtiles://` URL asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PmtilesUrl {
    /// The archive's TileJSON.
    Archive(String),
    /// One tile of the archive.
    Tile {
        /// The archive's own URL.
        archive: String,
        /// Zoom.
        z: u8,
        /// Column.
        x: u32,
        /// Row.
        y: u32,
    },
}

impl PmtilesUrl {
    /// Reads a `pmtiles://` URL; any other URL is `None`.
    pub fn parse(url: &str) -> Option<Self> {
        let rest = url.strip_prefix(SCHEME)?;
        let mut parts = rest.rsplitn(4, '/');
        let tile = (|| {
            let y = parts.next()?.parse().ok()?;
            let x = parts.next()?.parse().ok()?;
            let z = parts.next()?.parse().ok()?;
            Some((parts.next()?.to_owned(), z, x, y))
        })();
        Some(match tile {
            Some((archive, z, x, y)) => Self::Tile { archive, z, x, y },
            None => Self::Archive(rest.to_owned()),
        })
    }
}

/// An archive could not be read.
#[derive(Error, Debug)]
#[error("cannot read PMTiles archive {archive}")]
pub struct PmtilesError {
    /// The archive's URL.
    pub archive: String,
    /// Why.
    #[source]
    pub source: PmtilesFormatError,
}

fn failed(archive: &str, source: PmtilesFormatError) -> SourceFetchError {
    SourceFetchError(Box::new(PmtilesError {
        archive: archive.to_owned(),
        source,
    }))
}

struct Archive {
    header: Header,
    root: Vec<Entry>,
    leaves: Mutex<HashMap<u64, Arc<Vec<Entry>>>>,
}

/// Reads tiles out of PMTiles archives, keeping each archive's header and directories.
#[derive(Clone, Default)]
pub struct PmtilesReader {
    archives: Arc<Mutex<HashMap<String, Arc<Archive>>>>,
}

async fn range<C: HttpClient>(
    client: &C,
    url: &str,
    offset: u64,
    length: u64,
) -> Result<Vec<u8>, SourceFetchError> {
    client.fetch_range(url, ByteRange { offset, length }).await
}

impl PmtilesReader {
    /// Answers a `pmtiles://` request through `client`'s range requests.
    pub async fn fetch<C: HttpClient>(
        &self,
        client: &C,
        url: &str,
        request: PmtilesUrl,
    ) -> Result<Vec<u8>, SourceFetchError> {
        match request {
            PmtilesUrl::Archive(archive) => self.tile_json(client, &archive).await,
            PmtilesUrl::Tile { archive, z, x, y } => {
                let id = tile_id(z, x, y).map_err(|error| failed(&archive, error))?;
                let opened = self.open(client, &archive).await?;
                let Some(entry) = self.locate(client, &archive, &opened, id).await? else {
                    return Err(SourceFetchError::not_found(url));
                };
                let bytes = range(
                    client,
                    &archive,
                    opened.header.tiles_offset + entry.offset,
                    entry.length,
                )
                .await?;
                decompress(opened.header.tile_compression, bytes, "tile")
                    .map_err(|error| failed(&archive, error))
            }
        }
    }

    async fn open<C: HttpClient>(
        &self,
        client: &C,
        archive: &str,
    ) -> Result<Arc<Archive>, SourceFetchError> {
        if let Some(opened) = self.cached(archive) {
            return Ok(opened);
        }
        let bytes = range(client, archive, 0, HEADER_BYTES).await?;
        let header = Header::parse(&bytes).map_err(|error| failed(archive, error))?;
        let root = self
            .directory(client, archive, &header, header.root.0, header.root.1)
            .await?;
        let opened = Arc::new(Archive {
            header,
            root,
            leaves: Mutex::default(),
        });
        self.archives
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(archive.to_owned(), opened.clone());
        Ok(opened)
    }

    fn cached(&self, archive: &str) -> Option<Arc<Archive>> {
        self.archives
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(archive)
            .cloned()
    }

    async fn directory<C: HttpClient>(
        &self,
        client: &C,
        archive: &str,
        header: &Header,
        offset: u64,
        length: u64,
    ) -> Result<Vec<Entry>, SourceFetchError> {
        let bytes = range(client, archive, offset, length).await?;
        decompress(header.internal_compression, bytes, "directory")
            .and_then(|bytes| parse_directory(&bytes))
            .map_err(|error| failed(archive, error))
    }

    async fn locate<C: HttpClient>(
        &self,
        client: &C,
        archive: &str,
        opened: &Archive,
        id: u64,
    ) -> Result<Option<Entry>, SourceFetchError> {
        let mut directory = Arc::new(opened.root.clone());
        for _ in 0..MAX_DEPTH {
            match find(&directory, id) {
                None => return Ok(None),
                Some(Found::Tile(entry)) => return Ok(Some(entry)),
                Some(Found::Leaf(leaf)) => {
                    let offset = opened.header.leaves_offset + leaf.offset;
                    let known = opened
                        .leaves
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .get(&offset)
                        .cloned();
                    directory = match known {
                        Some(entries) => entries,
                        None => {
                            let entries = Arc::new(
                                self.directory(
                                    client,
                                    archive,
                                    &opened.header,
                                    offset,
                                    leaf.length,
                                )
                                .await?,
                            );
                            opened
                                .leaves
                                .lock()
                                .unwrap_or_else(PoisonError::into_inner)
                                .insert(offset, entries.clone());
                            entries
                        }
                    };
                }
            }
        }
        Err(failed(archive, PmtilesFormatError::TooDeep))
    }

    /// The TileJSON GL JS's protocol builds: zooms, bounds and centre from the header, the
    /// rest from the archive's metadata.
    async fn tile_json<C: HttpClient>(
        &self,
        client: &C,
        archive: &str,
    ) -> Result<Vec<u8>, SourceFetchError> {
        let opened = self.open(client, archive).await?;
        let header = &opened.header;
        let metadata = range(client, archive, header.metadata.0, header.metadata.1).await?;
        let metadata = decompress(header.internal_compression, metadata, "metadata")
            .map_err(|error| failed(archive, error))?;
        let mut document = match serde_json::from_slice::<serde_json::Value>(&metadata) {
            Ok(serde_json::Value::Object(fields)) => fields,
            _ => serde_json::Map::new(),
        };
        let [west, south, east, north] = header.bounds;
        for (key, value) in [
            ("tilejson", serde_json::json!("3.0.0")),
            (
                "tiles",
                serde_json::json!([format!("{SCHEME}{archive}/{{z}}/{{x}}/{{y}}")]),
            ),
            ("minzoom", serde_json::json!(header.zooms.0)),
            ("maxzoom", serde_json::json!(header.zooms.1)),
            ("bounds", serde_json::json!([west, south, east, north])),
            (
                "center",
                serde_json::json!([header.center.0, header.center.1, header.center.2]),
            ),
        ] {
            document.insert(key.to_owned(), value);
        }
        serde_json::to_vec(&document).map_err(|error| SourceFetchError(Box::new(error)))
    }
}

/// A transport that also answers `pmtiles://` URLs, for hosts whose loader is not a
/// [`SharedLoader`](crate::io::resource_loader::SharedLoader), which reads archives itself.
#[derive(Clone)]
pub struct PmtilesClient<HC: HttpClient> {
    inner: HC,
    reader: PmtilesReader,
}

impl<HC: HttpClient> PmtilesClient<HC> {
    /// Reads archives through `inner`'s range requests and passes every other URL to it.
    pub fn new(inner: HC) -> Self {
        Self {
            inner,
            reader: PmtilesReader::default(),
        }
    }
}

impl<HC: HttpClient + Default> Default for PmtilesClient<HC> {
    fn default() -> Self {
        Self::new(HC::default())
    }
}

#[cfg_attr(not(feature = "thread-safe-futures"), async_trait(?Send))]
#[cfg_attr(feature = "thread-safe-futures", async_trait)]
impl<HC: HttpClient> HttpClient for PmtilesClient<HC> {
    async fn fetch(&self, url: &str) -> Result<Vec<u8>, SourceFetchError> {
        match PmtilesUrl::parse(url) {
            Some(request) => self.reader.fetch(&self.inner, url, request).await,
            None => self.inner.fetch(url).await,
        }
    }

    async fn fetch_range(&self, url: &str, range: ByteRange) -> Result<Vec<u8>, SourceFetchError> {
        self.inner.fetch_range(url, range).await
    }
}

#[cfg(test)]
mod tests;
