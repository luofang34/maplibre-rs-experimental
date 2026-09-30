//! Glyphs and a sprite fetched through the shared asset cache reach the screen.
#![allow(clippy::expect_used, clippy::panic)]
use std::sync::{Arc, Mutex};

use geozero::mvt::Message;

use super::{colored_bounds, render_with_atlas, style};
use crate::{
    io::source_client::{HttpClient, HttpSourceClient, SourceClient, SourceFetchError},
    sdf::assets::load_symbol_assets,
};

/// Serves the bundled glyph range for every range and a one-icon sprite sheet.
#[derive(Clone, Default)]
struct Server {
    urls: Arc<Mutex<Vec<String>>>,
}

#[cfg_attr(not(feature = "thread-safe-futures"), async_trait::async_trait(?Send))]
#[cfg_attr(feature = "thread-safe-futures", async_trait::async_trait)]
impl HttpClient for Server {
    async fn fetch(&self, url: &str) -> Result<Vec<u8>, SourceFetchError> {
        self.urls.lock().expect("urls").push(url.to_owned());
        if url.ends_with(".json") {
            return Ok(
                br#"{"marker":{"x":0,"y":0,"width":16,"height":16,"pixelRatio":1}}"#.to_vec(),
            );
        }
        if url.ends_with(".png") {
            let mut png = std::io::Cursor::new(Vec::new());
            image::RgbaImage::from_pixel(16, 16, image::Rgba([0, 255, 0, 255]))
                .write_to(&mut png, image::ImageFormat::Png)
                .expect("PNG");
            return Ok(png.into_inner());
        }
        Ok(include_bytes!("../../../../data/0-255.pbf").to_vec())
    }
}

#[tokio::test]
async fn text_and_sprite_loaded_through_the_asset_cache_are_drawn() {
    let server = Server::default();
    let client = SourceClient::new(HttpSourceClient::new(server.clone()));
    let mut style = style(0., "ground");
    style.glyphs = Some("https://fonts.invalid/{fontstack}/{range}.pbf".into());
    style.sprite = Some(serde_json::json!("https://sprites.invalid/sprite"));
    let tile = geozero::mvt::Tile {
        layers: vec![geozero::mvt::tile::Layer {
            name: "places".into(),
            version: 2,
            extent: Some(4096),
            features: vec![geozero::mvt::tile::Feature {
                r#type: Some(1),
                geometry: vec![9, 4096, 4096],
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
    .encode_to_vec();
    let atlas = load_symbol_assets(&client, &style, &tile, 12.)
        .await
        .expect("assets load");
    assert!(atlas.icons.contains_key("marker"));
    let fetches = server.urls.lock().expect("urls").len();
    let again = load_symbol_assets(&client, &style, &tile, 12.)
        .await
        .expect("assets load again");
    assert_eq!(
        server.urls.lock().expect("urls").len(),
        fetches,
        "a second tile reuses the cached glyph range and sprite sheet"
    );
    assert_eq!(again.glyphs.len(), atlas.glyphs.len());

    let pixels = render_with_atlas(0., "ground", 12., 4, atlas).await;
    let (red, text) = colored_bounds(&pixels, 0);
    let (green, icon) = colored_bounds(&pixels, 1);
    assert!(red > 70, "loaded glyphs are not drawn: {red} {text:?}");
    assert!(green > 100, "loaded sprite is not drawn: {green} {icon:?}");
}
