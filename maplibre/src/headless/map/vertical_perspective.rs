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

/// Serves every elevation tile as level ground at one height, encoded as Terrarium.
#[derive(Clone, Copy)]
struct LevelDem {
    meters: f64,
}

#[cfg_attr(not(feature = "thread-safe-futures"), async_trait::async_trait(?Send))]
#[cfg_attr(feature = "thread-safe-futures", async_trait::async_trait)]
impl HttpClient for LevelDem {
    async fn fetch(&self, _url: &str) -> Result<Vec<u8>, SourceFetchError> {
        let encoded = ((self.meters + 32768.0) * 256.0).round() as u32;
        let pixel = image::Rgba([
            (encoded >> 16) as u8,
            (encoded >> 8) as u8,
            encoded as u8,
            255,
        ]);
        let mut png = std::io::Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(256, 256, pixel)
            .write_to(&mut png, image::ImageFormat::Png)
            .expect("PNG");
        Ok(png.into_inner())
    }
}

/// A view of the scene the vertical-perspective terrain path is checked in.
#[derive(Clone, Copy, Debug)]
struct Scene {
    meters: f64,
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
            r##"{{"version":8,"center":[88.05464359004759,27.765393137835165],"zoom":{zoom},"pitch":{pitch},"bearing":324.03714296701236,"sources":{{"dem":{{"type":"raster-dem","tiles":["https://dem.example/{{z}}/{{x}}/{{y}}.png"],"encoding":"terrarium"}}}},"layers":[{{"id":"background","type":"background","paint":{{"background-color":"#336699"}}}}],"terrain":{{"source":"dem","exaggeration":{exaggeration}}},"projection":{{"type":"vertical-perspective"}}}}"##,
            zoom = scene.zoom,
            pitch = scene.pitch,
            exaggeration = scene.exaggeration,
        );
        let style: Style = serde_json::from_str(&style).expect("style");
        let (kernel, renderer) = crate::headless::create_headless_renderer_with_loader(
            WIDTH,
            HEIGHT,
            Default::default(),
            crate::io::resource_loader::SharedLoader::new(LevelDem {
                meters: scene.meters,
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

    /// Draws until the center stands on the loaded terrain and two depth readbacks agree, so
    /// the check sees the settled frame rather than one still loading.
    async fn settle(&mut self, expected_center: f64) -> Vec<f32> {
        let mut previous: Option<Vec<f32>> = None;
        for _ in 0..40 {
            for _ in 0..4 {
                self.map.run_frame().expect("frame");
                for _ in 0..8 {
                    tokio::task::yield_now().await;
                }
            }
            let depth = self.read_depth();
            let centered = (self.map.view_state().center_elevation() - expected_center).abs() < 0.5;
            if centered && previous.as_ref() == Some(&depth) {
                return depth;
            }
            previous = Some(depth);
        }
        panic!(
            "the frame never settled: center at {} m, expected {expected_center} m",
            self.map.view_state().center_elevation()
        );
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
    let depth = level.settle(ground).await;
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
                exaggeration,
                pitch,
                zoom: 11.67,
            })
            .await;
        }
    }
}
