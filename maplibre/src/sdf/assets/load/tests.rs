#![allow(clippy::expect_used, clippy::panic)]
use super::*;
use crate::io::source_client::{HttpSourceClient, SourceFetchError};
use prost::Message as _;
use std::sync::Mutex;

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
    let glyphs = crate::sdf::text::sdf_glyphs::Glyphs {
        stacks: vec![crate::sdf::text::sdf_glyphs::Fontstack {
            name: "Font A".into(),
            range: "256-511".into(),
            glyphs: vec![crate::sdf::text::sdf_glyphs::Glyph {
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
    let atlas = load_symbol_assets(&client, &style, &tile, 12.).await;
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
    assert_eq!(&atlas.pixels[offset..offset + 4], &[0, 255, 0, 191]);
}
