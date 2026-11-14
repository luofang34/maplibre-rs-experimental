//! End-to-end symbol asset loader tests for `Style::font_faces` (`WOFF2`), `harfrust` shaping,
//! and `localIdeographFontFamily` skipping remote `.pbf` requests for CJK (including SIP).

use std::sync::{Arc, Mutex};

use super::*;
use crate::{
    font::tests::synth_font::{build_synthetic_sfnt, encode_woff2_transformed},
    io::source_client::{HttpClient, HttpSourceClient, SourceClient, SourceFetchError},
};

#[derive(Clone)]
struct FontFaceServer {
    urls: Arc<Mutex<Vec<String>>>,
    woff2: Vec<u8>,
}

#[cfg_attr(not(feature = "thread-safe-futures"), async_trait::async_trait(?Send))]
#[cfg_attr(feature = "thread-safe-futures", async_trait::async_trait)]
impl HttpClient for FontFaceServer {
    async fn fetch(&self, url: &str) -> Result<Vec<u8>, SourceFetchError> {
        self.urls.lock().expect("urls").push(url.to_owned());
        if url == "https://fonts.invalid/synth-shaper.woff2" {
            return Ok(self.woff2.clone());
        }
        if url.ends_with("0-255.pbf") {
            return Ok(include_bytes!("../../../../../../data/0-255.pbf").to_vec());
        }
        Err(SourceFetchError::not_found(url))
    }
}

fn single_point_tile(layer_name: &str) -> Vec<u8> {
    geozero::mvt::Tile {
        layers: vec![geozero::mvt::tile::Layer {
            version: 2,
            name: layer_name.into(),
            features: vec![geozero::mvt::tile::Feature {
                r#type: Some(1),
                geometry: vec![9, 4096, 4096],
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
    .encode_to_vec()
}

#[tokio::test]
async fn font_faces_woff2_loads_and_shapes_ligatures_and_indic_without_pbf_requests() {
    let sfnt = build_synthetic_sfnt().expect("synth sfnt");
    let woff2 = encode_woff2_transformed(&sfnt).expect("synth woff2");
    let server = FontFaceServer {
        urls: Arc::new(Mutex::new(Vec::new())),
        woff2,
    };
    let client = SourceClient::new(HttpSourceClient::new(server.clone()));
    let style: Style = serde_json::from_value(serde_json::json!({
        "version": 8,
        "sources": {},
        "glyphs": "https://fonts.invalid/{fontstack}/{range}.pbf",
        "font-faces": [
            {
                "font-family": "Wave5Woff2Face",
                "url": "https://fonts.invalid/synth-shaper.woff2",
                "font-weight": 400
            }
        ],
        "layers": [
            {
                "id": "ligature",
                "type": "symbol",
                "source": "map",
                "source-layer": "places",
                "layout": {
                    "text-field": "fi",
                    "text-font": ["Wave5Woff2Face"]
                }
            },
            {
                "id": "indic",
                "type": "symbol",
                "source": "map",
                "source-layer": "places",
                "layout": {
                    "text-field": "\u{0915}\u{094D}\u{0937}",
                    "text-font": ["Wave5Woff2Face"]
                }
            }
        ]
    }))
    .expect("style");

    let tile = single_point_tile("places");
    let atlas = load_symbol_assets(
        &client,
        SymbolAssetConfig::of(&style),
        &style.layers,
        &tile,
        10.0,
    )
    .await
    .expect("atlas");

    let urls = server.urls.lock().expect("urls").clone();
    assert_eq!(
        urls,
        vec!["https://fonts.invalid/synth-shaper.woff2".to_string()],
        "WOFF2 font-face supplies glyphs directly without requesting .pbf ranges"
    );

    let runs = atlas
        .shaped_runs
        .get("Wave5Woff2Face")
        .expect("shaped runs for Wave5Woff2Face");
    let fi_run = runs.get("fi").expect("fi shaped run");
    assert_eq!(fi_run.len(), 1);
    assert_eq!(fi_run[0].glyph_id, 6);
    assert!(atlas.face_glyphs.contains_key(&fi_run[0].atlas_key()));

    let ksha_run = runs
        .get("\u{0915}\u{094D}\u{0937}")
        .expect("Devanagari ksha shaped run");
    assert_eq!(ksha_run.len(), 1);
    assert_eq!(ksha_run[0].glyph_id, 10);
    assert!(atlas.face_glyphs.contains_key(&ksha_run[0].atlas_key()));
}

#[tokio::test]
async fn local_ideograph_font_family_skips_http_pbf_requests_for_bmp_and_sip_cjk() {
    let sfnt = build_synthetic_sfnt().expect("synth sfnt");
    crate::font::register_font_bytes(Some("Wave5SipCjk"), sfnt).expect("register sip cjk");

    let server = FontFaceServer {
        urls: Arc::new(Mutex::new(Vec::new())),
        woff2: Vec::new(),
    };
    let client = SourceClient::new(HttpSourceClient::new(server.clone()));
    let style: Style = serde_json::from_value(serde_json::json!({
        "version": 8,
        "sources": {},
        "localIdeographFontFamily": "Wave5SipCjk",
        "glyphs": "https://fonts.invalid/{fontstack}/{range}.pbf",
        "layers": [
            {
                "id": "cjk",
                "type": "symbol",
                "source": "map",
                "source-layer": "places",
                "layout": {
                    "text-field": "A\u{2A6A5}",
                    "text-font": ["Open Sans Regular"]
                }
            }
        ]
    }))
    .expect("style");

    let tile = single_point_tile("places");
    let atlas = load_symbol_assets(
        &client,
        SymbolAssetConfig::of(&style),
        &style.layers,
        &tile,
        10.0,
    )
    .await
    .expect("atlas");

    let urls = server.urls.lock().expect("urls").clone();
    assert_eq!(
        urls,
        vec!["https://fonts.invalid/Open%20Sans%20Regular/0-255.pbf".to_string()],
        "only Latin range 0-255 is requested over HTTP; Plane-2 SIP U+2A6A5 is rasterised locally"
    );
    let stack_glyphs = atlas
        .glyphs
        .get("Open Sans Regular")
        .expect("Open Sans Regular glyphs");
    assert!(stack_glyphs.contains_key(&(u32::from('A'))));
    assert!(
        stack_glyphs.contains_key(&0x2A6A5),
        "SIP ideograph U+2A6A5 packed locally into atlas"
    );
}
