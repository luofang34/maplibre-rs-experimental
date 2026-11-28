//! Terrarium DEM tiles drawn from a height over the world, served tile by tile so that every
//! zoom of the DEM shows the same ground.

use std::sync::{Arc, Mutex};

use super::AssetServer;

enum DemEntry {
    Pending { prefix: String, rgba: Vec<u8> },
    Encoded { prefix: String, png: Vec<u8> },
}

/// Lazy store of raw `256x256` RGBA8 Terrarium tiles encoded to PNG on first HTTP fetch.
#[derive(Clone, Default)]
pub(in crate::headless::map::suites::symbol_worker) struct DemStore(Arc<Mutex<Vec<DemEntry>>>);

impl DemStore {
    fn serve(&self, prefix: String, rgba: Vec<u8>) {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(DemEntry::Pending { prefix, rgba });
    }

    pub(in crate::headless::map::suites::symbol_worker) fn fetch(
        &self,
        url: &str,
    ) -> Option<Vec<u8>> {
        let mut entries = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let entry = entries.iter_mut().find(|entry| match entry {
            DemEntry::Pending { prefix, .. } | DemEntry::Encoded { prefix, .. } => {
                url.starts_with(prefix.as_str())
            }
        })?;
        if let DemEntry::Pending { prefix, rgba } = entry {
            let png = encode_png_256(rgba);
            *entry = DemEntry::Encoded {
                prefix: std::mem::take(prefix),
                png,
            };
        }
        match entry {
            DemEntry::Encoded { png, .. } => Some(png.clone()),
            DemEntry::Pending { .. } => None,
        }
    }
}

fn encode_png_256(rgba: &[u8]) -> Vec<u8> {
    use image::ImageEncoder as _;
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new_with_quality(
        &mut png,
        image::codecs::png::CompressionType::Fast,
        image::codecs::png::FilterType::NoFilter,
    )
    .write_image(rgba, 256, 256, image::ExtendedColorType::Rgba8)
    .expect("PNG");
    png
}

/// Raw `256x256` RGBA8 Terrarium pixels for tile `z/x/y`.
fn dem_rgba([z, x, y]: [u32; 3], metres: &dyn Fn(f64, f64) -> f64) -> Vec<u8> {
    let tiles = f64::from(1_u32 << z);
    let mut rgba = Vec::with_capacity(256 * 256 * 4);
    for row in 0..256_u32 {
        let world_y = (f64::from(y) + (f64::from(row) + 0.5) / 256.0) / tiles;
        for column in 0..256_u32 {
            let world_x = (f64::from(x) + (f64::from(column) + 0.5) / 256.0) / tiles;
            // Terrarium stores the height plus 32768 m as red * 256 + green + blue / 256.
            let value = (metres(world_x, world_y) + 32768.0).clamp(0.0, 65535.0);
            let red = (value / 256.0).floor();
            let green = (value - red * 256.0).floor();
            let blue = ((value - red * 256.0 - green) * 256.0).floor();
            rgba.extend_from_slice(&[red as u8, green as u8, blue as u8, 255]);
        }
    }
    rgba
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
                server.dem.serve(
                    format!("https://dem.test/{z}/{x}/{y}."),
                    dem_rgba([z, x, y], metres),
                );
            }
        }
    }
    let tiles = f64::from(1_u32 << max_zoom);
    let corner = metres(f64::from(west) / tiles, f64::from(north) / tiles);
    server.dem.serve(
        "https://dem.test/".to_owned(),
        dem_rgba([0, 0, 0], &|_, _| corner),
    );
}
