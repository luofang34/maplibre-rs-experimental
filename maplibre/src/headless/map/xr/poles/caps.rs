//! The polar caps close the terrain globe with the style's background colour, not with the
//! drape's last row stretched to the pole, and are real geometry with depth however the pole
//! is seen.
use cgmath::Deg;

use super::*;

const SIZE: u32 = 256;
/// The fill: the style's background colour.
const FILL: [u8; 3] = [0, 102, 0];

/// Where the eye looks at the pole from: its distance above it, how far it is tilted away
/// from straight down and in which direction.
#[derive(Clone, Copy, Debug)]
struct View {
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

    /// Pixels from the centre that the cap covers whatever the tilt: it reaches about five
    /// degrees from the pole, foreshortened by the tilt.
    fn cap_radius(self) -> f64 {
        let cap = (550_000.0 / self.distance).atan().tan() / (0.5_f64).tan();
        cap * f64::from(SIZE) / 2.0 * self.tilt.to_radians().cos() * 0.6
    }
}

struct PolarMap {
    map: HeadlessMap,
    depth: wgpu::Texture,
    latitude: f64,
    timestamp: u64,
}

impl PolarMap {
    async fn new(latitude: f64) -> Self {
        let style: Style = serde_json::from_str(r##"{"version":8,"sources":{"dem":{"type":"raster-dem","tiles":["https://dem.example/{z}/{x}/{y}.png"],"encoding":"terrarium"}},"layers":[{"id":"background","type":"background","paint":{"background-color":"#006600"}}],"terrain":{"source":"dem"},"projection":{"type":"globe"}}"##).expect("style");
        let (kernel, renderer) = create_headless_renderer(SIZE, SIZE, None)
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
            latitude,
            timestamp: 0,
        }
    }

    fn frame(&mut self, view: View) {
        self.timestamp += 16;
        let frame = XrFrame {
            opaque_environment: false,
            timestamp: Duration::from_millis(self.timestamp),
            placement: ScenePlacement {
                anchor: ExternalAnchor {
                    position: LatLon::new(self.latitude, 11.0),
                    altitude_meters: 0.0,
                },
                world_from_scene: Matrix4::identity(),
            },
            eyes: vec![XrEye {
                world_from_eye: view.world_from_eye(),
                frustum: EyeFrustum::symmetric(Rad(1.0), 1.0, 1000.0, 1e9),
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

    /// Checks every pixel within the cap: drawn with depth, in the fill colour. Returns how
    /// many white drape pixels the image has elsewhere.
    fn check(&self, view: View, drapes_white: bool, case: &str) -> usize {
        let colors = read_blocking(&self.map, self.map.head_texture().expect("color"));
        let depths = read_blocking(&self.map, &self.depth);
        let radius = view.cap_radius();
        let centre = f64::from(SIZE) / 2.0;
        let mut checked = 0;
        for y in 0..SIZE {
            for x in 0..SIZE {
                let (dx, dy) = (f64::from(x) + 0.5 - centre, f64::from(y) + 0.5 - centre);
                if dx.hypot(dy) > radius {
                    continue;
                }
                checked += 1;
                let offset = ((y * SIZE + x) * 4) as usize;
                let depth =
                    f32::from_le_bytes(depths[offset..offset + 4].try_into().expect("depth"));
                assert!(
                    depth > 0.0 && depth < 1.0,
                    "{case}: no surface at ({x},{y}): depth {depth}"
                );
                let pixel = &colors[offset..offset + 4];
                assert!(
                    pixel[3] == 255 && pixel[..3].iter().zip(FILL).all(|(a, b)| a.abs_diff(b) <= 3),
                    "{case}: the cap at ({x},{y}) is {pixel:?}, not the fill {FILL:?}"
                );
            }
        }
        assert!(
            checked > 300,
            "{case}: {checked} cap pixels checked, radius {radius}"
        );
        if !drapes_white {
            return 0;
        }
        colors
            .chunks_exact(4)
            .filter(|pixel| pixel[..3].iter().all(|channel| *channel > 220))
            .count()
    }
}

#[tokio::test]
async fn polar_caps_take_the_fill_with_depth_from_every_view_and_level_of_detail() {
    let views = [
        View {
            distance: 3_000_000.0,
            tilt: 0.0,
            bearing: 0.0,
        },
        View {
            distance: 3_000_000.0,
            tilt: 55.0,
            bearing: 0.0,
        },
        View {
            distance: 3_000_000.0,
            tilt: 55.0,
            bearing: 90.0,
        },
        View {
            distance: 3_000_000.0,
            tilt: 55.0,
            bearing: 180.0,
        },
        // Closer, the pole's tiles refine.
        View {
            distance: 1_500_000.0,
            tilt: 30.0,
            bearing: 45.0,
        },
    ];
    for latitude in [-89.999999, 89.999999] {
        let mut map = PolarMap::new(latitude).await;
        let first = views[0];
        // While the tiles load, before any drape is drawn.
        map.frame(first);
        map.check(first, false, &format!("{latitude} loading"));
        for view in views {
            let case = format!("{latitude} {view:?}");
            map.frame(view);
            map.frame(view);
            map.whiten_drapes();
            map.frame(view);
            let white = map.check(view, true, &case);
            assert!(
                white > 100,
                "{case}: the tiles keep their drape: {white} white pixels"
            );
        }
    }
}
