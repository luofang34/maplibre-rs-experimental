use std::{
    io::Write,
    sync::{Arc, Mutex},
};

use super::*;
use crate::io::resource_loader::SharedLoader;

fn varint(mut value: u64, out: &mut Vec<u8>) {
    while value >= 0x80 {
        out.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

/// Encodes `entries` of `(tile id, offset, length, run length)`.
fn directory(entries: &[(u64, u64, u64, u64)]) -> Vec<u8> {
    let mut out = Vec::new();
    varint(entries.len() as u64, &mut out);
    let mut last = 0;
    for (id, ..) in entries {
        varint(id - last, &mut out);
        last = *id;
    }
    for (.., run) in entries {
        varint(*run, &mut out);
    }
    for (_, _, length, _) in entries {
        varint(*length, &mut out);
    }
    for (_, offset, ..) in entries {
        varint(offset + 1, &mut out);
    }
    out
}

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(bytes).expect("gzip");
    encoder.finish().expect("gzip")
}

/// A tile's `(z, x, y)` and bytes.
pub(crate) type ArchivedTile = ((u8, u32, u32), Vec<u8>);

/// An archive of `tiles`, gzipped, with the tile entries in a leaf directory when `leaf`.
pub(crate) fn archive(tiles: &[ArchivedTile], leaf: bool) -> Vec<u8> {
    let mut tiles: Vec<(u64, Vec<u8>)> = tiles
        .iter()
        .map(|((z, x, y), bytes)| (tile_id(*z, *x, *y).expect("id"), gzip(bytes)))
        .collect();
    tiles.sort_by_key(|(id, _)| *id);
    let mut data = Vec::new();
    let mut entries = Vec::new();
    for (id, bytes) in &tiles {
        entries.push((*id, data.len() as u64, bytes.len() as u64, 1));
        data.extend_from_slice(bytes);
    }
    let (root, leaves) = if leaf {
        let leaves = gzip(&directory(&entries));
        let root = gzip(&directory(&[(entries[0].0, 0, leaves.len() as u64, 0)]));
        (root, leaves)
    } else {
        (gzip(&directory(&entries)), Vec::new())
    };
    let metadata = gzip(br#"{"name":"fixture","vector_layers":[{"id":"roads"}]}"#);
    let root_at = HEADER_BYTES;
    let metadata_at = root_at + root.len() as u64;
    let leaves_at = metadata_at + metadata.len() as u64;
    let data_at = leaves_at + leaves.len() as u64;
    let mut out = b"PMTiles\x03".to_vec();
    for value in [
        root_at,
        root.len() as u64,
        metadata_at,
        metadata.len() as u64,
        leaves_at,
        leaves.len() as u64,
        data_at,
        data.len() as u64,
        tiles.len() as u64,
        tiles.len() as u64,
        tiles.len() as u64,
    ] {
        out.extend_from_slice(&value.to_le_bytes());
    }
    // Clustered, gzip directories, gzip tiles, vector tiles, zooms 0 to 2.
    out.extend_from_slice(&[1, 2, 2, 1, 0, 2]);
    for degrees in [-180.0_f64, -85.0, 180.0, 85.0] {
        out.extend_from_slice(&((degrees * 1e7) as i32).to_le_bytes());
    }
    out.push(1);
    for degrees in [10.0_f64, 20.0] {
        out.extend_from_slice(&((degrees * 1e7) as i32).to_le_bytes());
    }
    assert_eq!(out.len() as u64, HEADER_BYTES);
    out.extend(root);
    out.extend(metadata);
    out.extend(leaves);
    out.extend(data);
    out
}

/// Serves one archive, and only ever as ranges, recording each range asked for.
#[derive(Clone)]
pub(crate) struct ArchiveServer {
    pub bytes: Arc<Vec<u8>>,
    pub ranges: Arc<Mutex<Vec<ByteRange>>>,
    pub served: Arc<tokio::sync::Notify>,
}

impl ArchiveServer {
    pub(crate) fn new(bytes: Vec<u8>) -> Self {
        Self {
            bytes: Arc::new(bytes),
            ranges: Arc::default(),
            served: Arc::default(),
        }
    }
}

#[cfg_attr(not(feature = "thread-safe-futures"), async_trait(?Send))]
#[cfg_attr(feature = "thread-safe-futures", async_trait)]
impl HttpClient for ArchiveServer {
    async fn fetch(&self, url: &str) -> Result<Vec<u8>, SourceFetchError> {
        panic!("an archive is read in ranges, not whole: {url}");
    }

    async fn fetch_range(&self, url: &str, range: ByteRange) -> Result<Vec<u8>, SourceFetchError> {
        self.ranges.lock().expect("ranges").push(range);
        self.served.notify_one();
        range.slice(url, &self.bytes).map(<[u8]>::to_vec)
    }
}

const ARCHIVE: &str = "https://tiles.invalid/world.pmtiles";

fn tiles() -> Vec<ArchivedTile> {
    vec![
        ((0, 0, 0), b"zero".to_vec()),
        ((1, 1, 0), b"one-east".to_vec()),
        ((2, 3, 3), b"two-corner".to_vec()),
    ]
}

#[test]
fn urls_name_an_archive_or_one_of_its_tiles() {
    assert_eq!(PmtilesUrl::parse("https://a.invalid/1/2/3"), None);
    assert_eq!(
        PmtilesUrl::parse("pmtiles://https://a.invalid/w.pmtiles"),
        Some(PmtilesUrl::Archive("https://a.invalid/w.pmtiles".into()))
    );
    assert_eq!(
        PmtilesUrl::parse("pmtiles://https://a.invalid/w.pmtiles/4/5/6"),
        Some(PmtilesUrl::Tile {
            archive: "https://a.invalid/w.pmtiles".into(),
            z: 4,
            x: 5,
            y: 6
        })
    );
}

#[tokio::test]
async fn tiles_come_out_of_the_archive_one_range_each_after_the_directories() {
    for leaf in [false, true] {
        let server = ArchiveServer::new(archive(&tiles(), leaf));
        let client = PmtilesClient::new(server.clone());
        for ((z, x, y), bytes) in tiles() {
            let url = format!("pmtiles://{ARCHIVE}/{z}/{x}/{y}");
            assert_eq!(client.fetch(&url).await.expect("tile"), bytes, "{url}");
        }
        let missing = client
            .fetch(&format!("pmtiles://{ARCHIVE}/2/0/0"))
            .await
            .expect_err("no such tile");
        assert!(missing.is_not_found(), "a tile not in the archive is empty");
        // Header and root, the leaf once when there is one, then one range per tile.
        let directories = if leaf { 3 } else { 2 };
        assert_eq!(
            server.ranges.lock().expect("ranges").len(),
            directories + tiles().len(),
            "leaf {leaf}"
        );
    }
}

#[tokio::test]
async fn an_archive_answers_with_tile_json_from_its_header_and_metadata() {
    let client = PmtilesClient::new(ArchiveServer::new(archive(&tiles(), false)));
    let document = client
        .fetch(&format!("pmtiles://{ARCHIVE}"))
        .await
        .expect("TileJSON");
    let tile_json: crate::io::tile_json::TileJson =
        serde_json::from_slice(&document).expect("valid TileJSON");
    assert_eq!(
        tile_json.tiles,
        vec![format!("pmtiles://{ARCHIVE}/{{z}}/{{x}}/{{y}}")]
    );
    assert_eq!((tile_json.minzoom, tile_json.maxzoom), (Some(0), Some(2)));
    assert_eq!(tile_json.bounds, Some((-180.0, -85.0, 180.0, 85.0)));
    let fields: serde_json::Value = serde_json::from_slice(&document).expect("JSON");
    assert_eq!(fields["vector_layers"][0]["id"], "roads");
}

#[tokio::test]
async fn a_shared_loader_reads_archives_too() {
    let server = ArchiveServer::new(archive(&tiles(), true));
    let loader = SharedLoader::new(server.clone());
    assert_eq!(
        loader
            .fetch(&format!("pmtiles://{ARCHIVE}/1/1/0"))
            .await
            .expect("tile"),
        b"one-east"
    );
}

#[tokio::test]
async fn something_else_at_the_url_is_not_an_archive() {
    let client = PmtilesClient::new(ArchiveServer::new(vec![0; 200]));
    let error = client
        .fetch(&format!("pmtiles://{ARCHIVE}/0/0/0"))
        .await
        .expect_err("not PMTiles");
    assert!(
        error.describe().contains("not a PMTiles v3 archive"),
        "{}",
        error.describe()
    );
}

fn road_tile() -> Vec<u8> {
    use geozero::mvt::Message as _;
    geozero::mvt::Tile {
        layers: vec![geozero::mvt::tile::Layer {
            version: 2,
            name: "roads".into(),
            extent: Some(4096),
            features: vec![geozero::mvt::tile::Feature {
                r#type: Some(geozero::mvt::tile::GeomType::Linestring as i32),
                geometry: vec![9, 2, 2, 10, 2, 2],
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
    .encode_to_vec()
}

#[tokio::test]
async fn a_map_draws_vector_tiles_out_of_an_archive() {
    use crate::{
        headless::{create_headless_renderer_with_loader, map::HeadlessMap},
        io::tile_retry::RequestKind,
        render::{frame_signals::ResourceReady, RenderPlugin},
        style::Style,
        vector::{DefaultVectorTransferables, VectorPlugin},
    };
    let tiles: Vec<_> = (0..4)
        .flat_map(|x| (0..4).map(move |y| ((2, x, y), road_tile())))
        .chain([((0, 0, 0), road_tile())])
        .collect();
    let server = ArchiveServer::new(archive(&tiles, true));
    let (kernel, renderer) = create_headless_renderer_with_loader(
        64,
        64,
        Default::default(),
        SharedLoader::new(server.clone()),
    )
    .await
    .expect("renderer");
    let style: Style = serde_json::from_value(serde_json::json!({
        "version": 8, "zoom": 2,
        "sources": {"v": {"type": "vector",
            "tiles": [format!("pmtiles://{ARCHIVE}/{{z}}/{{x}}/{{y}}")]}},
        "layers": [{"id": "roads", "type": "line", "source": "v", "source-layer": "roads"}]
    }))
    .expect("style");
    let mut map = HeadlessMap::new(
        style,
        renderer,
        kernel,
        vec![
            Box::new(RenderPlugin),
            Box::new(VectorPlugin::<DefaultVectorTransferables>::default()),
        ],
    )
    .expect("map");
    let mut loaded = 0;
    for _ in 0..50 {
        map.run_frame().expect("frame");
        loaded += map
            .take_ready_resources()
            .iter()
            .filter(|resource| {
                matches!(
                    resource,
                    ResourceReady::Tile {
                        kind: RequestKind::Vector,
                        loaded: true,
                        ..
                    }
                )
            })
            .count();
        if loaded >= 4 {
            break;
        }
        tokio::time::timeout(
            std::time::Duration::from_millis(200),
            server.served.notified(),
        )
        .await
        .ok();
    }
    assert!(
        loaded >= 4,
        "vector tiles load out of the archive: {loaded}"
    );
    let ranges = server.ranges.lock().expect("ranges").len();
    assert!(
        ranges < loaded + 4,
        "the header and directories are read once, then a range per tile: {ranges} ranges for {loaded} tiles"
    );
}

#[tokio::test]
async fn a_source_naming_an_archive_resolves_to_its_tiles() {
    use crate::{
        io::source_client::{HttpSourceClient, SourceClient},
        style::{source::Source, Style},
    };
    let mut style: Style = serde_json::from_value(serde_json::json!({
        "version": 8,
        "sources": {"v": {"type": "vector", "url": format!("pmtiles://{ARCHIVE}")}},
        "layers": []
    }))
    .expect("style");
    let client = SourceClient::new(HttpSourceClient::new(SharedLoader::new(
        ArchiveServer::new(archive(&tiles(), false)),
    )));
    crate::io::tile_json::resolve_tile_json_sources(&mut style, &client).await;
    let Some(Source::Vector(source)) = style.sources.get("v") else {
        panic!("vector source");
    };
    assert_eq!(
        source.tiles.as_deref(),
        Some(&[format!("pmtiles://{ARCHIVE}/{{z}}/{{x}}/{{y}}")][..])
    );
}
