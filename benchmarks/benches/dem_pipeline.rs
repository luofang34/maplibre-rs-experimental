#![expect(
    clippy::expect_used,
    reason = "benchmark DEM fixture generation and decoding abort on errors"
)]

use std::{collections::HashMap, hint::black_box, sync::Arc};

use criterion::{criterion_group, criterion_main, Criterion};
use image::{Rgba, RgbaImage};
use maplibre::{
    coords::{WorldTileCoords, ZoomLevel},
    projection::body::Body,
    raster::RasterSourceId,
    tcs::tiles::Tiles,
    terrain::{
        backfill::neighbours,
        backfill_neighbours,
        dem_store::{decode_image_to_dem, ingest_source_dem},
        DemTile, DemTileComponent, LoadedDem,
    },
};

const UNPACK_TERRARIUM: [f64; 4] = [256.0, 1.0, 1.0 / 256.0, 32768.0];

fn coords(x: i32, y: i32, z: u8) -> WorldTileCoords {
    WorldTileCoords {
        x,
        y,
        z: ZoomLevel::new(z),
    }
}

fn synthetic_terrarium_grid() -> Vec<(WorldTileCoords, Arc<RgbaImage>)> {
    let base_x = 2422;
    let base_y = 1388;
    let mut out = Vec::with_capacity(9);
    for dy in 0..3_i32 {
        for dx in 0..3_i32 {
            let c = coords(base_x + dx, base_y + dy, 12);
            let tile_offset = (dx * 256 + dy * 128) as u32;
            let image = RgbaImage::from_fn(256, 256, |x, y| {
                let elev = 800.0 + f64::from(x + tile_offset) * 1.5 + f64::from(y) * 0.75;
                let encoded = elev + 32768.0;
                let r = (encoded / 256.0).floor();
                let g = (encoded - r * 256.0).floor();
                let b = ((encoded - r * 256.0 - g) * 256.0).floor();
                Rgba([r as u8, g as u8, b as u8, 255])
            });
            out.push((c, Arc::new(image)));
        }
    }
    out
}

fn legacy_pixel_of(tile: &RgbaImage, x: u32, y: u32) -> [u8; 4] {
    let idx = ((y * tile.width() + x) * 4) as usize;
    tile.as_raw()
        .get(idx..idx + 4)
        .and_then(|s| s.try_into().ok())
        .unwrap_or([0; 4])
}

fn legacy_set_pixel(buf: &mut [u8], padded_w: u32, x: u32, y: u32, px: [u8; 4]) {
    let idx = ((y * padded_w + x) * 4) as usize;
    if let Some(dst) = buf.get_mut(idx..idx + 4) {
        dst.copy_from_slice(&px);
    }
}

/// Legacy main-thread 1-texel border assembly kept here as the historical baseline for `raster/dem_border.rs`, executed on
/// the render thread for each newly arrived tile and every already-uploaded neighbour lacking its edge.
fn legacy_with_border(image: &RgbaImage, grid: &[[Option<&RgbaImage>; 3]; 3]) -> RgbaImage {
    let (width, height) = image.dimensions();
    let padded_w = width + 2;
    let padded_h = height + 2;
    let mut out = vec![0_u8; (padded_w * padded_h * 4) as usize];
    let src = image.as_raw();
    let row_bytes = (width * 4) as usize;
    let dst_stride = (padded_w * 4) as usize;

    for y in 0..height as usize {
        let s = y * row_bytes;
        let d = (y + 1) * dst_stride + 4;
        if let (Some(dst), Some(src_row)) =
            (out.get_mut(d..d + row_bytes), src.get(s..s + row_bytes))
        {
            dst.copy_from_slice(src_row);
        }
    }

    let north = grid[0][1].filter(|n| n.dimensions() == (width, height));
    let south = grid[2][1].filter(|n| n.dimensions() == (width, height));
    let west = grid[1][0].filter(|n| n.dimensions() == (width, height));
    let east = grid[1][2].filter(|n| n.dimensions() == (width, height));

    for x in 0..width {
        let top = north.map_or_else(
            || legacy_pixel_of(image, x, 0),
            |n| legacy_pixel_of(n, x, height - 1),
        );
        let bot = south.map_or_else(
            || legacy_pixel_of(image, x, height - 1),
            |s| legacy_pixel_of(s, x, 0),
        );
        legacy_set_pixel(&mut out, padded_w, x + 1, 0, top);
        legacy_set_pixel(&mut out, padded_w, x + 1, padded_h - 1, bot);
    }
    for y in 0..height {
        let l = west.map_or_else(
            || legacy_pixel_of(image, 0, y),
            |w| legacy_pixel_of(w, width - 1, y),
        );
        let r = east.map_or_else(
            || legacy_pixel_of(image, width - 1, y),
            |e| legacy_pixel_of(e, 0, y),
        );
        legacy_set_pixel(&mut out, padded_w, 0, y + 1, l);
        legacy_set_pixel(&mut out, padded_w, padded_w - 1, y + 1, r);
    }
    fill_legacy_corners(&mut out, image, grid, width, height, padded_w, padded_h);

    RgbaImage::from_raw(padded_w, padded_h, out).expect("valid dimensions")
}

fn fill_legacy_corners(
    out: &mut [u8],
    image: &RgbaImage,
    grid: &[[Option<&RgbaImage>; 3]; 3],
    width: u32,
    height: u32,
    padded_w: u32,
    padded_h: u32,
) {
    for (cx, cy, gx, gy, nx, ny, fx, fy) in [
        (0, 0, 0, 0, width - 1, height - 1, 0, 0),
        (padded_w - 1, 0, 2, 0, 0, height - 1, width - 1, 0),
        (0, padded_h - 1, 0, 2, width - 1, 0, 0, height - 1),
        (
            padded_w - 1,
            padded_h - 1,
            2,
            2,
            0,
            0,
            width - 1,
            height - 1,
        ),
    ] {
        let px = grid
            .get(gy)
            .and_then(|row| row.get(gx))
            .copied()
            .flatten()
            .filter(|n| n.dimensions() == (width, height))
            .map_or_else(
                || legacy_pixel_of(image, fx, fy),
                |n| legacy_pixel_of(n, nx, ny),
            );
        legacy_set_pixel(out, padded_w, cx, cy, px);
    }
}

fn legacy_neighbour_grid<'a>(
    store: &'a HashMap<WorldTileCoords, RgbaImage>,
    c: WorldTileCoords,
) -> [[Option<&'a RgbaImage>; 3]; 3] {
    let mut grid: [[Option<&'a RgbaImage>; 3]; 3] = Default::default();
    for (n, (dx, dy)) in neighbours(c) {
        if let Some(cell) = grid
            .get_mut((dy + 1) as usize)
            .and_then(|row| row.get_mut((dx + 1) as usize))
        {
            *cell = store.get(&n);
        }
    }
    grid
}

fn encode_png(image: &RgbaImage) -> Vec<u8> {
    let mut cursor = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image.clone())
        .write_to(&mut cursor, image::ImageFormat::Png)
        .expect("encode synthetic DEM PNG");
    cursor.into_inner()
}

fn bench_dem_3x3_group(
    c: &mut Criterion,
    png_grid: &[(WorldTileCoords, Vec<u8>)],
    source: &RasterSourceId,
    circumference: f64,
) {
    // Benchmark 1: End-to-end 3x3 viewport PNG decode + DEM ingestion + border assembly when
    // `terrain` and `hillshade`/`color-relief` share one `raster-dem` source.
    let mut full_group = c.benchmark_group("dem_3x3_shared_terrain_and_hillshade");
    full_group.bench_function("legacy_split_decode_and_main_thread_border", |b| {
        b.iter(|| {
            let mut tiles = Tiles::default();
            let mut raster_images: HashMap<WorldTileCoords, RgbaImage> = HashMap::with_capacity(9);
            let mut uploaded_bordered: HashMap<WorldTileCoords, RgbaImage> =
                HashMap::with_capacity(9);

            for (coords, png_bytes) in png_grid {
                // 1. Terrain worker PNG decode + `DemTile` build + 2-texel backfill
                let terrain_rgba = image::load_from_memory(png_bytes)
                    .expect("png")
                    .into_rgba8();
                let dem =
                    decode_image_to_dem(*coords, &terrain_rgba, UNPACK_TERRARIUM, circumference)
                        .expect("dem");
                tiles
                    .spawn_mut(*coords)
                    .expect("tile")
                    .insert(DemTileComponent::Loaded(dem));
                backfill_neighbours(&mut tiles, *coords);

                // 2. Duplicate raster worker PNG decode + main-thread `with_border` re-uploads
                let raster_rgba = image::load_from_memory(png_bytes)
                    .expect("png")
                    .into_rgba8();
                raster_images.insert(*coords, raster_rgba);
                let mut dirty = vec![*coords];
                for (n, _) in neighbours(*coords) {
                    if uploaded_bordered.contains_key(&n) {
                        dirty.push(n);
                    }
                }
                for target in dirty {
                    if let Some(img) = raster_images.get(&target) {
                        let n_grid = legacy_neighbour_grid(&raster_images, target);
                        uploaded_bordered.insert(target, legacy_with_border(img, &n_grid));
                    }
                }
            }
            black_box((tiles.tiles.len(), uploaded_bordered.len()))
        });
    });

    full_group.bench_function("unified_single_decode_and_shared_dem_store", |b| {
        b.iter(|| {
            let mut tiles = Tiles::default();
            for (coords, png_bytes) in png_grid {
                let rgba = image::load_from_memory(png_bytes)
                    .expect("png")
                    .into_rgba8();
                let dem = decode_image_to_dem(*coords, &rgba, UNPACK_TERRARIUM, circumference)
                    .expect("dem");
                ingest_source_dem(&mut tiles, source, *coords, dem, true);
            }
            black_box(tiles.tiles.len())
        });
    });
    full_group.finish();
}

fn bench_dem_centre_arrival_group(
    c: &mut Criterion,
    grid: &[(WorldTileCoords, Arc<RgbaImage>)],
    predecoded: &[(WorldTileCoords, Arc<DemTile>)],
    source: &RasterSourceId,
) {
    // Benchmark 2: Main-thread render-frame cost when the centre DEM tile arrives after its 8
    // surrounding neighbours are already resident & uploaded.
    let Some((centre, centre_img_arc)) = grid.get(4) else {
        return;
    };
    let Some((_, centre_dem_arc)) = predecoded.get(4) else {
        return;
    };
    let centre = *centre;
    let centre_image = (**centre_img_arc).clone();
    let centre_shared_dem = Arc::clone(centre_dem_arc);

    let mut border_group = c.benchmark_group("dem_centre_arrival_main_thread_cost");
    border_group.bench_function("legacy_reborder_9_tiles_on_render_thread", |b| {
        let mut base_images: HashMap<WorldTileCoords, RgbaImage> = HashMap::with_capacity(9);
        for (i, (coord, img)) in grid.iter().enumerate() {
            if i != 4 {
                base_images.insert(*coord, RgbaImage::clone(img));
            }
        }
        b.iter(|| {
            let mut images = base_images.clone();
            images.insert(centre, centre_image.clone());
            let mut uploads = 0_usize;
            for (coord, img) in &images {
                let n_grid = legacy_neighbour_grid(&images, *coord);
                let bordered = legacy_with_border(img, &n_grid);
                uploads += bordered.as_raw().len();
            }
            black_box(uploads)
        });
    });

    border_group.bench_function("unified_in_place_2texel_strip_backfill", |b| {
        let mut base_tiles = Tiles::default();
        for (i, (coord, dem)) in predecoded.iter().enumerate() {
            if i != 4 {
                ingest_source_dem(
                    &mut base_tiles,
                    source,
                    *coord,
                    LoadedDem::from_shared(Arc::clone(dem)),
                    true,
                );
            }
        }
        b.iter(|| {
            // Reset centre + backfill strips on already-decoded worker `Arc<DemTile>`
            ingest_source_dem(
                &mut base_tiles,
                source,
                centre,
                LoadedDem::from_shared(Arc::clone(&centre_shared_dem)),
                true,
            );
            black_box(base_tiles.tiles.len())
        });
    });
    border_group.finish();
}

fn bench_dem_pipeline(c: &mut Criterion) {
    let grid = synthetic_terrarium_grid();
    let png_grid: Vec<(WorldTileCoords, Vec<u8>)> = grid
        .iter()
        .map(|(coords, img)| (*coords, encode_png(img)))
        .collect();
    let circumference = Body::EARTH.circumference_meters();
    let source = RasterSourceId::from("dem");

    let predecoded: Vec<(WorldTileCoords, Arc<DemTile>)> = grid
        .iter()
        .map(|(coords, image)| {
            let loaded = decode_image_to_dem(*coords, image, UNPACK_TERRARIUM, circumference)
                .expect("valid synthetic DEM");
            (*coords, loaded.tile)
        })
        .collect();

    bench_dem_3x3_group(c, &png_grid, &source, circumference);
    bench_dem_centre_arrival_group(c, &grid, &predecoded, &source);
}

criterion_group!(benches, bench_dem_pipeline);
criterion_main!(benches);
