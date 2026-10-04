#![allow(clippy::expect_used, clippy::panic)]
use std::sync::Mutex;

use prost::Message as _;

use super::*;
use crate::io::source_client::{HttpSourceClient, SourceFetchError};

mod scope;

#[derive(Clone)]
struct Assets {
    urls: Arc<Mutex<Vec<String>>>,
    png: Vec<u8>,
    glyphs: Vec<u8>,
}
#[cfg_attr(not(feature="thread-safe-futures"), async_trait::async_trait(?Send))]
#[cfg_attr(feature = "thread-safe-futures", async_trait::async_trait)]
impl HttpClient for Assets {
    async fn fetch(&self, url: &str) -> Result<Vec<u8>, SourceFetchError> {
        self.urls.lock().expect("request list").push(url.into());
        if url.ends_with("0-255.pbf") {
            return Ok(include_bytes!("../../../../../data/0-255.pbf").to_vec());
        }
        if url.ends_with("256-511.pbf") {
            return Ok(self.glyphs.clone());
        }
        if url.contains(".png?") {
            return Ok(self.png.clone());
        }
        if url.contains(".json?") {
            return Ok(br#"{"marker":{"x":1,"y":1,"width":2,"height":2,"pixelRatio":2}}"#.to_vec());
        }
        Err(SourceFetchError::not_found(url))
    }
}

#[tokio::test]
async fn style_assets_fetch_unicode_ranges_and_sprite_queries_without_hidden_layers() {
    let glyphs = crate::sdf::glyphs::Glyphs {
        stacks: vec![crate::sdf::glyphs::Fontstack {
            name: "Font A".into(),
            range: "256-511".into(),
            glyphs: vec![crate::sdf::glyphs::Glyph {
                id: 256,
                bitmap: Some(vec![255; 64]),
                width: 2,
                height: 2,
                left: 0,
                top: -9,
                advance: 4,
            }],
        }],
    }
    .encode_to_vec();
    let mut png = std::io::Cursor::new(Vec::new());
    image::RgbaImage::from_pixel(4, 4, image::Rgba([0, 255, 0, 191]))
        .write_to(&mut png, image::ImageFormat::Png)
        .expect("PNG");
    let assets = Assets {
        urls: Default::default(),
        png: png.into_inner(),
        glyphs,
    };
    let client = SourceClient::new(HttpSourceClient::new(assets.clone()));
    let style:Style=serde_json::from_value(serde_json::json!({"version":8,"sources":{},
        "glyphs":"https://fonts.invalid/{fontstack}/{range}.pbf","sprite":"https://sprites.invalid/sprite?key=test",
        "layers":[{"id":"label","type":"symbol","source":"map","source-layer":"places","minzoom":14,
            "layout":{"text-field":"Ā A","text-font":["Font A"],"icon-image":"marker"}},
            {"id":"hidden","type":"symbol","source":"map","source-layer":"places","minzoom":16,
            "layout":{"text-field":"Ж","text-font":["Hidden font"],"visibility":"none"}}]})).expect("style");
    let tile = geozero::mvt::Tile {
        layers: vec![geozero::mvt::tile::Layer {
            version: 2,
            name: "places".into(),
            features: vec![geozero::mvt::tile::Feature {
                r#type: Some(1),
                geometry: vec![9, 4096, 4096],
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
    .encode_to_vec();
    let atlas = load_symbol_assets(
        &client,
        SymbolAssetConfig::of(&style),
        &style.layers,
        &tile,
        12.,
    )
    .await
    .expect("assets");
    let urls = assets.urls.lock().expect("request list");
    assert_eq!(
        urls.len(),
        4,
        "unused fonts must not delay tile loading: {urls:?}"
    );
    assert!(urls.contains(&"https://fonts.invalid/Font%20A/256-511.pbf".to_string()));
    assert!(atlas.glyphs["Font A"].contains_key(&256));
    let icon = &atlas.icons["marker"];
    assert_eq!(icon.metrics[3], 2.0);
    let offset = ((icon.rect[1] * atlas.size[0] + icon.rect[0]) * 4) as usize;
    // Colour icons are stored premultiplied by their alpha.
    assert_eq!(&atlas.pixels[offset..offset + 4], &[0, 191, 0, 191]);
}

#[derive(Clone, Copy)]
enum Outage {
    NotFound(&'static str),
    Transient(&'static str),
}

/// Serves the bundled glyph range and a one-icon sprite; the named glyph range answers `status`.
#[derive(Clone)]
struct Server {
    urls: Arc<Mutex<Vec<String>>>,
    png: Vec<u8>,
    failing_range: Arc<Mutex<Option<Outage>>>,
}

#[cfg_attr(not(feature = "thread-safe-futures"), async_trait::async_trait(?Send))]
#[cfg_attr(feature = "thread-safe-futures", async_trait::async_trait)]
impl HttpClient for Server {
    async fn fetch(&self, url: &str) -> Result<Vec<u8>, SourceFetchError> {
        self.urls.lock().expect("request list").push(url.into());
        match *self.failing_range.lock().expect("script") {
            Some(Outage::Transient(range)) if url.contains(range) => {
                return Err(SourceFetchError::temporary(std::io::Error::other("reset")));
            }
            Some(Outage::NotFound(range)) if url.contains(range) => {
                return Err(SourceFetchError::not_found(url));
            }
            _ => {}
        }
        if url.ends_with(".png") {
            return Ok(self.png.clone());
        }
        if url.ends_with(".json") {
            return Ok(br#"{"marker":{"x":0,"y":0,"width":2,"height":2,"pixelRatio":1}}"#.to_vec());
        }
        Ok(include_bytes!("../../../../../data/0-255.pbf").to_vec())
    }
}

fn server(failing_range: Option<Outage>) -> (Server, SourceClient<Server>) {
    let mut png = std::io::Cursor::new(Vec::new());
    image::RgbaImage::from_pixel(4, 4, image::Rgba([1, 2, 3, 255]))
        .write_to(&mut png, image::ImageFormat::Png)
        .expect("PNG");
    let server = Server {
        urls: Default::default(),
        png: png.into_inner(),
        failing_range: Arc::new(Mutex::new(failing_range)),
    };
    let client = SourceClient::new(HttpSourceClient::new(server.clone()));
    (server, client)
}

fn labelled_tile_style(icon: &str) -> (Style, Vec<u8>) {
    let style: Style = serde_json::from_value(serde_json::json!({"version":8,"sources":{},
        "glyphs":"https://fonts.invalid/{fontstack}/{range}.pbf","sprite":"https://sprites.invalid/s",
        "layers":[{"id":"label","type":"symbol","source":"map","source-layer":"places",
            "layout":{"text-field":"A","text-font":["Font A"],"icon-image":icon}}]}))
    .expect("style");
    let tile = geozero::mvt::Tile {
        layers: vec![geozero::mvt::tile::Layer {
            version: 2,
            name: "places".into(),
            features: vec![geozero::mvt::tile::Feature {
                r#type: Some(1),
                geometry: vec![9, 4096, 4096],
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
    .encode_to_vec();
    (style, tile)
}

#[tokio::test]
async fn tiles_that_share_assets_fetch_and_decode_them_once() {
    let (server, client) = server(None);
    let (style, tile) = labelled_tile_style("marker");
    let first = load_symbol_assets(
        &client,
        SymbolAssetConfig::of(&style),
        &style.layers,
        &tile,
        12.,
    )
    .await
    .expect("first");
    let second = load_symbol_assets(
        &client,
        SymbolAssetConfig::of(&style),
        &style.layers,
        &tile,
        12.,
    )
    .await
    .expect("second");
    assert_eq!(
        server.urls.lock().expect("urls").len(),
        3,
        "range, sprite json and png"
    );
    assert_eq!(first.glyphs["Font A"].len(), second.glyphs["Font A"].len());
    assert!(second.icons.contains_key("marker"));
}

#[tokio::test]
async fn a_missing_glyph_range_leaves_the_tile_renderable_without_refetching() {
    let (server, client) = server(Some(Outage::NotFound("0-255")));
    let (style, tile) = labelled_tile_style("marker");
    for _ in 0..2 {
        let atlas = load_symbol_assets(
            &client,
            SymbolAssetConfig::of(&style),
            &style.layers,
            &tile,
            12.,
        )
        .await
        .expect("atlas");
        assert!(atlas.icons.contains_key("marker"), "sprites still load");
        assert!(
            atlas
                .glyphs
                .get("Font A")
                .is_some_and(|glyphs| glyphs.contains_key(&65)),
            "range 0 falls back to the bundled Latin glyphs"
        );
    }
    let range_requests = server
        .urls
        .lock()
        .expect("urls")
        .iter()
        .filter(|url| url.contains("0-255"))
        .count();
    assert_eq!(range_requests, 1);
}

#[tokio::test]
async fn an_icon_absent_from_the_sprite_sheet_is_left_out() {
    let (_server, client) = server(None);
    let (style, tile) = labelled_tile_style("unknown-icon");
    let atlas = load_symbol_assets(
        &client,
        SymbolAssetConfig::of(&style),
        &style.layers,
        &tile,
        12.,
    )
    .await
    .expect("atlas");
    assert!(atlas.icons.is_empty());
    assert!(atlas.glyphs.contains_key("Font A"));
}

#[tokio::test]
async fn a_transient_glyph_failure_is_returned_so_the_tile_can_retry() {
    let (server, client) = server(Some(Outage::Transient("0-255")));
    let (style, tile) = labelled_tile_style("marker");
    let error = load_symbol_assets(
        &client,
        SymbolAssetConfig::of(&style),
        &style.layers,
        &tile,
        12.,
    )
    .await
    .expect_err("transient failure propagates");
    assert!(error.url.ends_with("Font%20A/0-255.pbf"), "{error}");
    *server.failing_range.lock().expect("script") = None;
    let atlas = load_symbol_assets(
        &client,
        SymbolAssetConfig::of(&style),
        &style.layers,
        &tile,
        12.,
    )
    .await
    .expect("retry");
    assert!(atlas.glyphs["Font A"].contains_key(&65));
}

#[test]
fn an_image_added_to_the_style_is_packed_with_its_ratio_and_kind() {
    let mut builder = AtlasBuilder::new();
    let image = crate::style::StyleImage {
        width: 4,
        height: 2,
        data: vec![255; 4 * 2 * 4],
        pixel_ratio: 2.0,
        sdf: true,
    };
    pack_style_image(&mut builder, "added", &image);
    // Bytes that do not match the size are left out rather than read past.
    pack_style_image(
        &mut builder,
        "short",
        &crate::style::StyleImage {
            data: vec![0; 3],
            ..image.clone()
        },
    );
    let atlas = builder.finish();
    let entry = atlas.icons.get("added").expect("the added image");
    assert_eq!((entry.rect[2], entry.rect[3]), (4, 2));
    assert_eq!((entry.metrics[3], entry.kind), (2.0, 2));
    assert!(!atlas.icons.contains_key("short"));
}
