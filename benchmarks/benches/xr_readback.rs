//! Cost of one camera frame: an external-view render and its readback into a caller buffer.
//!
//! The scene is a background over synthetic Terrarium terrain, so the numbers are the
//! render floor of a terrain view plus the full readback cost; imagery and vector layers add
//! to the render side only.

use std::time::{Duration, Instant};

use cgmath::{Deg, Matrix4, Rad, SquareMatrix, Vector3};
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use image::{Rgba, RgbaImage};
use maplibre::{
    coords::{LatLon, WorldTileCoords},
    headless::{
        create_headless_renderer,
        map::{
            readback::{FrameReadback, PixelLayout},
            HeadlessMap,
        },
        HeadlessPlugin,
    },
    render::{
        camera::EyeFrustum,
        view_state::ExternalAnchor,
        xr::{EyeTarget, ScenePlacement, XrEye, XrFrame},
        RenderPlugin,
    },
    style::Style,
    terrain::{DefaultDemTransferables, TerrainPlugin},
};

const SIZES: [(u32, u32); 2] = [(640, 480), (1280, 720)];
const SITE: LatLon = LatLon {
    latitude: 40.577_186,
    longitude: -74.738_192,
};
const DEM_ZOOM: u8 = 12;

/// The z12 tile holding the site.
fn site_tile() -> (i32, i32) {
    let n = f64::from(1_u32 << DEM_ZOOM);
    let x = (SITE.longitude + 180.0) / 360.0 * n;
    let lat = SITE.latitude.to_radians();
    let y = (1.0 - (lat.tan() + 1.0 / lat.cos()).ln() / std::f64::consts::PI) / 2.0 * n;
    (x as i32, y as i32)
}

/// Rolling hills around 60 m, in Terrarium encoding.
fn dem_tile(tile_x: i32, tile_y: i32) -> RgbaImage {
    RgbaImage::from_fn(256, 256, |x, y| {
        let u = f64::from(tile_x * 256 + x as i32) / 40.0;
        let v = f64::from(tile_y * 256 + y as i32) / 55.0;
        let metres = 60.0 + 25.0 * u.sin() * v.cos();
        let encoded = metres + 32768.0;
        let red = (encoded / 256.0).floor();
        let green = (encoded - red * 256.0).floor();
        let blue = ((encoded - red * 256.0 - green) * 256.0).floor();
        Rgba([red as u8, green as u8, blue as u8, 255])
    })
}

fn map(runtime: &tokio::runtime::Runtime, width: u32, height: u32) -> HeadlessMap {
    runtime.block_on(async {
        let style: Style = serde_json::from_str(
            r##"{"version":8,"projection":{"type":"mercator"},"terrain":{"source":"dem"},
            "sources":{"dem":{"type":"raster-dem","tiles":["offline://dem"],"tileSize":256,"maxzoom":12,"encoding":"terrarium"}},
            "layers":[{"id":"background","type":"background","paint":{"background-color":"#6f8a5a"}}]}"##,
        )
        .unwrap_or_else(|error| panic!("benchmark style: {error}"));
        let (kernel, renderer) = create_headless_renderer(width, height, None)
            .await
            .unwrap_or_else(|error| panic!("benchmark renderer: {error}"));
        let mut map = HeadlessMap::new(
            style,
            renderer,
            kernel,
            vec![
                Box::new(RenderPlugin),
                Box::new(maplibre::background::BackgroundPlugin),
                Box::new(TerrainPlugin::<DefaultDemTransferables>::default()),
                Box::new(
                    HeadlessPlugin::new(false)
                        .preserve_tile_sources()
                        .retain_supplied_tiles(),
                ),
            ],
        )
        .unwrap_or_else(|error| panic!("benchmark map: {error}"));
        let (x, y) = site_tile();
        let tiles = (-1..=1)
            .flat_map(|dy| (-1..=1).map(move |dx| (x + dx, y + dy)))
            .map(|(x, y)| {
                (
                    WorldTileCoords::from((x, y, DEM_ZOOM.into())),
                    dem_tile(x, y),
                )
            })
            .collect();
        map.load_dem_tiles(tiles)
            .unwrap_or_else(|error| panic!("benchmark DEM: {error}"));
        map
    })
}

/// A forward camera 120 m over the site, pitched 10° down, as a low drone sees it.
fn frame(frame: u64, width: u32, height: u32, color: &wgpu::Texture) -> XrFrame {
    let aspect = f64::from(width) / f64::from(height);
    XrFrame {
        opaque_environment: true,
        timestamp: Duration::from_millis(16 * frame),
        placement: ScenePlacement {
            anchor: ExternalAnchor {
                position: SITE,
                altitude_meters: 0.0,
            },
            world_from_scene: Matrix4::identity(),
        },
        eyes: vec![XrEye {
            world_from_eye: Matrix4::from_translation(Vector3::new(0.0, 0.0, 180.0))
                * Matrix4::from_angle_x(Deg(80.0)),
            frustum: EyeFrustum::symmetric(Rad(1.2 / aspect), aspect, 1.0, 50_000.0),
            target: EyeTarget {
                color: Some(color.create_view(&Default::default())),
                depth: None,
            },
        }],
        request_overscan: 1.0,
        prefetch: None,
    }
}

fn colour_target(map: &HeadlessMap, width: u32, height: u32) -> wgpu::Texture {
    let format = map
        .head_texture()
        .unwrap_or_else(|| panic!("an offscreen head"))
        .format();
    map.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("camera colour"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

fn camera_frame(c: &mut Criterion) {
    // Tile requests for ground outside the supplied DEM spawn onto the runtime of the frame.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|error| panic!("benchmark runtime: {error}"));
    let _entered = runtime.enter();
    let mut group = c.benchmark_group("camera_frame");
    for (width, height) in SIZES {
        let mut map = map(&runtime, width, height);
        let color = colour_target(&map, width, height);
        let mut readback = FrameReadback::new(&map, width, height);
        let mut rgb = vec![0; readback.frame_len(PixelLayout::Rgb8)];
        let mut count = 0_u64;
        // Settle the terrain mesh before timing.
        for _ in 0..8 {
            count = count.wrapping_add(1);
            map.run_xr_frame(frame(count, width, height, &color))
                .unwrap_or_else(|error| panic!("settle frame: {error:?}"));
        }
        let label = format!("{width}x{height}");
        group.bench_function(BenchmarkId::new("render", &label), |b| {
            b.iter_custom(|iterations| {
                let start = Instant::now();
                for _ in 0..iterations {
                    count = count.wrapping_add(1);
                    map.run_xr_frame(frame(count, width, height, &color))
                        .unwrap_or_else(|error| panic!("render: {error}"));
                    map.device()
                        .poll(wgpu::PollType::wait_indefinitely())
                        .unwrap_or_else(|error| panic!("poll: {error}"));
                }
                start.elapsed()
            });
        });
        group.bench_function(BenchmarkId::new("readback_rgb8", &label), |b| {
            b.iter(|| {
                readback
                    .read_into_blocking(&map, &color, PixelLayout::Rgb8, &mut rgb)
                    .unwrap_or_else(|error| panic!("readback: {error}"));
            });
        });
        group.bench_function(BenchmarkId::new("render_and_readback_rgb8", &label), |b| {
            b.iter(|| {
                count = count.wrapping_add(1);
                map.run_xr_frame(frame(count, width, height, &color))
                    .unwrap_or_else(|error| panic!("render: {error}"));
                readback
                    .read_into_blocking(&map, &color, PixelLayout::Rgb8, &mut rgb)
                    .unwrap_or_else(|error| panic!("readback: {error}"));
            });
        });
    }
    group.finish();
}

criterion_group!(name = benches;
    config = Criterion::default().significance_level(0.1).sample_size(30);
    targets = camera_frame);
criterion_main!(benches);
