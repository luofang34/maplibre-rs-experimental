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
    let on_globe = style
        .projection
        .as_ref()
        .is_some_and(|specification| specification.projection_type.uses_globe_rendering(0.0));
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
        // GL JS on the globe shows a picture in its embedded colour profile converted to sRGB;
        // on the flat map it shows the stored values.
        let image = if on_globe {
            decode_srgb(&path)?
        } else {
            image::open(&path)
                .map_err(|error| format!("Cannot read image {}: {error}", path.display()))?
                .to_rgba8()
        };
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

/// The picture at `path` in sRGB: a picture that embeds a colour profile is converted from it.
fn decode_srgb(path: &std::path::Path) -> Result<RgbaImage, String> {
    use image::ImageDecoder;
    let failure =
        |error: &dyn std::fmt::Display| format!("Cannot read image {}: {error}", path.display());
    let mut decoder = image::ImageReader::open(path)
        .map_err(|error| failure(&error))?
        .with_guessed_format()
        .map_err(|error| failure(&error))?
        .into_decoder()
        .map_err(|error| failure(&error))?;
    let profile = decoder.icc_profile().map_err(|error| failure(&error))?;
    let picture = image::DynamicImage::from_decoder(decoder)
        .map_err(|error| failure(&error))?
        .to_rgba8();
    let Some(Ok(source)) = profile.map(|bytes| moxcms::ColorProfile::new_from_slice(&bytes)) else {
        return Ok(picture);
    };
    let transform = source
        .create_transform_8bit(
            moxcms::Layout::Rgba,
            &moxcms::ColorProfile::new_srgb(),
            moxcms::Layout::Rgba,
            moxcms::TransformOptions::default(),
        )
        .map_err(|error| failure(&error))?;
    let (width, height) = picture.dimensions();
    let mut converted = vec![0_u8; picture.as_raw().len()];
    transform
        .transform(picture.as_raw(), &mut converted)
        .map_err(|error| failure(&error))?;
    RgbaImage::from_raw(width, height, converted)
        .ok_or_else(|| failure(&"the converted picture has the wrong size"))
}
