//! Image sources reach the renderer as raster tiles resampled from the picture.

use std::collections::HashMap;

use image::RgbaImage;
use maplibre::style::{
    source::{Source, VectorSource},
    Style,
};

/// A loaded picture and the geographic corners it is stretched over.
pub(super) struct PlacedImage {
    pub image: RgbaImage,
    pub coordinates: [[f64; 2]; 4],
}

const IMAGE_URL_PREFIX: &str = "local://image/";

/// Replaces every image source by a raster source of the same name, and returns the pictures.
pub(super) fn lower(style: &mut Style) -> Result<HashMap<String, PlacedImage>, String> {
    let mut images = HashMap::new();
    for (name, source) in &mut style.sources {
        let Source::Image(placed) = source else {
            continue;
        };
        let relative = placed
            .url
            .strip_prefix(IMAGE_URL_PREFIX)
            .ok_or_else(|| format!("Unsupported image URL in render harness: {}", placed.url))?;
        let path = std::path::Path::new("render-tests/src/assets/image").join(relative);
        let image = image::open(&path)
            .map_err(|error| format!("Cannot read image {}: {error}", path.display()))?
            .to_rgba8();
        images.insert(
            name.clone(),
            PlacedImage {
                image,
                coordinates: placed.coordinates,
            },
        );
        *source = Source::Raster(VectorSource {
            attribution: None,
            bounds: None,
            maxzoom: None,
            minzoom: None,
            scheme: None,
            tiles: Some(vec!["local://image-source".to_owned()]),
            tile_size: Some(maplibre::raster::image_source::IMAGE_TILE_SIZE),
            url: None,
        });
    }
    Ok(images)
}
