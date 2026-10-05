//! Terrarium DEM tiles drawn from a height over the world, served tile by tile so that every
//! zoom of the DEM shows the same ground.

use super::AssetServer;

/// The terrarium tile `z/x/y`, each pixel the height `metres` gives at its center, which takes
/// the world's mercator `x` and `y` in 0..1.
pub(super) fn dem_tile([z, x, y]: [u32; 3], metres: &dyn Fn(f64, f64) -> f64) -> Vec<u8> {
    let tiles = f64::from(1_u32 << z);
    let tile = image::RgbaImage::from_fn(256, 256, |column, row| {
        let world_x = (f64::from(x) + (f64::from(column) + 0.5) / 256.0) / tiles;
        let world_y = (f64::from(y) + (f64::from(row) + 0.5) / 256.0) / tiles;
        // Terrarium stores the height plus 32768 m as red * 256 + green + blue / 256.
        let value = (metres(world_x, world_y) + 32768.0).clamp(0.0, 65535.0);
        let red = (value / 256.0).floor();
        let green = (value - red * 256.0).floor();
        let blue = ((value - red * 256.0 - green) * 256.0).floor();
        image::Rgba([red as u8, green as u8, blue as u8, 255])
    });
    let mut png = std::io::Cursor::new(Vec::new());
    tile.write_to(&mut png, image::ImageFormat::Png)
        .expect("PNG");
    png.into_inner()
}

/// Serves `metres` as DEM tiles under `https://dem.test/` for every zoom up to `max_zoom`, over
/// the tiles of `max_zoom` from `west` to `east` and `north` to `south` inclusive and their
/// parents; tiles elsewhere hold the height at the region's north-west corner.
pub(super) fn serve_dem(
    server: &AssetServer,
    max_zoom: u32,
    [west, north, east, south]: [u32; 4],
    metres: &dyn Fn(f64, f64) -> f64,
) {
    for z in 0..=max_zoom {
        let shift = max_zoom - z;
        for y in north >> shift..=south >> shift {
            for x in west >> shift..=east >> shift {
                server.serve(
                    &format!("https://dem.test/{z}/{x}/{y}."),
                    dem_tile([z, x, y], metres),
                );
            }
        }
    }
    let tiles = f64::from(1_u32 << max_zoom);
    let corner = metres(f64::from(west) / tiles, f64::from(north) / tiles);
    server.serve("https://dem.test/", dem_tile([0, 0, 0], &|_, _| corner));
}
