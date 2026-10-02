//! Resamples an `image` source into the tiles that cover it.
//!
//! Resampling the picture into raster tiles lets terrain, globe and masks treat it like any other
//! raster source. Corners that form a rectangle in Mercator give GL JS's picture exactly. A quad
//! that is not one is split into two triangles, which matches neither of GL JS's warps: its
//! `perspective` warp, which foreshortens the picture as a view of a plane, or its `flat` warp,
//! which interpolates it bilinearly between the corners.

use image::RgbaImage;

use crate::coords::WorldTileCoords;

mod warp;

/// Pixels per side of a resampled tile.
pub const IMAGE_TILE_SIZE: u32 = 512;

fn mercator([longitude, latitude]: [f64; 2]) -> [f64; 2] {
    let x = (longitude + 180.0) / 360.0;
    let y = 0.5
        - (std::f64::consts::FRAC_PI_4 + latitude.to_radians() / 2.0)
            .tan()
            .ln()
            / std::f64::consts::TAU;
    [x, y]
}

fn sample(image: &RgbaImage, u: f64, v: f64) -> [f32; 4] {
    let (width, height) = image.dimensions();
    let x = (u * f64::from(width) - 0.5).clamp(0.0, f64::from(width - 1));
    let y = (v * f64::from(height) - 0.5).clamp(0.0, f64::from(height - 1));
    let (x0, y0) = (x.floor() as u32, y.floor() as u32);
    let (x1, y1) = ((x0 + 1).min(width - 1), (y0 + 1).min(height - 1));
    let (fx, fy) = ((x - f64::from(x0)) as f32, (y - f64::from(y0)) as f32);
    let mut premultiplied = [0.0_f32; 4];
    for ((px, py), weight) in [
        ((x0, y0), (1.0 - fx) * (1.0 - fy)),
        ((x1, y0), fx * (1.0 - fy)),
        ((x0, y1), (1.0 - fx) * fy),
        ((x1, y1), fx * fy),
    ] {
        let [r, g, b, a] = image
            .get_pixel(px, py)
            .0
            .map(|channel| f32::from(channel) / 255.0);
        for (out, value) in premultiplied.iter_mut().zip([r * a, g * a, b * a, a]) {
            *out += value * weight;
        }
    }
    premultiplied
}

/// Draws one triangle of the warped picture into the tile: each pixel whose centre it covers
/// takes the picture at the texture coordinates interpolated across it and divided by the
/// interpolated weight, as GPU interpolation of GL JS's `v_pos0` gives them. A pixel an earlier
/// triangle or world copy already drew keeps its colour.
fn fill(
    tile: &mut RgbaImage,
    drawn: &mut [bool],
    image: &RgbaImage,
    [(a, ta), (b, tb), (c, tc)]: [([f64; 2], [f64; 3]); 3],
) {
    let determinant = (b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1]);
    if determinant == 0.0 || !determinant.is_finite() {
        return;
    }
    let size = f64::from(IMAGE_TILE_SIZE);
    let lo = |values: [f64; 3]| values.into_iter().fold(f64::MAX, f64::min).floor().max(0.0);
    let hi = |values: [f64; 3]| values.into_iter().fold(f64::MIN, f64::max).ceil().min(size);
    let (x0, x1) = (lo([a[0], b[0], c[0]]), hi([a[0], b[0], c[0]]));
    let (y0, y1) = (lo([a[1], b[1], c[1]]), hi([a[1], b[1], c[1]]));
    for py in y0 as u32..y1 as u32 {
        for px in x0 as u32..x1 as u32 {
            let index = (py * IMAGE_TILE_SIZE + px) as usize;
            if drawn[index] {
                continue;
            }
            let (x, y) = (f64::from(px) + 0.5, f64::from(py) + 0.5);
            let l1 = ((x - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (y - a[1])) / determinant;
            let l2 = ((b[0] - a[0]) * (y - a[1]) - (x - a[0]) * (b[1] - a[1])) / determinant;
            let l0 = 1.0 - l1 - l2;
            if l0 < 0.0 || l1 < 0.0 || l2 < 0.0 {
                continue;
            }
            let texture = [0, 1, 2].map(|i| l0 * ta[i] + l1 * tb[i] + l2 * tc[i]);
            let [r, g, blue, alpha] =
                sample(image, texture[0] / texture[2], texture[1] / texture[2]);
            if alpha > 0.0 {
                tile.put_pixel(
                    px,
                    py,
                    image::Rgba(
                        [r / alpha, g / alpha, blue / alpha, alpha]
                            .map(|c| (c * 255.0).round() as u8),
                    ),
                );
                drawn[index] = true;
            }
        }
    }
}

/// Copies of the world an image can wrap onto before the rest are ignored.
const MAX_WORLD_SHIFTS: usize = 4;

/// The part of the image inside `tile`, or `None` when the image does not touch it.
pub fn render_tile(
    image: &RgbaImage,
    coordinates: [[f64; 2]; 4],
    tile: WorldTileCoords,
) -> Option<RgbaImage> {
    let tile_count = 2_f64.powi(i32::from(u8::from(tile.z)));
    let corners = coordinates.map(mercator);
    let (west, east) = corners.iter().fold((f64::MAX, f64::MIN), |(lo, hi), c| {
        (lo.min(c[0]), hi.max(c[0]))
    });
    let (north, south) = corners.iter().fold((f64::MAX, f64::MIN), |(lo, hi), c| {
        (lo.min(c[1]), hi.max(c[1]))
    });
    let (left, top) = (
        f64::from(tile.x) / tile_count,
        f64::from(tile.y) / tile_count,
    );
    let size = 1.0 / tile_count;
    // The image may reach past the antimeridian, so the tile also shows it shifted by whole worlds.
    let shifts: Vec<f64> = ((west - left - size).floor() as i64..=(east - left).ceil() as i64)
        .map(|shift| shift as f64)
        .filter(|shift| {
            let (lo, hi) = (left + shift, left + shift + size);
            !(east < lo || west > hi)
        })
        .take(MAX_WORLD_SHIFTS)
        .collect();
    if shifts.is_empty() || south < top || north > top + size {
        return None;
    }
    let mut tile_image = RgbaImage::new(IMAGE_TILE_SIZE, IMAGE_TILE_SIZE);
    let mut drawn = vec![false; (IMAGE_TILE_SIZE * IMAGE_TILE_SIZE) as usize];
    let triangles = warp::triangles(corners);
    // Mercator units to tile pixels, for the copy of the world `shift` whole worlds east.
    let pixels = f64::from(IMAGE_TILE_SIZE) / size;
    for shift in &shifts {
        let to_tile = |[x, y]: [f64; 2]| [(x - left - shift) * pixels, (y - top) * pixels];
        for triangle in &triangles {
            let corners = triangle.map(|(position, texture)| (to_tile(position), texture));
            fill(&mut tile_image, &mut drawn, image, corners);
        }
    }
    Some(tile_image)
}

/// Why an image source's picture cannot be drawn.
#[derive(Debug, thiserror::Error)]
pub enum PictureError {
    /// The bytes are not a picture the decoder reads.
    #[error("the picture cannot be decoded")]
    Decode(#[source] image::ImageError),
    /// The picture's embedded colour profile cannot be converted to sRGB.
    #[error("the picture's colour profile cannot be converted to sRGB")]
    Profile(#[source] moxcms::CmsError),
}

/// Decodes an image source's picture into sRGB, converting it from the colour profile it embeds,
/// as GL JS shows it once the browser has decoded it.
pub fn decode(bytes: &[u8]) -> Result<RgbaImage, PictureError> {
    use image::ImageDecoder;
    let mut decoder = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| PictureError::Decode(image::ImageError::IoError(error)))?
        .into_decoder()
        .map_err(PictureError::Decode)?;
    let profile = decoder.icc_profile().map_err(PictureError::Decode)?;
    let picture = image::DynamicImage::from_decoder(decoder)
        .map_err(PictureError::Decode)?
        .to_rgba8();
    // A profile the converter does not understand leaves the stored values as they are.
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
        .map_err(PictureError::Profile)?;
    let (width, height) = picture.dimensions();
    let mut converted = vec![0_u8; picture.as_raw().len()];
    transform
        .transform(picture.as_raw(), &mut converted)
        .map_err(PictureError::Profile)?;
    Ok(RgbaImage::from_raw(width, height, converted).unwrap_or(picture))
}

/// Makes an image source's tiles again after its picture or corners changed. A tile the
/// picture still reaches keeps showing the old one until the new arrives; a tile it no longer
/// reaches drops it now.
pub(crate) fn reload(
    world: &mut crate::tcs::world::World,
    style: &crate::style::Style,
    name: &str,
) {
    use crate::{
        io::tile_retry::{self, RequestKind},
        raster::{RasterLayersDataComponent, RasterSourceId},
        render::eventually::Eventually,
        style::source::Source,
    };
    let Some(Source::Image(image)) = style.sources.get(name) else {
        return;
    };
    let source = RasterSourceId::from(name);
    let coords: Vec<WorldTileCoords> = world.tiles.tiles.values().map(|tile| tile.coords).collect();
    for coords in coords {
        if render_tile_bounds_reach(image.coordinates, coords) {
            continue;
        }
        if let Some(component) = world
            .tiles
            .query_mut::<&mut RasterLayersDataComponent>(coords)
        {
            component.layers.retain(|layer| layer.source() != &source);
        }
        if let Some(Eventually::Initialized(raster)) = world
            .resources
            .get_mut::<Eventually<crate::raster::resource::RasterResources>>()
        {
            raster.remove_source_texture(&source, coords);
        }
    }
    tile_retry::refresh(world, RequestKind::Raster);
}

/// Whether the picture stretched over `coordinates` can reach the tile: its corners' bounding box
/// overlaps the tile.
fn render_tile_bounds_reach(coordinates: [[f64; 2]; 4], tile: WorldTileCoords) -> bool {
    let tile_count = 2_f64.powi(i32::from(u8::from(tile.z)));
    let corners = coordinates.map(mercator);
    let fold = |axis: usize| {
        corners.iter().fold((f64::MAX, f64::MIN), |(lo, hi), c| {
            (lo.min(c[axis]), hi.max(c[axis]))
        })
    };
    let ((west, east), (north, south)) = (fold(0), fold(1));
    let (left, top) = (
        f64::from(tile.x) / tile_count,
        f64::from(tile.y) / tile_count,
    );
    let size = 1.0 / tile_count;
    // A picture past the antimeridian shows in copies of the world, which this does not follow.
    let wraps = west < 0.0 || east > 1.0;
    wraps || !(east < left || west > left + size || south < top || north > top + size)
}

/// The tile `coords` of an image source, resampled from its picture, which is fetched and
/// decoded once through the client's shared asset cache. `Ok(None)` when the picture does not
/// reach the tile.
pub(crate) async fn load_tile<HC: crate::io::source_client::HttpClient>(
    client: &crate::io::source_client::SourceClient<HC>,
    source: &crate::style::source::ImageSource,
    coords: WorldTileCoords,
) -> Result<Option<RgbaImage>, crate::sdf::assets::AssetFailure> {
    use crate::sdf::assets::AssetFailure;
    let url = source.url.as_str();
    let picture = client
        .assets()
        .load(
            format!("image-source:{url}#{}", source.generation),
            || async {
                let bytes = crate::sdf::assets::fetch(client, url, "image source").await?;
                let picture = decode(&bytes).map_err(|error| {
                    tracing::warn!(%url, %error, "invalid image source picture");
                    AssetFailure::Terminal(format!("invalid image {url}: {error}"))
                })?;
                let bytes = picture.as_raw().len();
                Ok((picture, bytes))
            },
        )
        .await?;
    Ok(render_tile(&picture, source.coordinates, coords))
}

#[cfg(test)]
mod tests;
