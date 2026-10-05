//! The names a provider serves are packed once their answers are known, after whatever the
//! sprite and the style supply.

use super::{
    names::{roads, style},
    *,
};
use crate::sdf::assets::{
    ImageProviders, ImageRequest, ImageResolution, ProvideFuture, ProvidedImage, StyleImageProvider,
};

/// Draws a 4 x 6 shield whose body is the bottom 4 x 4, for one route only.
struct Shields;

impl StyleImageProvider for Shields {
    fn generation(&self) -> String {
        "test".into()
    }

    fn provide(&self, request: ImageRequest) -> ProvideFuture<'_> {
        Box::pin(async move {
            if request.id != "US:I=287" {
                return Ok(ImageResolution::Absent);
            }
            Ok(ImageResolution::Image(ProvidedImage {
                image: StyleImage {
                    width: 4,
                    height: 6,
                    data: vec![255; 4 * 6 * 4],
                    pixel_ratio: 2.0,
                    sdf: false,
                },
                anchor: Some([2.0, 4.0]),
            }))
        })
    }
}

#[tokio::test]
async fn provided_images_are_packed_once_known_and_listed_until_then() {
    let providers = ImageProviders::default();
    providers.register("shield", Arc::new(Shields));
    let server = Server {
        urls: Default::default(),
        png: Vec::new(),
        failing_range: Default::default(),
    };
    let client =
        SourceClient::new(HttpSourceClient::new(server)).with_image_providers(providers.clone());
    let style = style(serde_json::json!([
        "coalesce",
        [
            "image",
            ["concat", "shield:", ["get", "network"], "=", ["get", "ref"]]
        ],
        ["image", "generic-shield"]
    ]));
    let config = SymbolAssetConfig {
        pixel_ratio: 2.0,
        ..SymbolAssetConfig::of(&style)
    };
    let tile = roads();
    let load = || load_symbol_assets_awaiting(&client, config, &style.layers, &tile, 14.0);
    let first = load().await.expect("assets");
    assert_eq!(first.provided, ["shield:US:I=287", "shield:US:NJ:CR=609"]);
    assert_eq!(first.awaiting, first.provided, "nothing is known yet");
    assert!(first.atlas.icons.is_empty());
    for name in &first.awaiting {
        providers.resolve(name, 2.0).await.expect("an answer");
    }
    let second = load().await.expect("assets");
    assert!(second.awaiting.is_empty());
    let shield = &second.atlas.icons["shield:US:I=287"];
    assert_eq!(shield.kind, 1, "a colour image, not an SDF");
    assert_eq!(
        shield.metrics,
        [0.0, 1.0, 0.0, 2.0],
        "the body's centre is one image pixel below the picture's"
    );
    assert!(
        !second.atlas.icons.contains_key("shield:US:NJ:CR=609"),
        "an absent shield leaves the fallback to the style"
    );
    // A provided image at another pixel ratio is another image.
    let other = SymbolAssetConfig {
        pixel_ratio: 3.0,
        ..config
    };
    let third = load_symbol_assets_awaiting(&client, other, &style.layers, &tile, 14.0)
        .await
        .expect("assets");
    assert_eq!(third.awaiting.len(), 2);
}

#[tokio::test]
async fn an_image_the_style_supplies_wins_over_a_provider() {
    let providers = ImageProviders::default();
    providers.register("shield", Arc::new(Shields));
    let server = Server {
        urls: Default::default(),
        png: Vec::new(),
        failing_range: Default::default(),
    };
    let client = SourceClient::new(HttpSourceClient::new(server)).with_image_providers(providers);
    let mut style = style(serde_json::json!([
        "concat",
        "shield:",
        ["get", "network"],
        "=",
        ["get", "ref"]
    ]));
    style.images.insert(
        "shield:US:I=287".into(),
        StyleImage {
            width: 1,
            height: 1,
            data: vec![0, 0, 255, 255],
            pixel_ratio: 1.0,
            sdf: false,
        },
    );
    let loaded = load_symbol_assets_awaiting(
        &client,
        SymbolAssetConfig::of(&style),
        &style.layers,
        &roads(),
        14.0,
    )
    .await
    .expect("assets");
    assert_eq!(loaded.provided, ["shield:US:NJ:CR=609"]);
    assert_eq!(loaded.atlas.icons["shield:US:I=287"].rect[2], 1);
}
