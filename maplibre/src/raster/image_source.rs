//! Resamples an `image` source into the tiles that cover it.
//!
//! GL JS draws the image as a quad of two triangles; resampling it into raster tiles gives the same
//! picture and lets terrain, globe and masks treat it like any other raster source.

use image::RgbaImage;

use crate::coords::WorldTileCoords;

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

/// The image coordinates of a point inside the quad, or `None` outside it.
///
/// GL JS draws the quad as two triangles split along the diagonal from the top right to the
/// bottom left corner and interpolates linearly inside each, so a quad that is not a
/// parallelogram bends along that diagonal instead of mapping projectively.
fn unit_coordinates(corners: &[[f64; 2]; 4], x: f64, y: f64) -> Option<[f64; 2]> {
    let [top_left, top_right, bottom_right, bottom_left] = *corners;
    let triangles = [
        (
            [top_left, top_right, bottom_left],
            [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
        ),
        (
            [bottom_right, top_right, bottom_left],
            [[1.0, 1.0], [1.0, 0.0], [0.0, 1.0]],
        ),
    ];
    triangles.iter().find_map(|(points, uv)| {
        let [a, b, c] = *points;
        let determinant = (b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1]);
        if determinant == 0.0 {
            return None;
        }
        let l1 = ((x - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (y - a[1])) / determinant;
        let l2 = ((b[0] - a[0]) * (y - a[1]) - (x - a[0]) * (b[1] - a[1])) / determinant;
        let l0 = 1.0 - l1 - l2;
        if l0 < 0.0 || l1 < 0.0 || l2 < 0.0 {
            return None;
        }
        Some([
            l0 * uv[0][0] + l1 * uv[1][0] + l2 * uv[2][0],
            l0 * uv[0][1] + l1 * uv[1][1] + l2 * uv[2][1],
        ])
    })
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
    let step = size / f64::from(IMAGE_TILE_SIZE);
    for (px, py, pixel) in tile_image.enumerate_pixels_mut() {
        let y = top + (f64::from(py) + 0.5) * step;
        for shift in &shifts {
            let x = left + shift + (f64::from(px) + 0.5) * step;
            let Some([u, v]) = unit_coordinates(&corners, x, y) else {
                continue;
            };
            let [r, g, b, a] = sample(image, u, v);
            if a > 0.0 {
                *pixel = image::Rgba([r / a, g / a, b / a, a].map(|c| (c * 255.0).round() as u8));
                break;
            }
        }
    }
    Some(tile_image)
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
        .load(format!("image-source:{url}"), || async {
            let bytes = crate::sdf::assets::fetch(client, url, "image source").await?;
            let picture = image::load_from_memory(&bytes)
                .map_err(|error| {
                    tracing::warn!(%url, %error, "invalid image source picture");
                    AssetFailure::Terminal(format!("invalid image {url}: {error}"))
                })?
                .to_rgba8();
            let bytes = picture.as_raw().len();
            Ok((picture, bytes))
        })
        .await?;
    Ok(render_tile(&picture, source.coordinates, coords))
}

#[cfg(test)]
mod tests;
