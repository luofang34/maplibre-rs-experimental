#![allow(clippy::expect_used, clippy::panic)]

use std::time::Duration;

use cgmath::{Matrix4, Rad, SquareMatrix, Vector3};

use crate::{
    coords::LatLon,
    headless::{create_headless_renderer, map::HeadlessMap},
    render::{
        camera::EyeFrustum,
        view_state::ExternalAnchor,
        xr::{EyeTarget, ScenePlacement, XrEye, XrFrame},
        RenderPlugin,
    },
    style::Style,
};

const SIZE: u32 = 64;

fn texture(device: &wgpu::Device, format: wgpu::TextureFormat) -> wgpu::Texture {
    texture_sized(device, format, SIZE, SIZE)
}

fn texture_sized(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    width: u32,
    height: u32,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: None,
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

/// Fills a target with a value no frame produces, so a frame that draws into it can be told
/// from one that does not.
fn fill(device: &wgpu::Device, queue: &wgpu::Queue, color: &wgpu::Texture, depth: &wgpu::Texture) {
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    let color = color.create_view(&Default::default());
    let depth = depth.create_view(&Default::default());
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("fill colour"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &color,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::BLUE),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
    });
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("fill depth"),
        color_attachments: &[],
        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
            view: &depth,
            depth_ops: Some(wgpu::Operations {
                load: wgpu::LoadOp::Clear(1.0),
                store: wgpu::StoreOp::Store,
            }),
            stencil_ops: None,
        }),
        timestamp_writes: None,
        occlusion_query_set: None,
    });
    queue.submit([encoder.finish()]);
}

fn read_back(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture) -> Vec<u8> {
    let bytes = u64::from(SIZE * SIZE * 4);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: bytes,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::ImageCopyBuffer {
            buffer: &buffer,
            layout: wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(SIZE * 4),
                rows_per_image: None,
            },
        },
        texture.size(),
    );
    queue.submit([encoder.finish()]);
    buffer.slice(..).map_async(wgpu::MapMode::Read, |_| ());
    device.poll(wgpu::Maintain::Wait);
    let data = buffer.slice(..).get_mapped_range().to_vec();
    buffer.unmap();
    data
}

#[tokio::test]
async fn each_eye_draws_into_its_own_targets() {
    let style: Style = serde_json::from_str(r#"{"version": 8, "sources": {}, "layers": []}"#)
        .expect("an empty style parses");
    let (kernel, renderer) = create_headless_renderer(SIZE, SIZE, None)
        .await
        .expect("a headless renderer");
    let format = renderer.state().surface().surface_format();
    let mut map =
        HeadlessMap::new(style, renderer, kernel, vec![Box::new(RenderPlugin)]).expect("a map");
    let colors = [texture(map.device(), format), texture(map.device(), format)];
    let depths = [
        texture(map.device(), wgpu::TextureFormat::Depth32Float),
        texture(map.device(), wgpu::TextureFormat::Depth32Float),
    ];
    for (color, depth) in colors.iter().zip(&depths) {
        fill(map.device(), map.queue(), color, depth);
    }
    let frame = XrFrame {
        timestamp: Duration::from_millis(16),
        placement: ScenePlacement {
            anchor: ExternalAnchor {
                position: LatLon::new(47.0, 11.0),
                altitude_meters: 0.0,
            },
            world_from_scene: Matrix4::identity(),
        },
        eyes: (0..2_u32)
            .map(|index| XrEye {
                // Two eyes a little apart, three kilometres up, looking straight down.
                world_from_eye: Matrix4::from_translation(Vector3::new(
                    f64::from(index) * 0.064,
                    0.0,
                    3000.0,
                )),
                frustum: EyeFrustum::symmetric(Rad(1.0), 1.0, 0.1, 100_000.0),
                target: EyeTarget {
                    color: Some(colors[index as usize].create_view(&Default::default())),
                    depth: Some(depths[index as usize].create_view(&Default::default())),
                },
            })
            .collect(),
        request_overscan: 1.0,
        prefetch: None,
    };

    map.run_xr_frame(frame).expect("both eyes render");

    for (index, (color, depth)) in colors.iter().zip(&depths).enumerate() {
        let pixels = read_back(map.device(), map.queue(), color);
        assert!(
            pixels.iter().all(|&byte| byte == 0),
            "eye {index} was cleared by the frame, not left blue"
        );
        let depth = read_back(map.device(), map.queue(), depth);
        assert!(
            depth.iter().all(|&byte| byte == 0),
            "eye {index} received the frame's depth, which is zero at the far plane"
        );
    }
    let pose = map.view_state().camera_pose();
    assert!((pose.altitude_meters - 3000.0).abs() < 1e-6, "{pose:?}");
}

#[tokio::test]
async fn a_headless_surface_multisamples_like_a_window() {
    use crate::render::{eventually::Eventually, settings::Msaa};

    let style: Style = serde_json::from_str(r#"{"version": 8, "sources": {}, "layers": []}"#)
        .expect("an empty style parses");
    let (kernel, renderer) = create_headless_renderer(SIZE, SIZE, None)
        .await
        .expect("a headless renderer");
    assert!(
        renderer
            .state()
            .surface()
            .is_multisampling_supported(Msaa { samples: 4 }),
        "the adapter multisamples the surface format"
    );
    let mut map =
        HeadlessMap::new(style, renderer, kernel, vec![Box::new(RenderPlugin)]).expect("a map");
    map.run_frame().expect("a frame renders");
    assert!(
        matches!(
            map.map_context.renderer.resources.multisampling_texture,
            Eventually::Initialized(Some(_))
        ),
        "the frame resolves from a multisampled texture"
    );
}

#[tokio::test]
async fn without_terrain_no_elevation_is_known() {
    let style: Style = serde_json::from_str(r#"{"version": 8, "sources": {}, "layers": []}"#)
        .expect("an empty style parses");
    let (kernel, renderer) = create_headless_renderer(SIZE, SIZE, None)
        .await
        .expect("a headless renderer");
    let mut map =
        HeadlessMap::new(style, renderer, kernel, vec![Box::new(RenderPlugin)]).expect("a map");
    map.run_frame().expect("a frame renders");
    assert_eq!(map.terrain_elevation_at(LatLon::new(47.26, 11.39)), None);
}

/// A globe style with terrain and a vector source, as the device draws.
fn terrain_globe_style() -> Style {
    serde_json::from_str(
        r##"{"version":8,"sources":{"osm":{"type":"vector","tiles":["https://osm.example/{z}/{x}/{y}.pbf"],"maxzoom":14},"dem":{"type":"raster-dem","tiles":["https://dem.example/{z}/{x}/{y}.png"],"tileSize":256,"maxzoom":12,"encoding":"terrarium"}},"layers":[{"id":"land","type":"fill","source":"osm","source-layer":"land","paint":{"fill-color":"#ccc"}}],"terrain":{"source":"dem","exaggeration":1},"projection":{"type":"globe"}}"##,
    )
    .expect("a globe terrain style parses")
}

#[tokio::test]
async fn both_eyes_of_a_frame_draw_the_same_tiles_over_terrain() {
    use crate::render::eye_covering::SharedCovering;

    // The zoom an eye derives to counts tiles per pixel, so the surface is the headset's.
    let (width, height) = (1888, 1792);
    let (kernel, renderer) = create_headless_renderer(width, height, None)
        .await
        .expect("a headless renderer");
    let format = renderer.state().surface().surface_format();
    let mut map = HeadlessMap::new(
        terrain_globe_style(),
        renderer,
        kernel,
        vec![Box::new(RenderPlugin)],
    )
    .expect("a map");
    map.set_max_pitch(cgmath::Deg(89.0));
    let colors = [
        texture_sized(map.device(), format, width, height),
        texture_sized(map.device(), format, width, height),
    ];
    let depths = [
        texture_sized(
            map.device(),
            wgpu::TextureFormat::Depth32Float,
            width,
            height,
        ),
        texture_sized(
            map.device(),
            wgpu::TextureFormat::Depth32Float,
            width,
            height,
        ),
    ];
    for frame_number in 0..3_u64 {
        // Two eyes 1000 km up, looking north and nearly level, a pupil apart.
        let level_gaze = Matrix4::from_angle_x(Rad(89.5_f64.to_radians()));
        let frame = XrFrame {
            timestamp: Duration::from_millis(16 * (frame_number + 1)),
            placement: ScenePlacement {
                anchor: ExternalAnchor {
                    position: LatLon::new(47.26, 11.39),
                    altitude_meters: 0.0,
                },
                world_from_scene: Matrix4::identity(),
            },
            eyes: (0..2_u32)
                .map(|index| XrEye {
                    world_from_eye: Matrix4::from_translation(Vector3::new(
                        f64::from(index) * 0.064,
                        0.0,
                        1.0e6,
                    )) * level_gaze,
                    frustum: EyeFrustum::symmetric(Rad(1.4), 1.05, 0.5, 1.0e8),
                    target: EyeTarget {
                        color: Some(colors[index as usize].create_view(&Default::default())),
                        depth: Some(depths[index as usize].create_view(&Default::default())),
                    },
                })
                .collect(),
            request_overscan: 1.2,
            prefetch: None,
        };
        map.run_xr_frame(frame)
            .expect("both eyes render over terrain");

        let world = map.world();
        let shared = world
            .resources
            .get::<SharedCovering>()
            .expect("the first eye kept its selection");
        let first_eye: Vec<_> = shared.tiles().expect("the first eye saw tiles").to_vec();
        let second_eye = crate::render::drawn_tiles(world);
        let view = map.view_state();
        assert!(
            first_eye.len() >= 25,
            "frame {frame_number}: a level gaze from 1000 km draws {} tiles at zoom {:.2} pitch {:?} center {:?}: {first_eye:?}",
            first_eye.len(),
            view.zoom().value(),
            view.camera().get_pitch(),
            view.external_view().anchor,
        );
        for coords in &first_eye {
            assert!(
                second_eye.contains(coords),
                "frame {frame_number}: the second eye does not draw {coords:?} the first eye selected"
            );
        }
    }
}

#[tokio::test]
async fn a_globe_on_a_table_requests_only_the_levels_it_shows() {
    use cgmath::Matrix3;

    let (width, height) = (1888, 1792);
    let (kernel, renderer) = create_headless_renderer(width, height, None)
        .await
        .expect("a headless renderer");
    let format = renderer.state().surface().surface_format();
    let mut map = HeadlessMap::new(
        terrain_globe_style(),
        renderer,
        kernel,
        vec![Box::new(RenderPlugin)],
    )
    .expect("a map");
    map.set_max_pitch(cgmath::Deg(89.0));
    let colors = [
        texture_sized(map.device(), format, width, height),
        texture_sized(map.device(), format, width, height),
    ];
    let depths = [
        texture_sized(
            map.device(),
            wgpu::TextureFormat::Depth32Float,
            width,
            height,
        ),
        texture_sized(
            map.device(),
            wgpu::TextureFormat::Depth32Float,
            width,
            height,
        ),
    ];
    // A globe fifteen centimetres in radius a metre ahead of the eyes, as on the table.
    let radius_meters = 6_371_008.8;
    let radius = 0.15;
    let scale = radius / radius_meters;
    let latitude = 47.26_f64.to_radians();
    let east = Vector3::new(1.0, 0.0, 0.0);
    let north = Vector3::new(0.0, latitude.cos(), -latitude.sin());
    let up = Vector3::new(0.0, latitude.sin(), latitude.cos());
    let rotation = Matrix3::from_cols(east, north, up);
    let center = Vector3::new(0.0, -0.3, -1.0);
    let world_from_scene = Matrix4::from_translation(center + up * radius)
        * Matrix4::from(rotation)
        * Matrix4::from_scale(scale);
    for frame_number in 0..2_u64 {
        let frame = XrFrame {
            timestamp: Duration::from_millis(16 * (frame_number + 1)),
            placement: ScenePlacement {
                anchor: ExternalAnchor {
                    position: LatLon::new(47.26, 11.39),
                    altitude_meters: 0.0,
                },
                world_from_scene,
            },
            eyes: (0..2_u32)
                .map(|index| XrEye {
                    world_from_eye: Matrix4::from_translation(Vector3::new(
                        f64::from(index) * 0.064,
                        0.0,
                        0.0,
                    )),
                    frustum: EyeFrustum::symmetric(Rad(1.4), 1.05, 0.05, 100.0),
                    target: EyeTarget {
                        color: Some(colors[index as usize].create_view(&Default::default())),
                        depth: Some(depths[index as usize].create_view(&Default::default())),
                    },
                })
                .collect(),
            request_overscan: 1.2,
            prefetch: None,
        };
        map.run_xr_frame(frame)
            .expect("both eyes render the table globe");
    }
    let view = map.view_state();
    let level = u8::from(view.zoom().zoom_level(crate::coords::TILE_SIZE));
    let mut levels: Vec<u8> = map
        .world()
        .tiles
        .tiles
        .values()
        .map(|tile| u8::from(tile.coords.z))
        .collect();
    levels.sort_unstable();
    levels.dedup();
    assert!(
        levels.iter().all(|z| *z <= level + 2),
        "a table globe at zoom {:.2} (level {level}) requested tiles at levels {levels:?}",
        view.zoom().value()
    );
}

#[tokio::test]
async fn a_flight_from_the_table_requests_tiles_a_few_at_a_time() {
    use crate::io::tile_backpressure::{tiles_in_flight, MAX_TILES_IN_FLIGHT};
    use cgmath::Matrix3;

    let (width, height) = (1888, 1792);
    let (kernel, renderer) = create_headless_renderer(width, height, None)
        .await
        .expect("a headless renderer");
    let format = renderer.state().surface().surface_format();
    let mut map = HeadlessMap::new(
        terrain_globe_style(),
        renderer,
        kernel,
        vec![Box::new(RenderPlugin)],
    )
    .expect("a map");
    map.set_max_pitch(cgmath::Deg(89.0));
    let colors = [
        texture_sized(map.device(), format, width, height),
        texture_sized(map.device(), format, width, height),
    ];
    let depths = [
        texture_sized(
            map.device(),
            wgpu::TextureFormat::Depth32Float,
            width,
            height,
        ),
        texture_sized(
            map.device(),
            wgpu::TextureFormat::Depth32Float,
            width,
            height,
        ),
    ];
    let radius_meters = 6_371_008.8;
    let latitude = 47.26_f64.to_radians();
    let east = Vector3::new(1.0, 0.0, 0.0);
    let north = Vector3::new(0.0, latitude.cos(), -latitude.sin());
    let up = Vector3::new(0.0, latitude.sin(), latitude.cos());
    let rotation = Matrix3::from_cols(east, north, up);
    // The table globe the eyes see now, and the ground 2000 m under them the flight ends
    // at: the placement a frame carries as its prefetch.
    let table = Matrix4::from_translation(Vector3::new(0.0, -0.3, -1.0) + up * 0.15)
        * Matrix4::from(rotation)
        * Matrix4::from_scale(0.15 / radius_meters);
    let ground =
        Matrix4::from_translation(Vector3::new(0.0, -2000.0, 0.0)) * Matrix4::from(rotation);
    let anchor = ExternalAnchor {
        position: LatLon::new(47.26, 11.39),
        altitude_meters: 0.0,
    };
    for frame_number in 0..4_u64 {
        let frame = XrFrame {
            timestamp: Duration::from_millis(16 * (frame_number + 1)),
            placement: ScenePlacement {
                anchor,
                world_from_scene: table,
            },
            eyes: (0..2_u32)
                .map(|index| XrEye {
                    world_from_eye: Matrix4::from_translation(Vector3::new(
                        f64::from(index) * 0.064,
                        0.0,
                        0.0,
                    )),
                    frustum: EyeFrustum::symmetric(Rad(1.4), 1.05, 0.05, 1.0e8),
                    target: EyeTarget {
                        color: Some(colors[index as usize].create_view(&Default::default())),
                        depth: Some(depths[index as usize].create_view(&Default::default())),
                    },
                })
                .collect(),
            request_overscan: 1.2,
            prefetch: Some(ScenePlacement {
                anchor,
                world_from_scene: ground,
            }),
        };
        map.run_xr_frame(frame).expect("the frame renders");
        // No tile ever arrives here, so every request stays in flight.
        let in_flight = tiles_in_flight(&map.world().tiles);
        assert!(
            in_flight <= MAX_TILES_IN_FLIGHT,
            "frame {frame_number}: {in_flight} tiles in flight, {} resident",
            map.world().tiles.tiles.len()
        );
    }
}

#[tokio::test]
async fn a_flights_prefetch_is_built_once_and_does_not_follow_the_head() {
    use crate::render::xr::PrefetchView;
    use cgmath::Matrix3;

    let (width, height) = (1888, 1792);
    let (kernel, renderer) = create_headless_renderer(width, height, None)
        .await
        .expect("a headless renderer");
    let format = renderer.state().surface().surface_format();
    let mut map = HeadlessMap::new(
        terrain_globe_style(),
        renderer,
        kernel,
        vec![Box::new(RenderPlugin)],
    )
    .expect("a map");
    map.set_max_pitch(cgmath::Deg(89.0));
    let colors = [
        texture_sized(map.device(), format, width, height),
        texture_sized(map.device(), format, width, height),
    ];
    let depths = [
        texture_sized(
            map.device(),
            wgpu::TextureFormat::Depth32Float,
            width,
            height,
        ),
        texture_sized(
            map.device(),
            wgpu::TextureFormat::Depth32Float,
            width,
            height,
        ),
    ];
    let radius_meters = 6_371_008.8;
    let latitude = 47.26_f64.to_radians();
    let rotation = Matrix3::from_cols(
        Vector3::new(1.0, 0.0, 0.0),
        Vector3::new(0.0, latitude.cos(), -latitude.sin()),
        Vector3::new(0.0, latitude.sin(), latitude.cos()),
    );
    let table = Matrix4::from_translation(Vector3::new(0.0, -0.15, -1.0))
        * Matrix4::from(rotation)
        * Matrix4::from_scale(0.15 / radius_meters);
    let ground =
        Matrix4::from_translation(Vector3::new(0.0, -2000.0, 0.0)) * Matrix4::from(rotation);
    let anchor = ExternalAnchor {
        position: LatLon::new(47.26, 11.39),
        altitude_meters: 0.0,
    };
    let mut centers = Vec::new();
    for (frame_number, yaw) in [0.0_f64, 25.0, -40.0].into_iter().enumerate() {
        // The anchor's altitude follows the terrain as it loads during the flight.
        let anchor = ExternalAnchor {
            altitude_meters: 100.0 * frame_number as f64,
            ..anchor
        };
        let frame = XrFrame {
            timestamp: Duration::from_millis(16 * (frame_number as u64 + 1)),
            placement: ScenePlacement {
                anchor,
                world_from_scene: table,
            },
            eyes: (0..2_u32)
                .map(|index| XrEye {
                    // The head turns from frame to frame while the flight runs.
                    world_from_eye: Matrix4::from_angle_y(cgmath::Deg(yaw))
                        * Matrix4::from_translation(Vector3::new(
                            f64::from(index) * 0.064,
                            0.0,
                            0.0,
                        )),
                    frustum: EyeFrustum::symmetric(Rad(1.4), 1.05, 0.05, 1.0e8),
                    target: EyeTarget {
                        color: Some(colors[index as usize].create_view(&Default::default())),
                        depth: Some(depths[index as usize].create_view(&Default::default())),
                    },
                })
                .collect(),
            request_overscan: 1.2,
            prefetch: Some(ScenePlacement {
                anchor,
                world_from_scene: ground,
            }),
        };
        map.run_xr_frame(frame).expect("the frame renders");
        let prefetch = map
            .world()
            .resources
            .get::<PrefetchView>()
            .and_then(|prefetch| prefetch.view_state.as_ref())
            .expect("a flight keeps its prefetch view");
        let view = prefetch.external_view();
        centers.push((
            view.anchor.position.latitude,
            view.anchor.position.longitude,
            prefetch.zoom().value(),
        ));
    }
    assert!(
        centers.windows(2).all(|pair| pair[0] == pair[1]),
        "the prefetch view followed the head: {centers:?}"
    );
}
