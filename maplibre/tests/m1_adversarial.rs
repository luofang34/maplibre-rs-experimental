#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    f64::consts::PI,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
};

use cgmath::Vector3;
use maplibre::{
    coords::LatLon,
    render::{
        tracked_pass::TrackedRenderPass, PassDrawHook, PassDrawHooks, PassHookPoint, SceneLighting,
    },
    tcs::world::World,
};

// -----------------------------------------------------------------------------
// AREA 1: PassDrawHooks Adversarial Tests
// -----------------------------------------------------------------------------

#[test]
fn test_pass_draw_hooks_empty_noop() {
    let hooks = PassDrawHooks::new();
    assert!(hooks.get_hooks(PassHookPoint::MainPassBegin).is_empty());
    assert!(hooks
        .get_hooks(PassHookPoint::TranslucentPassBegin)
        .is_empty());

    let world = World::default();
    let pass_hooks = world
        .resources
        .get::<PassDrawHooks>()
        .map(|h| h.get_hooks(PassHookPoint::MainPassBegin));
    assert!(pass_hooks.is_none());

    let mut world = World::default();
    world.resources.insert(PassDrawHooks::new());
    let pass_hooks = world
        .resources
        .get::<PassDrawHooks>()
        .map(|h| h.get_hooks(PassHookPoint::MainPassBegin));
    assert!(pass_hooks.is_some());
    assert!(pass_hooks.unwrap().is_empty());
}

struct OrderHook {
    id: usize,
    log: Arc<Mutex<Vec<usize>>>,
}

impl PassDrawHook for OrderHook {
    fn draw<'w>(&self, _pass: &mut TrackedRenderPass<'w>, _world: &'w World) {
        self.log.lock().unwrap().push(self.id);
    }
}

#[derive(Default, Debug, PartialEq)]
struct CustomTestResource(u32);

struct QueryHook {
    expected_value: u32,
    executed: Arc<AtomicUsize>,
}

impl PassDrawHook for QueryHook {
    fn draw<'w>(&self, _pass: &mut TrackedRenderPass<'w>, world: &'w World) {
        let res = world.resources.get::<CustomTestResource>();
        assert_eq!(res, Some(&CustomTestResource(self.expected_value)));
        let hooks_res = world.resources.get::<PassDrawHooks>();
        assert!(hooks_res.is_some());
        self.executed.fetch_add(1, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn test_pass_draw_hooks_execution_order_and_borrow_safety() {
    let instance = wgpu::Instance::default();
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions::default())
        .await
        .expect("GPU adapter");
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor::default())
        .await
        .expect("GPU device");
    let target = device
        .create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: 4,
                height: 4,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default());

    let mut world = World::default();
    world.resources.insert(CustomTestResource(42));

    let log = Arc::new(Mutex::new(Vec::new()));
    let executed = Arc::new(AtomicUsize::new(0));

    let mut hooks = PassDrawHooks::new();
    for id in [1, 2, 3] {
        hooks.register(
            PassHookPoint::MainPassBegin,
            OrderHook {
                id,
                log: log.clone(),
            },
        );
    }
    hooks.register(
        PassHookPoint::MainPassBegin,
        QueryHook {
            expected_value: 42,
            executed: executed.clone(),
        },
    );
    hooks.register(
        PassHookPoint::TranslucentPassBegin,
        OrderHook {
            id: 999,
            log: log.clone(),
        },
    );
    world.resources.insert(hooks);

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    let render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("adversarial test pass"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &target,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    let mut tracked = TrackedRenderPass::new(render_pass);

    let pass_hooks = world
        .resources
        .get::<PassDrawHooks>()
        .map(|hooks| hooks.get_hooks(PassHookPoint::MainPassBegin));
    if let Some(hooks) = pass_hooks {
        for hook in hooks {
            hook.draw(&mut tracked, &world);
        }
    }

    assert_eq!(*log.lock().unwrap(), vec![1, 2, 3]);
    assert_eq!(executed.load(Ordering::SeqCst), 1);

    drop(tracked);
    queue.submit(Some(encoder.finish()));
}

#[test]
fn test_pass_draw_hooks_repeated_registration() {
    let mut hooks = PassDrawHooks::new();
    let counter = Arc::new(AtomicUsize::new(0));

    struct SimpleHook(Arc<AtomicUsize>);
    impl PassDrawHook for SimpleHook {
        fn draw<'w>(&self, _pass: &mut TrackedRenderPass<'w>, _world: &'w World) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    for _ in 0..50 {
        hooks.register(PassHookPoint::MainPassBegin, SimpleHook(counter.clone()));
    }
    assert_eq!(hooks.get_hooks(PassHookPoint::MainPassBegin).len(), 50);
}

// -----------------------------------------------------------------------------
// AREA 2: SceneLighting Adversarial Tests
// -----------------------------------------------------------------------------

#[test]
fn test_scene_lighting_daytime_emulation() {
    let default_lighting = SceneLighting::default();
    assert_eq!(default_lighting.sun_direction, None);
    assert_eq!(default_lighting.daylight_uniform(), [0.0; 4]);
    assert_eq!(default_lighting.sun_visible(), 1.0);
    assert_eq!(default_lighting.twilight, [0.0, 0.0, 0.15, 1.0]);
    assert_eq!(default_lighting.moonlight, [0.0; 4]);
    assert!(!default_lighting.atmosphere);
    // In terrain shader (terrain.fragment.wgsl:53): w < 0.5 emulates daytime
    assert!(default_lighting.daylight_uniform()[3] < 0.5);
}

#[test]
fn test_scene_lighting_daylight_uniform_variations() {
    let lighting_none = SceneLighting {
        sun_direction: None,
        ..Default::default()
    };
    assert_eq!(lighting_none.daylight_uniform(), [0.0; 4]);

    let dirs = [
        Vector3::new(1.0, 0.0, 0.0),
        Vector3::new(0.0, 1.0, 0.0),
        Vector3::new(0.0, 0.0, 1.0),
        Vector3::new(-1.0, 0.0, 0.0),
        Vector3::new(
            1.0 / 3.0f64.sqrt(),
            1.0 / 3.0f64.sqrt(),
            1.0 / 3.0f64.sqrt(),
        ),
    ];

    for dir in dirs {
        let lighting = SceneLighting {
            sun_direction: Some(dir),
            ..Default::default()
        };
        let u = lighting.daylight_uniform();
        assert!((u[0] - dir.x as f32).abs() < 1e-6);
        assert!((u[1] - dir.y as f32).abs() < 1e-6);
        assert!((u[2] - dir.z as f32).abs() < 1e-6);
        assert_eq!(u[3], 1.0);
    }

    let extreme = SceneLighting {
        sun_direction: Some(Vector3::new(f64::MAX, f64::MIN, 0.0)),
        ..Default::default()
    };
    assert_eq!(
        extreme.daylight_uniform(),
        [f32::INFINITY, f32::NEG_INFINITY, 0.0, 1.0]
    );

    let nan_vec = SceneLighting {
        sun_direction: Some(Vector3::new(f64::NAN, f64::INFINITY, f64::NEG_INFINITY)),
        ..Default::default()
    };
    let u = nan_vec.daylight_uniform();
    assert!(u[0].is_nan() && u[1] == f32::INFINITY && u[2] == f32::NEG_INFINITY && u[3] == 1.0);

    let subnormal = SceneLighting {
        sun_direction: Some(Vector3::new(1e-320, -1e-320, 0.0)),
        ..Default::default()
    };
    assert_eq!(subnormal.daylight_uniform(), [0.0, 0.0, 0.0, 1.0]);

    let zero = SceneLighting {
        sun_direction: Some(Vector3::new(0.0, 0.0, 0.0)),
        ..Default::default()
    };
    assert_eq!(zero.daylight_uniform(), [0.0, 0.0, 0.0, 1.0]);
}

#[test]
fn test_scene_lighting_twilight_sun_visible() {
    let custom = SceneLighting {
        twilight: [0.1, 0.2, 0.3, 0.75],
        ..Default::default()
    };
    assert_eq!(custom.sun_visible(), 0.75);
}

// -----------------------------------------------------------------------------
// AREA 3: LatLon Mercator Coordinate Math Adversarial Tests
// -----------------------------------------------------------------------------

#[test]
fn test_latlon_polar_boundaries() {
    let web_mercator_max_lat = (PI.sinh().atan()).to_degrees(); // ~85.05112877980659
    assert!((web_mercator_max_lat - 85.05112877980659).abs() < 1e-12);

    let north_limit = LatLon::new(web_mercator_max_lat, 0.0);
    let north_merc = north_limit.to_mercator();
    assert!(north_merc.y >= 0.0 && north_merc.y < 1e-8);
    let north_back = LatLon::from_mercator(north_merc);
    assert!((north_back.latitude - web_mercator_max_lat).abs() < 1e-8);

    let south_limit = LatLon::new(-web_mercator_max_lat, 0.0);
    let south_merc = south_limit.to_mercator();
    assert!(south_merc.y <= 1.0 && south_merc.y >= 1.0 - 1e-8);
    let south_back = LatLon::from_mercator(south_merc);
    let diff = (south_back.latitude - (-web_mercator_max_lat)).abs();
    assert!(diff < 1e-7); // clamped to 1.0 - 1e-9 -> delta ~3.1e-8 deg

    for lat in [85.05112878, 85.051129, 86.0, 89.0, 89.999999, 90.0] {
        let pt = LatLon::new(lat, 0.0);
        let merc = pt.to_mercator();
        assert_eq!(merc.y, 0.0);
        assert_eq!(merc.x, 0.5);
        let back = LatLon::from_mercator(merc);
        assert!((back.latitude - web_mercator_max_lat).abs() < 1e-10);
    }

    for lat in [-85.05112878, -85.051129, -86.0, -89.0, -89.999999, -90.0] {
        let pt = LatLon::new(lat, 0.0);
        let merc = pt.to_mercator();
        assert_eq!(merc.y, 1.0 - 1e-9);
        assert_eq!(merc.x, 0.5);
        let back = LatLon::from_mercator(merc);
        assert!((back.latitude - (-web_mercator_max_lat)).abs() < 1e-5);
    }

    // For unnormalized lat > 90° (quadrant II), tan() flips sign
    let merc_95 = LatLon::new(95.0, 0.0).to_mercator();
    assert!(merc_95.y > 0.99);
}

#[test]
fn test_latlon_meridian_transitions_and_wrapping() {
    let prime = LatLon::new(0.0, 0.0);
    assert_eq!(prime.to_mercator().x, 0.5);
    assert_eq!(LatLon::from_mercator(prime.to_mercator()), prime);

    let west = LatLon::new(0.0, -180.0);
    assert_eq!(west.to_mercator().x, 0.0);
    assert_eq!(LatLon::from_mercator(west.to_mercator()), west);

    let east = LatLon::new(0.0, 180.0);
    assert_eq!(east.to_mercator().x, 1.0);
    assert_eq!(LatLon::from_mercator(east.to_mercator()), east);

    for lon in [-540.0, -360.0, -270.0, 270.0, 360.0, 540.0, 720.0] {
        let pt = LatLon::new(0.0, lon);
        let merc = pt.to_mercator();
        assert_eq!(merc.x, lon / 360.0 + 0.5);
        let back = LatLon::from_mercator(merc);
        assert!((back.longitude - lon).abs() < 1e-10);
    }
}

#[test]
fn test_latlon_dense_roundtrip_precision() {
    let tolerance = 1e-6; // micro-degree tolerance (~0.11m)
    let mut lat = -85.0;
    while lat <= 85.0 {
        let mut lon = -180.0;
        while lon <= 180.0 {
            let orig = LatLon::new(lat, lon);
            let merc = orig.to_mercator();
            assert!(merc.x >= 0.0 && merc.x <= 1.0);
            assert!(merc.y >= 0.0 && merc.y < 1.0);
            let back = LatLon::from_mercator(merc);
            assert!((back.latitude - orig.latitude).abs() < tolerance);
            assert!((back.longitude - orig.longitude).abs() < tolerance);
            lon += 15.0;
        }
        lat += 5.0;
    }

    let cities = [
        ("London", 51.5074, -0.1278),
        ("Tokyo", 35.6762, 139.6503),
        ("New York", 40.7128, -74.0060),
        ("Sydney", -33.8688, 151.2093),
        ("Reykjavik", 64.1466, -21.9426),
        ("Ushuaia", -54.8019, -68.3030),
        ("Quito", -0.1807, -78.4678),
        ("Singapore", 1.3521, 103.8198),
        ("Tromsø", 69.6492, 18.9553),
        ("McMurdo Station", -77.8419, 166.6863),
    ];
    for (name, c_lat, c_lon) in cities {
        let city = LatLon::new(c_lat, c_lon);
        let back = LatLon::from_mercator(city.to_mercator());
        assert!(
            (back.latitude - city.latitude).abs() < 1e-10,
            "{name} lat precision"
        );
        assert!(
            (back.longitude - city.longitude).abs() < 1e-10,
            "{name} lon precision"
        );
    }
}

#[test]
fn test_latlon_float_edge_cases() {
    let neg_zero = LatLon::new(-0.0, -0.0);
    let merc = neg_zero.to_mercator();
    assert_eq!(merc.x, 0.5);
    assert_eq!(merc.y, 0.5);
    let back = LatLon::from_mercator(merc);
    assert_eq!(back.latitude, 0.0);
    assert_eq!(back.longitude, 0.0);

    let nan_lat = LatLon::new(f64::NAN, 0.0);
    assert!(nan_lat.to_mercator().y.is_nan());
    assert!(LatLon::from_mercator(nan_lat.to_mercator())
        .latitude
        .is_nan());

    let nan_lon = LatLon::new(0.0, f64::NAN);
    assert!(nan_lon.to_mercator().x.is_nan());
    assert!(LatLon::from_mercator(nan_lon.to_mercator())
        .longitude
        .is_nan());

    let inf_lat = LatLon::new(f64::INFINITY, 0.0);
    assert!(inf_lat.to_mercator().y.is_nan());

    let inf_lon = LatLon::new(0.0, f64::INFINITY);
    assert_eq!(inf_lon.to_mercator().x, f64::INFINITY);
}
