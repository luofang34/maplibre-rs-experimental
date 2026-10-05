//! Shields drawn from the Americana pack. These need a roadshield resource pack, which the
//! crate does not copy: set `ROADSHIELD_PACK` to its directory and run with `--ignored`.

#[path = "../examples/support/scene.rs"]
mod scene;

use std::path::PathBuf;

use maplibre::sdf::assets::{ImageRequest, ImageResolution, ProvidedImage, StyleImageProvider};
use maplibre_roadshield::{RoadShieldProvider, RouteRequest};
use roadshield::Rendering;

#[cfg(test)]
fn pack() -> PathBuf {
    PathBuf::from(std::env::var("ROADSHIELD_PACK").expect("ROADSHIELD_PACK names a pack"))
}

#[cfg(test)]
fn provider() -> RoadShieldProvider {
    scene::provider(&pack()).expect("the pack loads")
}

#[cfg(test)]
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime")
}

#[cfg(test)]
fn ask(provider: &RoadShieldProvider, id: &str, pixel_ratio: f32) -> ImageResolution {
    let request = ImageRequest {
        name: format!("roadshield:{id}"),
        id: id.to_owned(),
        pixel_ratio,
    };
    runtime()
        .block_on(provider.provide(request))
        .unwrap_or_else(|error| panic!("{id}: {error}"))
}

#[cfg(test)]
fn image(provider: &RoadShieldProvider, id: &str, pixel_ratio: f32) -> ProvidedImage {
    match ask(provider, id, pixel_ratio) {
        ImageResolution::Image(image) => image,
        ImageResolution::Absent => panic!("{id} has no shield"),
    }
}

#[test]
#[ignore = "needs ROADSHIELD_PACK, the path of a roadshield resource pack"]
fn routes_from_tile_attributes_get_their_networks_shields() {
    let provider = provider();
    for id in [
        "US:I=287",
        "US:NJ:CR=609",
        "US:US=1",
        "US:US:Truck:Bypass=1",
    ] {
        let Ok(Rendering::Symbol(symbol)) = provider.render_symbol(id, 1.0) else {
            panic!("{id} draws a shield");
        };
        assert!(!symbol.rule.fallback, "{id} has a rule of its own");
    }
    // A state route whose data names no state, and a country without rules, get the generic
    // shield, recorded as such; nothing is guessed.
    for id in ["=609", "XX:somewhere=12"] {
        let Ok(Rendering::Symbol(symbol)) = provider.render_symbol(id, 1.0) else {
            panic!("{id} draws the generic shield");
        };
        assert!(symbol.rule.fallback, "{id}");
    }
    // A ref too long for any shield leaves the style's fallback.
    assert_eq!(
        ask(&provider, "US:I=12345678", 1.0),
        ImageResolution::Absent
    );
    assert!(RouteRequest::parse("US:I").is_err());
}

#[test]
#[ignore = "needs ROADSHIELD_PACK, the path of a roadshield resource pack"]
fn a_shield_is_straight_alpha_at_the_displays_density_anchored_on_its_body() {
    let provider = provider();
    let one = image(&provider, "US:I=287", 1.0);
    let two = image(&provider, "US:I=287", 2.0);
    assert!(!one.image.sdf);
    assert_eq!(two.image.pixel_ratio, 2.0);
    for (dense, sparse) in [
        (two.image.width, one.image.width),
        (two.image.height, one.image.height),
    ] {
        assert!(dense.abs_diff(sparse * 2) <= 2, "{dense} is twice {sparse}");
    }
    // Straight alpha: an anti-aliased edge keeps its colour while its alpha falls.
    assert!(
        two.image.data.chunks_exact(4).any(|pixel| pixel[3] > 0
            && pixel[3] < 200
            && pixel[..3].iter().any(|c| *c > pixel[3])),
        "edges are not premultiplied"
    );
    // A banner sits above the body, so the body's centre is below the picture's.
    let bannered = image(&provider, "US:US:Truck:Bypass=1", 2.0);
    let [_, y] = bannered.anchor.expect("an anchor");
    assert!(
        y > bannered.image.height as f32 / 2.0 + 2.0,
        "the body centre {y} is below the middle of {}",
        bannered.image.height
    );
}

/// Where each road of the scene crosses the image, in device pixels from the top.
const ROAD_ROWS: [u32; 3] = [51, 256, 461];

/// The commonest opaque colour of `image` that `wanted` accepts.
#[cfg(test)]
fn commonest(image: &ProvidedImage, wanted: impl Fn([u8; 3]) -> bool) -> [u8; 3] {
    let mut counts = std::collections::HashMap::new();
    for pixel in image.image.data.chunks_exact(4) {
        let colour = [pixel[0], pixel[1], pixel[2]];
        if pixel[3] == 255 && wanted(colour) {
            *counts.entry(colour).or_insert(0_usize) += 1;
        }
    }
    counts
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .map(|(colour, _)| colour)
        .expect("a colour of that kind")
}

#[test]
#[ignore = "needs ROADSHIELD_PACK, the path of a roadshield resource pack"]
fn a_map_draws_shields_made_from_its_tiles_route_attributes() {
    let provider = provider();
    // The interstate's red crown and the county pentagon's yellow tell the two blue shields
    // apart.
    let red = |[r, g, b]: [u8; 3]| r > 150 && g < 100 && b < 100;
    let yellow = |[r, g, b]: [u8; 3]| r > 150 && g > 150 && b < 100;
    let interstate = commonest(&image(&provider, "US:I=287", 2.0), red);
    let county = commonest(&image(&provider, "US:NJ:CR=609", 2.0), yellow);
    runtime().block_on(async {
        let mut map = scene::map(provider, 2.0).await.expect("map");
        let pixels = scene::settle(&mut map).await.expect("settled");
        let stats = map.image_providers().expect("registry").stats();
        assert_eq!(
            stats.images, 3,
            "the interstate, the county route and the generic state route: {stats:?}"
        );
        assert_eq!(stats.failed, 0, "{stats:?}");
        // Each road's own band of the image, so one route's shield cannot stand for another's.
        let band = |road: usize, colour: [u8; 3]| {
            let row = ROAD_ROWS[road];
            (row.saturating_sub(30)..(row + 30).min(scene::SIZE))
                .flat_map(|y| (0..scene::SIZE).map(move |x| (x, y)))
                .filter(|(x, y)| {
                    let index = ((y * scene::SIZE + x) * 4) as usize;
                    pixels[index..index + 3]
                        .iter()
                        .zip(colour)
                        .all(|(a, b)| a.abs_diff(b) < 12)
                })
                .count()
        };
        assert!(
            band(0, interstate) > 50,
            "the interstate's shield: {}",
            band(0, interstate)
        );
        assert!(
            band(1, county) > 50,
            "the county route's shield: {}",
            band(1, county)
        );
        assert!(
            band(0, county) < 5 && band(1, interstate) < 5,
            "each road its own shield"
        );
        assert!(
            band(2, county) < 20 && band(2, interstate) < 20,
            "the state route of unnamed state gets neither shield"
        );
        assert!(
            band(2, [0, 0, 0]) > 30,
            "but its number, on the generic shield"
        );
    });
}
