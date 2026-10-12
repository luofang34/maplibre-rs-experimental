//! The vertical-perspective globe orbits the terrain under its center: drawn terrain, the
//! camera and the depth buffer agree on where the ground is in every view.
#![allow(clippy::expect_used, clippy::panic)]

use cgmath::{InnerSpace, Point2};

use super::HeadlessMap;
use crate::{
    io::source_client::{HttpClient, SourceFetchError},
    projection::globe::ray_sphere_intersection,
    render::{projection::globe_camera_for_view, RenderPlugin},
    style::Style,
};

const WIDTH: u32 = 2330;
const HEIGHT: u32 = 1800;

/// Serves elevation tiles of ground at `meters`, with hills of `relief` metres either side a
/// few kilometres apart, encoded as Terrarium at the Mercator position of every sample.
#[derive(Clone, Copy)]
struct SyntheticDem {
    meters: f64,
    relief: f64,
}

impl SyntheticDem {
    fn height(&self, x: f64, y: f64) -> f64 {
        self.meters + self.relief * (x * 9000.0).sin() * (y * 7000.0).cos()
    }
}

#[cfg_attr(not(feature = "thread-safe-futures"), async_trait::async_trait(?Send))]
#[cfg_attr(feature = "thread-safe-futures", async_trait::async_trait)]
impl HttpClient for SyntheticDem {
    async fn fetch(&self, url: &str) -> Result<Vec<u8>, SourceFetchError> {
        let address: Vec<f64> = url
            .trim_end_matches(".png")
            .rsplit('/')
            .take(3)
            .map(|part| part.parse().expect("tile address"))
            .collect();
        let (y, x, scale) = (address[0], address[1], 2_f64.powf(address[2]));
        let image = image::RgbaImage::from_fn(256, 256, |column, row| {
            let height = self.height(
                (x + (f64::from(column) + 0.5) / 256.0) / scale,
                (y + (f64::from(row) + 0.5) / 256.0) / scale,
            );
            let encoded = ((height + 32768.0) * 256.0).round() as u32;
            image::Rgba([
                (encoded >> 16) as u8,
                (encoded >> 8) as u8,
                encoded as u8,
                255,
            ])
        });
        let mut png = std::io::Cursor::new(Vec::new());
        image
            .write_to(&mut png, image::ImageFormat::Png)
            .expect("PNG");
        Ok(png.into_inner())
    }
}

/// A view of the scene the vertical-perspective terrain path is checked in.
#[derive(Clone, Copy, Debug)]
struct Scene {
    meters: f64,
    relief: f64,
    exaggeration: f64,
    pitch: f64,
    zoom: f64,
}

struct LevelMap {
    map: HeadlessMap,
    depth: wgpu::Texture,
}

impl LevelMap {
    async fn new(scene: Scene) -> Self {
        let style = format!(
            r##"{{"version":8,"center":[88.05464359004759,27.765393137835165],"zoom":{zoom},"pitch":{pitch},"bearing":324.03714296701236,"sources":{{"dem":{{"type":"raster-dem","tiles":["https://dem.example/{{z}}/{{x}}/{{y}}.png"],"encoding":"terrarium","tileSize":256}}}},"layers":[{{"id":"background","type":"background","paint":{{"background-color":"#336699"}}}}],"terrain":{{"source":"dem","exaggeration":{exaggeration}}},"projection":{{"type":"vertical-perspective"}}}}"##,
            zoom = scene.zoom,
            pitch = scene.pitch,
            exaggeration = scene.exaggeration,
        );
        let style: Style = serde_json::from_str(&style).expect("style");
        let (kernel, renderer) = crate::headless::create_headless_renderer_with_loader(
            WIDTH,
            HEIGHT,
            Default::default(),
            crate::io::resource_loader::SharedLoader::new(SyntheticDem {
                meters: scene.meters,
                relief: scene.relief,
            }),
        )
        .await
        .expect("renderer");
        let mut map = HeadlessMap::new(
            style,
            renderer,
            kernel,
            vec![
                Box::new(RenderPlugin),
                Box::new(crate::background::BackgroundPlugin),
                Box::new(crate::terrain::TerrainPlugin::<
                    crate::terrain::DefaultDemTransferables,
                >::default()),
                Box::new(crate::overlay::OverlayPlugin),
            ],
        )
        .expect("map");
        map.set_max_pitch(cgmath::Deg(85.0));
        let depth = map.device().create_texture(&wgpu::TextureDescriptor {
            label: Some("vertical perspective depth"),
            size: wgpu::Extent3d {
                width: WIDTH,
                height: HEIGHT,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        map.map_context.renderer.resources.eye_depth_target =
            Some(depth.create_view(&Default::default()));
        Self { map, depth }
    }

    /// Draws until the center stands on the loaded terrain, at `expected_center` metres when
    /// given, and two depth readbacks agree, so the check sees the settled frame rather than
    /// one still loading.
    async fn settle(&mut self, expected_center: Option<f64>) -> Vec<f32> {
        let mut previous: Option<Vec<f32>> = None;
        for _ in 0..40 {
            for _ in 0..4 {
                self.map.run_frame().expect("frame");
                for _ in 0..8 {
                    tokio::task::yield_now().await;
                }
            }
            let depth = self.read_depth();
            let center = self.map.view_state().center_elevation();
            let centered =
                expected_center.map_or(center != 0.0, |expected| (center - expected).abs() < 0.5);
            if centered && previous.as_ref() == Some(&depth) {
                return depth;
            }
            previous = Some(depth);
        }
        panic!(
            "the frame never settled: center at {} m, expected {expected_center:?} m",
            self.map.view_state().center_elevation()
        );
    }

    /// The frame's colour as RGBA bytes, row by row.
    fn read_color(&self) -> Vec<[u8; 4]> {
        let texture = self.map.head_texture().expect("head texture");
        let row = (WIDTH * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
            * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer = self.map.device().create_buffer(&wgpu::BufferDescriptor {
            label: Some("vertical perspective colour readback"),
            size: u64::from(row * HEIGHT),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .map
            .device()
            .create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: None,
                },
            },
            texture.size(),
        );
        self.map.queue().submit([encoder.finish()]);
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, |result| result.expect("readback"));
        self.map
            .device()
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("GPU readback completes");
        let bytes = buffer.slice(..).get_mapped_range().expect("range").to_vec();
        buffer.unmap();
        (0..HEIGHT)
            .flat_map(|y| {
                let bytes = &bytes;
                (0..WIDTH).map(move |x| {
                    let at = (y * row + x * 4) as usize;
                    [bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]
                })
            })
            .collect()
    }

    fn read_depth(&self) -> Vec<f32> {
        let row = (WIDTH * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
            * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer = self.map.device().create_buffer(&wgpu::BufferDescriptor {
            label: Some("vertical perspective depth readback"),
            size: u64::from(row * HEIGHT),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .map
            .device()
            .create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            self.depth.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: None,
                },
            },
            self.depth.size(),
        );
        self.map.queue().submit([encoder.finish()]);
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, |result| result.expect("readback"));
        self.map
            .device()
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("GPU readback completes");
        let bytes = buffer.slice(..).get_mapped_range().expect("range").to_vec();
        buffer.unmap();
        (0..HEIGHT)
            .flat_map(|y| {
                let bytes = &bytes;
                (0..WIDTH).map(move |x| {
                    let at = (y * row + x * 4) as usize;
                    f32::from_le_bytes(bytes[at..at + 4].try_into().expect("texel"))
                })
            })
            .collect()
    }
}

/// Renders `scene` and checks every sampled pixel's depth against the distance along the view
/// axis at which the camera's ray through it meets the level ground.
async fn check(scene: Scene) {
    let ground = scene.meters * scene.exaggeration;
    let mut level = LevelMap::new(scene).await;
    let depth = level.settle(Some(ground)).await;
    let camera = globe_camera_for_view(level.map.view_state()).expect("camera");
    let (near, far) = camera.depth_range();
    let distance = |depth: f32| near * far / (f64::from(depth) * (far - near) + near);
    // The viewport's center is the corner of four pixels; at a grazing pitch half a pixel moves
    // the ground a long way, so the four average out to the ground at the corner.
    let center = [(0, 0), (1, 0), (0, 1), (1, 1)]
        .map(|(dx, dy)| distance(depth[((HEIGHT / 2 - dy) * WIDTH + WIDTH / 2 - dx) as usize]))
        .iter()
        .sum::<f64>()
        / 4.0;
    assert!(
        (center / camera.camera_to_center_distance() - 1.0).abs() < 2e-3,
        "{scene:?}: the ground at the center lies {center} px away, the camera orbits {} px away",
        camera.camera_to_center_distance()
    );
    let eye = camera.camera_position();
    let sphere = 1.0 + ground / camera.body().radius_meters;
    let (mut checked, mut holes, mut floating) = (0, 0, 0);
    for y in (0..HEIGHT).step_by(15) {
        for x in (0..WIDTH).step_by(15) {
            let value = depth[(y * WIDTH + x) as usize];
            let ray = camera
                .ray_direction_from_pixel(Point2::new(f64::from(x) + 0.5, f64::from(y) + 0.5))
                .expect("ray");
            let hit = ray_sphere_intersection(eye, ray, sphere).filter(|hit| hit.t_min > 0.0);
            let Some(hit) = hit else {
                // Beyond the ground's silhouette only the sky shows.
                floating += usize::from(
                    value > 0.0 && ray_sphere_intersection(eye, ray, sphere * 1.0001).is_none(),
                );
                continue;
            };
            let point = eye + ray * hit.t_min;
            // Grazing rays meet the flat triangles of the far tiles well off the sphere.
            if -point.normalize().dot(ray) < 0.2 {
                continue;
            }
            checked += 1;
            if value <= 0.0 {
                holes += 1;
                continue;
            }
            let view = camera.view() * point.extend(1.0);
            let expected = -view.z / view.w;
            assert!(
                (distance(value) / expected - 1.0).abs() < 1e-2,
                "{scene:?}: ({x},{y}) holds ground {} px away, the camera puts it {expected} px away",
                distance(value)
            );
        }
    }
    assert!(checked > 1000, "{scene:?}: {checked} ground pixels checked");
    assert_eq!(
        holes, 0,
        "{scene:?}: {holes} of {checked} ground pixels show no terrain"
    );
    assert_eq!(floating, 0, "{scene:?}: {floating} sky pixels hold terrain");
}

#[tokio::test]
async fn level_terrain_meets_the_camera_at_its_center_and_everywhere_in_view() {
    for (meters, exaggeration) in [(4000.0, 1.0), (4000.0, 2.0), (0.0, 1.0), (-400.0, 1.0)] {
        for pitch in [70.0, 85.0] {
            check(Scene {
                meters,
                relief: 0.0,
                exaggeration,
                pitch,
                zoom: 11.67,
            })
            .await;
        }
    }
}

/// Distance along the view axis to the ground picked through `pixel`: `None` for the sky or a
/// polar cap, an error for ground without DEM.
fn picked_distance(
    camera: &crate::projection::globe::camera::GlobeCameraState,
    terrain: crate::terrain::sightline::DrawnTerrain<'_>,
    pixel: Point2<f64>,
) -> Result<Option<f64>, ()> {
    use crate::terrain::sightline::{pick_globe_terrain, TerrainPick};
    match pick_globe_terrain(camera, terrain, pixel) {
        TerrainPick::Ground(hit) => {
            let location = crate::coords::LatLon::new(
                (std::f64::consts::PI * (1.0 - 2.0 * hit.mercator.y))
                    .sinh()
                    .atan()
                    .to_degrees(),
                hit.mercator.x * 360.0 - 180.0,
            );
            let point = crate::projection::globe::lat_lon_to_unit_sphere(location)
                * camera.body().unit_radius_at(hit.elevation);
            let view = camera.view() * point.extend(1.0);
            Ok(Some(-view.z / view.w))
        }
        TerrainPick::Unknown => Err(()),
        TerrainPick::Sky | TerrainPick::PolarCap(_) => Ok(None),
    }
}

/// Every sampled pixel's terrain pick lies where the frame drew the ground: at the depth the
/// GPU stored there, on hills whose every sample differs.
#[tokio::test]
async fn picks_land_where_the_frame_draws_the_ground() {
    use crate::terrain::{sightline::DrawnTerrain, TerrainCoverageIndex};
    for pitch in [70.0, 85.0] {
        let scene = Scene {
            meters: 1500.0,
            relief: 600.0,
            exaggeration: 1.0,
            pitch,
            zoom: 11.67,
        };
        let mut hills = LevelMap::new(scene).await;
        let depth = hills.settle(None).await;
        let camera = globe_camera_for_view(hills.map.view_state()).expect("camera");
        let (near, far) = camera.depth_range();
        let world = hills.map.world();
        let terrain = DrawnTerrain {
            index: world
                .resources
                .get::<TerrainCoverageIndex>()
                .expect("coverage index"),
            tiles: &world.tiles,
            body: camera.body(),
        };
        // The frame keeps the nearest of a pixel's samples, the standard four-sample pattern;
        // at a grazing view one pixel spans a long stretch of ground.
        let samples = [
            (0.375, 0.125),
            (0.875, 0.375),
            (0.125, 0.625),
            (0.625, 0.875),
        ];
        let distance_of = |pixel| picked_distance(&camera, terrain, pixel);
        let (mut picked, mut unknown) = (0, 0);
        for y in (0..HEIGHT).step_by(90) {
            for x in (0..WIDTH).step_by(90) {
                let picks: Result<Vec<Option<f64>>, ()> = samples
                    .iter()
                    .map(|(dx, dy)| distance_of(Point2::new(f64::from(x) + dx, f64::from(y) + dy)))
                    .collect();
                let Ok(picks) = picks else {
                    unknown += 1;
                    continue;
                };
                let Some(nearest) = picks.iter().flatten().copied().reduce(f64::min) else {
                    continue;
                };
                let stored = depth[(y * WIDTH + x) as usize];
                assert!(
                    stored > 0.0,
                    "{scene:?}: ({x},{y}) picks ground the frame left empty"
                );
                let drawn = near * far / (f64::from(stored) * (far - near) + near);
                assert!(
                    (drawn / nearest - 1.0).abs() < 2e-3,
                    "{scene:?}: ({x},{y}) picks ground {nearest} px away, the frame drew it {drawn} px away"
                );
                picked += 1;
            }
        }
        assert!(picked > 150, "{scene:?}: {picked} pixels picked ground");
        assert_eq!(
            unknown, 0,
            "{scene:?}: the settled frame has ground without DEM"
        );
    }
}

mod poles;
mod unclamped;
mod zoom;

/// Through [`HeadlessMap::drawn_terrain`], ground the frame draws is in sight of the camera,
/// and ground nearer drawn ground covers on the screen is not.
#[tokio::test]
async fn drawn_terrain_sees_the_picked_ground_and_not_what_ridges_cover() {
    use crate::terrain::sightline::{
        pick_globe_terrain, target_view, TargetAltitude, TerrainPick, Visibility,
    };
    let scene = Scene {
        meters: 1500.0,
        relief: 600.0,
        exaggeration: 1.0,
        pitch: 80.0,
        zoom: 11.67,
    };
    let mut hills = LevelMap::new(scene).await;
    hills.settle(None).await;
    let camera = globe_camera_for_view(hills.map.view_state()).expect("camera");
    let terrain = hills
        .map
        .drawn_terrain(camera.body())
        .expect("terrain drawn");
    let mut picks = Vec::new();
    for y in (HEIGHT / 3..HEIGHT).step_by(3) {
        let pixel = Point2::new(f64::from(WIDTH / 2) + 0.5, f64::from(y) + 0.5);
        if let TerrainPick::Ground(hit) = pick_globe_terrain(&camera, terrain, pixel) {
            picks.push(crate::coords::LatLon::new(
                (std::f64::consts::PI * (1.0 - 2.0 * hit.mercator.y))
                    .sinh()
                    .atan()
                    .to_degrees(),
                hit.mercator.x * 360.0 - 180.0,
            ));
        }
    }
    assert!(picks.len() > 100, "{} ground picks", picks.len());
    let seen = |at| target_view(&camera, Some(terrain), at, TargetAltitude::OnGround).visibility;
    let visible = picks
        .iter()
        .filter(|&&at| seen(at) == Visibility::Visible)
        .count();
    assert!(
        visible * 100 >= picks.len() * 98,
        "{visible} of {} picked points in sight",
        picks.len()
    );
    // Over the picked region, ground is hidden exactly when the pick through its pixel lands
    // nearer the camera than it.
    let (lat, lon) = picks.iter().fold(
        ((f64::MAX, f64::MIN), (f64::MAX, f64::MIN)),
        |((a, b), (c, d)), p| {
            (
                (a.min(p.latitude), b.max(p.latitude)),
                (c.min(p.longitude), d.max(p.longitude)),
            )
        },
    );
    let eye = camera.camera_position();
    let (mut agree, mut checked, mut hidden) = (0, 0, 0);
    for i in 0..60 {
        for j in 0..60 {
            let at = crate::coords::LatLon::new(
                lat.0 + (lat.1 - lat.0) * f64::from(i) / 59.0,
                lon.0 + (lon.1 - lon.0) * f64::from(j) / 59.0,
            );
            let view = target_view(&camera, Some(terrain), at, TargetAltitude::OnGround);
            let (Some(pixel), Some(elevation)) = (view.pixel, view.elevation) else {
                continue;
            };
            // Off the screen no pick describes the point.
            if view.visibility == Visibility::OutsideFrustum {
                continue;
            }
            let TerrainPick::Ground(hit) = pick_globe_terrain(&camera, terrain, pixel) else {
                continue;
            };
            let unit = |at, meters| {
                crate::projection::globe::lat_lon_to_unit_sphere(at)
                    * camera.body().unit_radius_at(meters)
            };
            let picked = crate::coords::LatLon::new(
                (std::f64::consts::PI * (1.0 - 2.0 * hit.mercator.y))
                    .sinh()
                    .atan()
                    .to_degrees(),
                hit.mercator.x * 360.0 - 180.0,
            );
            let own = (unit(at, elevation) - eye).magnitude();
            let first = (unit(picked, hit.elevation) - eye).magnitude();
            // Too close to call: the pick and the point lie on the same patch of ground.
            if (first / own - 1.0).abs() < 2e-3 {
                continue;
            }
            let covered = first < own;
            checked += 1;
            hidden += usize::from(covered);
            let said = match view.visibility {
                Visibility::Visible => Some(false),
                Visibility::BehindTerrain => Some(true),
                _ => None,
            };
            agree += usize::from(said == Some(covered));
        }
    }
    assert!(hidden > 50, "{hidden} of {checked} points behind hills");
    assert!(
        agree == checked,
        "line of sight agrees with the picks for {agree} of {checked} points"
    );
}

/// A point of ground on a grid over hills, with how the frame should treat it.
struct Mark {
    x: f64,
    y: f64,
    /// The depth a host gives it, as projected without following the line of sight.
    depth: f32,
    in_sight: bool,
    /// How much nearer than it the ground drawn at its pixel is, as a share of its
    /// distance; zero where it is in sight.
    covered_by: f64,
}

impl Mark {
    /// Ground well behind a hill: more than twice the depth bias nearer.
    fn well_hidden(&self) -> bool {
        !self.in_sight && self.covered_by > 2.0 * f64::from(crate::overlay::DEPTH_BIAS)
    }
}

/// Hills seen at 80° pitch, and ground points on a grid over the ground down the middle of
/// the screen, out to where hills hide part of it, kept 8 px apart.
async fn ground_marks() -> (LevelMap, Vec<Mark>) {
    use crate::terrain::sightline::{target_view, TargetAltitude, Visibility};
    let scene = Scene {
        meters: 1500.0,
        relief: 600.0,
        exaggeration: 1.0,
        pitch: 80.0,
        zoom: 11.67,
    };
    let mut hills = LevelMap::new(scene).await;
    hills.settle(None).await;
    let camera = globe_camera_for_view(hills.map.view_state()).expect("camera");
    // Ground down the middle of the screen, out to where hills hide part of it.
    let terrain = hills.map.drawn_terrain(camera.body()).expect("terrain");
    let picked: Vec<_> = (HEIGHT / 3..HEIGHT)
        .step_by(3)
        .filter_map(|y| {
            let pixel = Point2::new(f64::from(WIDTH / 2) + 0.5, f64::from(y) + 0.5);
            match crate::terrain::sightline::pick_globe_terrain(&camera, terrain, pixel) {
                crate::terrain::sightline::TerrainPick::Ground(hit) => Some(hit.mercator),
                _ => None,
            }
        })
        .collect();
    let lat = |y: f64| {
        (std::f64::consts::PI * (1.0 - 2.0 * y))
            .sinh()
            .atan()
            .to_degrees()
    };
    let (y0, y1) = picked
        .iter()
        .fold((f64::MAX, f64::MIN), |(a, b), m| (a.min(m.y), b.max(m.y)));
    let (x0, x1) = picked
        .iter()
        .fold((f64::MAX, f64::MIN), |(a, b), m| (a.min(m.x), b.max(m.x)));
    let (lat0, lat1, lon0, lon1) = (lat(y1), lat(y0), x0 * 360.0 - 180.0, x1 * 360.0 - 180.0);
    let span = (lat1 - lat0).max(lon1 - lon0);
    // Marks on a grid of ground points, kept apart so none covers another, each a square
    // of 3 px at the depth of its ground; the ground's height comes from the terrain, its
    // depth as a host projects it, without following the line of sight.
    let mut marks = Vec::new();
    let mut taken: Vec<(f64, f64)> = Vec::new();
    for i in 0..80 {
        for j in 0..80 {
            let at = crate::coords::LatLon::new(
                lat0 + (lat1 - lat0) * f64::from(i) / 79.0,
                (lon0 + lon1) / 2.0 + span * (f64::from(j) / 79.0 - 0.5),
            );
            let seen = target_view(&camera, Some(terrain), at, TargetAltitude::OnGround);
            let (Some(elevation), true) = (
                seen.elevation,
                matches!(
                    seen.visibility,
                    Visibility::Visible | Visibility::BehindTerrain
                ),
            ) else {
                continue;
            };
            let bare = target_view(
                &camera,
                None,
                at,
                TargetAltitude::Drawn { meters: elevation },
            );
            let (Some(pixel), Some(depth)) = (bare.pixel, bare.depth) else {
                continue;
            };
            let (x, y) = (pixel.x, pixel.y);
            if x < 4.0 || y < 4.0 || x > f64::from(WIDTH) - 4.0 || y > f64::from(HEIGHT) - 4.0 {
                continue;
            }
            if taken
                .iter()
                .any(|&(u, v)| (u - x).abs() < 8.0 && (v - y).abs() < 8.0)
            {
                continue;
            }
            taken.push((x, y));
            // How much nearer than the point the ground drawn at its pixel is: a share of
            // its distance, zero where it is in sight.
            let unit = |at, meters| {
                crate::projection::globe::lat_lon_to_unit_sphere(at)
                    * camera.body().unit_radius_at(meters)
            };
            let eye = camera.camera_position();
            let own = (unit(at, elevation) - eye).magnitude();
            let covered_at = |dx: f64, dy: f64| {
                let p = Point2::new(pixel.x + dx, pixel.y + dy);
                match crate::terrain::sightline::pick_globe_terrain(&camera, terrain, p) {
                    crate::terrain::sightline::TerrainPick::Ground(hit) => {
                        let first = crate::coords::LatLon::new(
                            lat(hit.mercator.y),
                            hit.mercator.x * 360.0 - 180.0,
                        );
                        1.0 - (unit(first, hit.elevation) - eye).magnitude() / own
                    }
                    _ => 0.0,
                }
            };
            let covered_by = covered_at(0.0, 0.0);
            marks.push(Mark {
                x,
                y,
                depth: depth as f32,
                in_sight: seen.visibility == Visibility::Visible,
                covered_by,
            });
        }
    }
    let in_sight = marks.iter().filter(|m| m.in_sight).count();
    let well_hidden = marks.iter().filter(|m| m.well_hidden()).count();
    assert!(
        in_sight > 30 && well_hidden > 30,
        "{in_sight} marks in sight, {well_hidden} well behind hills"
    );
    (hills, marks)
}

/// An overlay drawn at the depth of the ground it marks shows where that ground is in sight
/// and not where nearer terrain covers it.
#[tokio::test]
async fn overlay_marks_on_hidden_ground_are_covered_by_the_terrain() {
    use crate::overlay::{OverlayMesh, OverlayVertex};
    let (mut hills, marks) = ground_marks().await;
    let mut mesh = OverlayMesh::default();
    for m in &marks {
        let base = mesh.vertices.len() as u32;
        for (dx, dy) in [(-1.5, -1.5), (1.5, -1.5), (1.5, 1.5), (-1.5, 1.5)] {
            mesh.vertices.push(OverlayVertex {
                position: [(m.x + dx) as f32, (m.y + dy) as f32],
                depth: m.depth,
                color: [0.0, 1.0, 0.0, 1.0],
            });
        }
        mesh.indices.extend([0, 1, 2, 0, 2, 3].map(|k| base + k));
    }
    hills.map.set_overlay(mesh);
    hills.map.run_frame().expect("frame");
    let color = hills.read_color();
    let green = |m: &Mark| {
        let p = color[(m.y as u32 * WIDTH + m.x as u32) as usize];
        p[1] > 200 && p[0] < 60 && p[2] < 60
    };
    check_shown(&marks, green);
}

/// Depth probes at ground points answer as the overlay draws them: in sight, or covered.
#[tokio::test]
async fn depth_probes_tell_ground_in_sight_from_ground_behind_hills() {
    use crate::overlay::DepthProbe;
    let (mut hills, marks) = ground_marks().await;
    let probes = marks
        .iter()
        .map(|m| DepthProbe {
            position: [m.x as f32, m.y as f32],
            depth: m.depth,
        })
        .collect();
    let request = hills.map.set_depth_probes(probes);
    let mut answer = None;
    for _ in 0..20 {
        hills.map.run_frame().expect("frame");
        tokio::task::yield_now().await;
        if let Some(results) = hills.map.depth_probe_results() {
            answer = Some(results);
            break;
        }
    }
    let answer = answer.expect("the probes are answered within a few frames");
    assert_eq!(answer.request, request);
    assert_eq!(answer.visible.len(), marks.len());
    let by_mark: Vec<(&Mark, bool)> = marks.iter().zip(answer.visible).collect();
    check_shown(&marks, |m| {
        by_mark
            .iter()
            .find(|(n, _)| std::ptr::eq(*n, m))
            .is_some_and(|(_, seen)| *seen)
    });
}

/// At least 99 % of the marks in sight are shown, and at most 5 % of those well behind
/// hills: a mark on a ridge's silhouette is partly covered, and may show at its centre.
fn check_shown(marks: &[Mark], shown: impl Fn(&Mark) -> bool) {
    let in_sight = marks.iter().filter(|m| m.in_sight).count();
    let well_hidden = marks.iter().filter(|m| m.well_hidden()).count();
    let shown_in_sight = marks.iter().filter(|m| m.in_sight && shown(m)).count();
    let shown_hidden = marks.iter().filter(|m| m.well_hidden() && shown(m)).count();
    assert!(
        shown_in_sight * 100 >= in_sight * 99,
        "{shown_in_sight} of {in_sight} marks in sight are shown"
    );
    assert!(
        shown_hidden * 100 <= well_hidden * 5,
        "of {well_hidden} marks well behind hills, {shown_hidden} show"
    );
}

/// The same marks, answered by the depth probes and by the CPU line of sight.
mod occlusion;
