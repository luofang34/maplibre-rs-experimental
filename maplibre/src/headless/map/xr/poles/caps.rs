//! The polar caps close the terrain globe with the style's background colour, not with the
//! drape's last row stretched to the pole, and are real geometry with depth however the pole
//! is seen.
use cgmath::{Deg, InnerSpace};

use super::*;

const SIZE: u32 = 256;
/// The fill: the style's background colour.
const FILL: [u8; 3] = [0, 102, 0];

/// Where the eye looks from: the latitude the scene is anchored at, the eye's distance above
/// that point, how far it is tilted away from straight down and in which direction.
#[derive(Clone, Copy, Debug)]
struct View {
    latitude: f64,
    distance: f64,
    tilt: f64,
    bearing: f64,
}

impl View {
    /// The eye looks at the pole, which is the scene's origin, so the pole stays at the
    /// centre of the image.
    fn world_from_eye(self) -> Matrix4<f64> {
        Matrix4::from_angle_z(Deg(self.bearing))
            * Matrix4::from_angle_x(Deg(self.tilt))
            * Matrix4::from_translation(Vector3::new(0.0, 0.0, self.distance))
    }

    /// Where the eye's ray through pixel `(x, y)` meets the globe: the distance along the view
    /// axis, the colatitude in degrees, and how squarely the ray meets the surface (the cosine
    /// of its angle to the surface normal).
    fn surface_at(self, x: u32, y: u32) -> Option<(f64, f64, f64)> {
        let half = (0.5_f64).tan();
        let ndc = [
            (f64::from(x) + 0.5) / f64::from(SIZE) * 2.0 - 1.0,
            1.0 - (f64::from(y) + 0.5) / f64::from(SIZE) * 2.0,
        ];
        let world_from_eye = self.world_from_eye();
        let eye = world_from_eye * cgmath::Vector4::new(0.0, 0.0, 0.0, 1.0);
        // A direction whose component along the view axis is 1, so the ray parameter is the
        // distance along that axis, the quantity depth encodes.
        let direction =
            world_from_eye * cgmath::Vector4::new(ndc[0] * half, ndc[1] * half, -1.0, 0.0);
        let centre = cgmath::Vector3::new(0.0, 0.0, -RADIUS);
        let from_centre = eye.truncate() - centre;
        let direction = direction.truncate();
        let a = direction.dot(direction);
        let b = 2.0 * from_centre.dot(direction);
        let c = from_centre.dot(from_centre) - RADIUS * RADIUS;
        let discriminant = b * b - 4.0 * a * c;
        if discriminant < 0.0 {
            return None;
        }
        let along = (-b - discriminant.sqrt()) / (2.0 * a);
        let hit = from_centre + direction * along;
        // The scene's axes point east, north and up at the anchor, so the pole the anchor is
        // nearest lies along this direction from the centre.
        let latitude = self.latitude.to_radians();
        let pole =
            cgmath::Vector3::new(0.0, latitude.cos(), latitude.sin()) * self.latitude.signum();
        let colatitude = (hit.dot(pole) / hit.magnitude())
            .clamp(-1.0, 1.0)
            .acos()
            .to_degrees();
        let incidence = -hit.dot(direction) / (hit.magnitude() * direction.magnitude());
        Some((along, colatitude, incidence))
    }
}

/// The sphere the scene's metres measure.
const RADIUS: f64 = crate::projection::globe::EARTH_RADIUS_METERS;
/// Degrees from the pole to the last row of tiles, at the Mercator latitude limit.
const CAP_COLATITUDE: f64 = 90.0 - 85.051_128_779_806_59;
/// Degrees on either side of the seam left to antialiasing.
const SEAM_MARGIN: f64 = 0.2;
/// How far the drawn cap may lie from the sphere: its flat triangles sag a few kilometres
/// below the arc, while the far side of the globe seen through a hole is thousands away.
const CHORD_TOLERANCE: f64 = 30_000.0;
/// Degrees beyond the seam in which every tile pixel must keep its drape, so the cap's fill
/// stops at the last row of tiles instead of spreading over it.
const TILE_RING: f64 = 2.5;
const NEAR: f64 = 1000.0;
const FAR: f64 = 1e9;

/// The distance along the view axis that a reversed depth value stands for.
fn distance_from_depth(depth: f32) -> f64 {
    NEAR * FAR / (f64::from(depth) * (FAR - NEAR) + NEAR)
}

/// Serves every elevation tile as flat ground at sea level, so terrain refines as it would
/// with real data while the cap and tiles stay at a known height.
#[derive(Clone)]
struct FlatDem;

#[cfg_attr(not(feature = "thread-safe-futures"), async_trait::async_trait(?Send))]
#[cfg_attr(feature = "thread-safe-futures", async_trait::async_trait)]
impl crate::io::source_client::HttpClient for FlatDem {
    async fn fetch(
        &self,
        _url: &str,
    ) -> Result<Vec<u8>, crate::io::source_client::SourceFetchError> {
        let mut png = std::io::Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(256, 256, image::Rgba([128, 0, 0, 255]))
            .write_to(&mut png, image::ImageFormat::Png)
            .expect("PNG");
        Ok(png.into_inner())
    }
}

struct PolarMap {
    map: HeadlessMap,
    depth: wgpu::Texture,
    timestamp: u64,
}

impl PolarMap {
    async fn new() -> Self {
        let style: Style = serde_json::from_str(r##"{"version":8,"sources":{"dem":{"type":"raster-dem","tiles":["https://dem.example/{z}/{x}/{y}.png"],"encoding":"terrarium"}},"layers":[{"id":"background","type":"background","paint":{"background-color":"#006600"}}],"terrain":{"source":"dem"},"projection":{"type":"globe"}}"##).expect("style");
        let (kernel, renderer) = crate::headless::create_headless_renderer_with_loader(
            SIZE,
            SIZE,
            Default::default(),
            crate::io::resource_loader::SharedLoader::new(FlatDem),
        )
        .await
        .expect("renderer");
        let map = HeadlessMap::new(
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
        let depth = map.device().create_texture(&wgpu::TextureDescriptor {
            label: Some("polar cap depth"),
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
        });
        Self {
            map,
            depth,
            timestamp: 0,
        }
    }

    /// Draws `count` frames, letting elevation requests finish in between.
    async fn frames(&mut self, view: View, count: usize) {
        for _ in 0..count {
            self.frame(view);
            for _ in 0..8 {
                tokio::task::yield_now().await;
            }
        }
    }

    fn frame(&mut self, view: View) {
        self.timestamp += 16;
        let frame = XrFrame {
            opaque_environment: false,
            timestamp: Duration::from_millis(self.timestamp),
            placement: ScenePlacement {
                anchor: ExternalAnchor {
                    position: LatLon::new(view.latitude, 11.0),
                    altitude_meters: 0.0,
                },
                world_from_scene: Matrix4::identity(),
            },
            eyes: vec![XrEye {
                world_from_eye: view.world_from_eye(),
                frustum: EyeFrustum::symmetric(Rad(1.0), 1.0, NEAR, FAR),
                target: EyeTarget {
                    color: None,
                    depth: Some(self.depth.create_view(&Default::default())),
                },
            }],
            request_overscan: 1.0,
            prefetch: None,
        };
        self.map.run_xr_frame(frame).expect("polar frame");
    }

    /// Paints every drape white, so drape stretched into the cap would show as white.
    fn whiten_drapes(&self) {
        use crate::{
            render::eventually::{Eventually, Eventually::Initialized},
            terrain::resources::TerrainResources,
        };
        let Some(Initialized(terrain)) = self
            .map
            .world()
            .resources
            .get::<Eventually<TerrainResources>>()
        else {
            panic!("terrain");
        };
        assert!(!terrain.draws().is_empty(), "terrain is drawn");
        for draw in terrain.draws() {
            if let Some(texture) = terrain.drape_texture(draw.coords) {
                clear_drape_white(&self.map, &texture.texture);
            }
        }
    }

    /// Checks every pixel whose ray meets the cap, up to the seam: drawn with the fill at the
    /// cap's own depth, so the far side of the globe seen through a hole cannot pass. Returns
    /// how many white drape pixels the tiles show.
    fn check(&self, view: View, drapes_white: bool, case: &str) -> usize {
        let colors = read_blocking(&self.map, self.map.head_texture().expect("color"));
        let depths = read_blocking(&self.map, &self.depth);
        let mut cap = 0;
        let mut white = 0;
        let mut ring = 0;
        for y in 0..SIZE {
            for x in 0..SIZE {
                let offset = ((y * SIZE + x) * 4) as usize;
                let pixel = &colors[offset..offset + 4];
                let Some((along, colatitude, incidence)) = view.surface_at(x, y) else {
                    continue;
                };
                // The cap is a fan of flat triangles, chords under the sphere, so along its
                // silhouette the sphere and the drawn cap part ways.
                if incidence < 0.25 {
                    continue;
                }
                if colatitude > CAP_COLATITUDE + SEAM_MARGIN {
                    let is_white = pixel[..3].iter().all(|channel| *channel > 220);
                    white += usize::from(is_white);
                    if drapes_white && colatitude < CAP_COLATITUDE + TILE_RING {
                        ring += 1;
                        assert!(
                            is_white,
                            "{case}: the last row of tiles at ({x},{y}) {colatitude:.3}° from \
                             the pole is {pixel:?}, not its drape"
                        );
                    }
                    continue;
                }
                if colatitude > CAP_COLATITUDE - SEAM_MARGIN {
                    continue;
                }
                cap += 1;
                let depth =
                    f32::from_le_bytes(depths[offset..offset + 4].try_into().expect("depth"));
                let distance = distance_from_depth(depth);
                assert!(
                    (distance - along).abs() < CHORD_TOLERANCE,
                    "{case}: ({x},{y}) {colatitude:.3}° from the pole lies at {distance:.0} m, \
                     not on the cap at {along:.0} m"
                );
                assert!(
                    pixel[3] == 255 && pixel[..3].iter().zip(FILL).all(|(a, b)| a.abs_diff(b) <= 3),
                    "{case}: the cap at ({x},{y}) {colatitude:.3}° from the pole is {pixel:?}, \
                     not the fill {FILL:?}"
                );
            }
        }
        assert!(cap > 200, "{case}: {cap} cap pixels checked");
        assert!(
            !drapes_white || ring > 200,
            "{case}: {ring} pixels of the last row of tiles checked"
        );
        if drapes_white {
            white
        } else {
            0
        }
    }

    /// The finest zoom the terrain draws.
    fn finest_zoom(&self) -> u8 {
        use crate::{
            render::eventually::{Eventually, Eventually::Initialized},
            terrain::resources::TerrainResources,
        };
        match self
            .map
            .world()
            .resources
            .get::<Eventually<TerrainResources>>()
        {
            Some(Initialized(terrain)) => terrain
                .draws()
                .iter()
                .map(|draw| u8::from(draw.coords.z))
                .max()
                .unwrap_or(0),
            _ => 0,
        }
    }
}

#[tokio::test]
async fn polar_caps_take_the_fill_with_depth_from_every_view_and_level_of_detail() {
    for pole in [-1.0, 1.0] {
        let above = |tilt, bearing| View {
            latitude: 89.999999 * pole,
            distance: 3_000_000.0,
            tilt,
            bearing,
        };
        // Low over the last row of tiles, looking at the pole, the tiles there refine.
        let near = View {
            latitude: 85.0 * pole,
            distance: 150_000.0,
            tilt: 30.0,
            bearing: if pole < 0.0 { 180.0 } else { 0.0 },
        };
        let views = [
            above(0.0, 0.0),
            above(55.0, 0.0),
            above(55.0, 90.0),
            above(55.0, 180.0),
            near,
        ];
        let mut map = PolarMap::new().await;
        let mut zooms: Vec<u8> = Vec::new();
        for (index, view) in views.into_iter().enumerate() {
            let case = format!("{view:?}");
            map.frame(view);
            if index == 0 || index == views.len() - 1 {
                // While the tiles of the new view load, before their drapes are drawn.
                map.check(view, false, &format!("{case} loading"));
            }
            map.frames(view, 12).await;
            map.whiten_drapes();
            map.frame(view);
            let white = map.check(view, true, &case);
            assert!(
                white > 100,
                "{case}: the tiles keep their drape: {white} white pixels"
            );
            zooms.push(map.finest_zoom());
        }
        assert!(
            zooms.last().is_some_and(|zoom| *zoom >= 2) && zooms.first() == Some(&0),
            "{pole}: the near view switches to finer tiles: {zooms:?}"
        );
        // A map allowed no drape draws every tile without one.
        let mut bare = PolarMap::new().await;
        bare.map
            .map_context
            .world
            .resources
            .insert(crate::terrain::DrapeBudget {
                per_frame: 0,
                per_eye_frame: 0,
                ..Default::default()
            });
        for view in [above(0.0, 0.0), above(55.0, 90.0), near] {
            bare.frames(view, 4).await;
            bare.check(view, false, &format!("{view:?} without drapes"));
        }
    }
}
