//! Terrain readback with analytic planes and spheres independent of tile selection.
#![allow(clippy::expect_used, clippy::panic)]

use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use cgmath::{Deg, InnerSpace, Matrix4, SquareMatrix, Vector3};
use image::{Rgba, RgbaImage};

use crate::{
    coords::{LatLon, WorldTileCoords},
    headless::{
        create_headless_renderer_with_settings,
        map::{reference::read_texture_blocking, HeadlessMap, ProcessedLayers},
        HeadlessPlugin,
    },
    render::{
        camera::EyeFrustum,
        eventually::{Eventually, Eventually::Initialized},
        settings::{Msaa, RendererSettings},
        view_state::ExternalAnchor,
        xr::{EyeTarget, ScenePlacement, XrEye, XrFrame},
        RenderPlugin,
    },
    style::Style,
    terrain::{resources::TerrainResources, DefaultDemTransferables, TerrainPlugin},
};

const SIZE: u32 = 128;
const NEAR: f64 = 0.1;
const FAR: f64 = 100_000_000.0;

struct Harness {
    map: HeadlessMap,
    style: Style,
    depth: wgpu::Texture,
    frame: u64,
    capture: Option<PathBuf>,
    depth_format: String,
}

#[derive(serde::Serialize)]
struct FrameSample {
    tiles: Vec<WorldTileCoords>,
    triangles: usize,
    depth_format: String,
    globe_transition: f32,
    flat_nonfinite_elements: usize,
    globe_nonfinite_elements: usize,
    zoom: f64,
    frame_ms: f64,
    readback_ms: f64,
    #[serde(skip)]
    rgba: Vec<u8>,
    #[serde(skip)]
    depth: Vec<f64>,
}

impl Harness {
    async fn new(projection: &str, height: i32) -> Self {
        Self::with_exaggeration(projection, height, 1.0).await
    }

    async fn with_exaggeration(projection: &str, height: i32, exaggeration: f64) -> Self {
        Self::with_style(terrain_style(projection, exaggeration), height).await
    }

    async fn with_style(style: Style, height: i32) -> Self {
        let settings = RendererSettings {
            msaa: Msaa { samples: 1 },
            ..Default::default()
        };
        let (kernel, renderer) = create_headless_renderer_with_settings(SIZE, SIZE, None, settings)
            .await
            .expect("renderer");
        let depth_format = format!("{:?}", renderer.settings.depth_texture_format);
        let mut map = HeadlessMap::new(
            style.clone(),
            renderer,
            kernel,
            vec![
                Box::new(RenderPlugin),
                Box::new(TerrainPlugin::<DefaultDemTransferables>::default()),
                // A sea-level background would conceal holes in the terrain mesh.
                Box::new(
                    HeadlessPlugin::new(false)
                        .preserve_tile_sources()
                        .retain_supplied_tiles(),
                ),
            ],
        )
        .expect("map");
        map.render_frames_with_terrain(
            ProcessedLayers::default(),
            vec![],
            vec![(WorldTileCoords::default(), constant_dem(height))],
            1,
        )
        .expect("DEM upload");
        let depth = depth_target(map.device());
        Self {
            map,
            style,
            depth,
            frame: 0,
            depth_format,
            capture: std::env::var_os("MAPLIBRE_TEST_CAPTURE_DIR").map(PathBuf::from),
        }
    }

    fn render_blocking(
        &mut self,
        name: &str,
        anchor: LatLon,
        pose: Matrix4<f64>,
        fov: f64,
    ) -> FrameSample {
        let start = Instant::now();
        self.draw(anchor, pose, fov);
        let frame_ms = start.elapsed().as_secs_f64() * 1000.0;
        let start = Instant::now();
        let rgba = read_texture_blocking(
            &self.map,
            self.map.head_texture().expect("color"),
            wgpu::TextureAspect::All,
        )
        .expect("color readback");
        let depth = read_texture_blocking(&self.map, &self.depth, wgpu::TextureAspect::DepthOnly)
            .expect("depth readback")
            .chunks_exact(4)
            .map(metric_depth)
            .collect();
        let readback_ms = start.elapsed().as_secs_f64() * 1000.0;
        let Some(Initialized(terrain)) = self
            .map
            .world()
            .resources
            .get::<Eventually<TerrainResources>>()
        else {
            panic!("terrain resources")
        };
        let tiles: Vec<_> = terrain.draws().iter().map(|draw| draw.coords).collect();
        let triangles = tiles.len()
            * super::create_terrain_mesh(super::TERRAIN_MESH_SIZE)
                .indices
                .len()
            / 3;
        let view = self.map.view_state();
        let projection = crate::render::projection::projection_data_for_view(&self.style, view)
            .expect("actual shader projection data");
        let expected = self
            .style
            .projection
            .as_ref()
            .expect("projection")
            .projection_type
            .globe_transition(view.zoom().value());
        assert_eq!(projection.transition, expected);
        assert!(
            expected == 0.0 || expected == 1.0,
            "analytic oracle requires fixed projection"
        );
        let flat = view.gpu_view_projection().0;
        self.capture(name, &rgba);
        FrameSample {
            tiles,
            triangles,
            depth_format: self.depth_format.clone(),
            globe_transition: projection.transition,
            flat_nonfinite_elements: (0..4)
                .flat_map(|i| (0..4).map(move |j| flat[i][j]))
                .filter(|v| !v.is_finite())
                .count(),
            globe_nonfinite_elements: projection
                .main_matrix
                .iter()
                .flatten()
                .filter(|v| !v.is_finite())
                .count(),
            zoom: view.zoom().value(),
            frame_ms,
            readback_ms,
            rgba,
            depth,
        }
    }

    fn draw(&mut self, anchor: LatLon, pose: Matrix4<f64>, fov: f64) {
        self.frame = self.frame.wrapping_add(1);
        self.map
            .run_xr_frame(XrFrame {
                opaque_environment: false,
                timestamp: Duration::from_millis(self.frame.wrapping_mul(16)),
                placement: ScenePlacement {
                    anchor: ExternalAnchor {
                        position: anchor,
                        altitude_meters: 0.0,
                    },
                    world_from_scene: Matrix4::identity(),
                },
                eyes: vec![XrEye {
                    world_from_eye: pose,
                    frustum: EyeFrustum::symmetric(Deg(fov).into(), 1.0, NEAR, FAR),
                    target: EyeTarget {
                        color: None,
                        depth: Some(self.depth.create_view(&Default::default())),
                    },
                }],
                request_overscan: 1.0,
                prefetch: None,
            })
            .expect("terrain frame");
    }

    fn capture(&self, name: &str, rgba: &[u8]) {
        if let Some(directory) = &self.capture {
            std::fs::create_dir_all(directory).expect("capture directory");
            image::save_buffer(
                directory.join(format!("{name}.png")),
                rgba,
                SIZE,
                SIZE,
                image::ColorType::Rgba8,
            )
            .expect("capture");
        }
    }

    fn record(&self, name: &str, records: &[FrameSample]) {
        if let Some(directory) = &self.capture {
            std::fs::create_dir_all(directory).expect("capture directory");
            std::fs::write(
                directory.join(format!("{name}.json")),
                serde_json::to_vec_pretty(records).expect("metrics JSON"),
            )
            .expect("metrics");
        }
    }
}

fn terrain_style(projection: &str, exaggeration: f64) -> Style {
    serde_json::from_value(serde_json::json!({
        "version":8,"center":[0,0],"zoom":2,
        "sources":{"dem":{"type":"raster-dem","tiles":["offline://dem"],"encoding":"terrarium"}},
        "layers":[{"id":"background","type":"background","paint":{"background-color":"#20b060"}}],
        "terrain":{"source":"dem","exaggeration":exaggeration},
        "projection":{"type":if projection == "globe" {"vertical-perspective"} else {projection}}
    }))
    .expect("terrain style")
}

fn constant_dem(height: i32) -> RgbaImage {
    let encoded = u32::try_from(height + 32768).expect("Terrarium elevation");
    RgbaImage::from_pixel(32, 32, Rgba([(encoded >> 8) as u8, encoded as u8, 0, 255]))
}

fn depth_target(device: &wgpu::Device) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("terrain mesh depth oracle"),
        size: wgpu::Extent3d {
            width: SIZE,
            height: SIZE,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth32Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

fn metric_depth(bytes: &[u8]) -> f64 {
    let raw = f64::from(f32::from_le_bytes(bytes.try_into().expect("depth sample")));
    assert!(
        raw.is_finite() && (0.0..=1.0).contains(&raw),
        "invalid GPU depth {raw}"
    );
    if raw == 0.0 {
        0.0
    } else {
        NEAR * FAR / (NEAR + raw * (FAR - NEAR))
    }
}

fn pose(altitude: f64, pitch: f64, bearing: f64) -> Matrix4<f64> {
    Matrix4::from_translation(Vector3::new(0.0, 0.0, altitude))
        * Matrix4::from_angle_z(Deg(bearing))
        * Matrix4::from_angle_x(Deg(pitch))
}

fn ray(pose: Matrix4<f64>, fov: f64, x: u32, y: u32) -> (Vector3<f64>, Vector3<f64>) {
    ray_position(pose, fov, f64::from(x) + 0.5, f64::from(y) + 0.5)
}

fn ray_position(pose: Matrix4<f64>, fov: f64, x: f64, y: f64) -> (Vector3<f64>, Vector3<f64>) {
    let tangent = (fov.to_radians() * 0.5).tan();
    let direction = Vector3::new(
        (2.0 * x / f64::from(SIZE) - 1.0) * tangent,
        (1.0 - 2.0 * y / f64::from(SIZE)) * tangent,
        -1.0,
    );
    (pose.w.truncate(), (pose * direction.extend(0.0)).truncate())
}

fn sphere_depth(
    origin: Vector3<f64>,
    direction: Vector3<f64>,
    radius: f64,
    height: f64,
) -> Option<f64> {
    let offset = origin + Vector3::new(0.0, 0.0, radius);
    let b = offset.dot(direction);
    let a = direction.magnitude2();
    let c = offset.magnitude2() - (radius + height).powi(2);
    let discriminant = b * b - a * c;
    if discriminant <= 0.0 {
        return None;
    }
    let t = c / (-b + discriminant.sqrt());
    (NEAR..=FAR).contains(&t).then_some(t)
}

mod route;
mod seams;
mod sphere;
mod tests;
