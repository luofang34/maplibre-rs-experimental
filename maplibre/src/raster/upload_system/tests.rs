use image::{Rgba, RgbaImage};

use super::*;
use crate::{
    coords::ZoomLevel,
    io::tile_sources::TileKind,
    raster::RasterSourceId,
    render::{
        memory_budget::FRAME_UPLOAD_BYTES,
        settings::Msaa,
        tile_view_pattern::{HasTile, ViewTileSources},
    },
    style::Style,
    tcs::world::World,
};

/// A pipeline whose second group takes a tile texture and its sampler, as the raster one does.
fn pipeline(device: &wgpu::Device) -> wgpu::RenderPipeline {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: None,
        source: wgpu::ShaderSource::Wgsl(
            "@group(1) @binding(0) var tile: texture_2d<f32>;
             @group(1) @binding(1) var tile_sampler: sampler;
             @vertex fn vs() -> @builtin(position) vec4<f32> { return vec4<f32>(0.0); }
             @fragment fn fs() -> @location(0) vec4<f32> {
                 return textureSample(tile, tile_sampler, vec2<f32>(0.5));
             }"
            .into(),
        ),
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: None,
        layout: None,
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::TextureFormat::Rgba8Unorm.into())],
        }),
        multiview_mask: None,
        cache: None,
    })
}

/// A tile whose image carries coordinates other than those it is stored under is uploaded
/// once, under the stored ones, and then left alone.
#[tokio::test]
async fn a_bound_tile_is_not_uploaded_again() {
    let instance = wgpu::Instance::default();
    let Ok(adapter) = instance
        .request_adapter(&wgpu::RequestAdapterOptions::default())
        .await
    else {
        return;
    };
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor::default())
        .await
        .expect("device");
    let queue = crate::render::upload_queue::UploadQueue::new(queue);
    let style: Style = serde_json::from_value(serde_json::json!({"version": 8,
        "sources": {"pic": {"type": "raster", "tiles": ["offline://{z}/{x}/{y}.png"]}},
        "layers": [{"id": "pic", "type": "raster", "source": "pic"}]}))
    .expect("style");
    let mut resources = RasterResources::new(Msaa { samples: 1 }, &device, pipeline(&device));
    resources.update_layer_sources(&style);
    let source = RasterSourceId::from("pic");
    let stored = WorldTileCoords {
        x: 1,
        y: 2,
        z: ZoomLevel::from(3),
    };
    let carried = WorldTileCoords { x: 9, ..stored };
    let mut world = World::default();
    world
        .tiles
        .spawn_mut(stored)
        .expect("tile")
        .insert(RasterLayersDataComponent {
            layers: vec![RasterLayerData::Available(AvailableRasterLayerData {
                coords: carried,
                source: source.clone(),
                image: RgbaImage::from_pixel(4, 4, Rgba([255, 0, 0, 255])),
            })],
        });

    let upload = |resources: &mut RasterResources| {
        upload_raster_layer(
            resources,
            (&device, &queue),
            &world.tiles,
            vec![stored],
            &mut FrameUploads::default(),
        );
    };
    upload(&mut resources);
    assert!(resources.get_bound_texture(&source, &stored).is_some());
    assert!(resources.get_bound_texture(&source, &carried).is_none());
    let revision = resources.revision();
    queue.take_written_bytes();
    for _ in 0..3 {
        upload(&mut resources);
    }
    assert_eq!(resources.revision(), revision, "no texture was bound again");
    assert_eq!(
        queue.take_written_bytes(),
        0,
        "no texels were written again"
    );
}

/// A child tile whose CPU image has arrived but whose GPU upload was deferred by the frame
/// budget does not claim coverage yet, keeping its uploaded ancestor eligible as a stand-in.
#[tokio::test]
async fn deferred_child_tile_waits_for_gpu_upload_before_claiming_coverage() {
    let instance = wgpu::Instance::default();
    let Ok(adapter) = instance
        .request_adapter(&wgpu::RequestAdapterOptions::default())
        .await
    else {
        return;
    };
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor::default())
        .await
        .expect("device");
    let queue = crate::render::upload_queue::UploadQueue::new(queue);
    let style: Style = serde_json::from_value(serde_json::json!({"version": 8,
        "sources": {"pic": {"type": "raster", "tiles": ["offline://{z}/{x}/{y}.png"]}},
        "layers": [{"id": "pic", "type": "raster", "source": "pic"}]}))
    .expect("style");
    let mut resources = RasterResources::new(Msaa { samples: 1 }, &device, pipeline(&device));
    resources.update_layer_sources(&style);
    let source = RasterSourceId::from("pic");
    let parent = WorldTileCoords {
        x: 0,
        y: 0,
        z: ZoomLevel::from(0),
    };
    let child = WorldTileCoords {
        x: 0,
        y: 0,
        z: ZoomLevel::from(1),
    };
    let mut world = World::default();
    world
        .resources
        .get_or_init_mut::<ViewTileSources>()
        .add::<crate::raster::RasterTilesDone>(TileKind::Raster);
    for coords in [child, parent] {
        world
            .tiles
            .spawn_mut(coords)
            .expect("tile")
            .insert(RasterLayersDataComponent {
                layers: vec![RasterLayerData::Available(AvailableRasterLayerData {
                    coords,
                    source: source.clone(),
                    image: RgbaImage::from_pixel(4, 4, Rgba([255, 0, 0, 255])),
                })],
            });
    }

    // Consume all but 64 bytes of the image budget so only the coarse parent fits this frame.
    let mut budget = FrameUploads::default();
    assert!(budget.take(UploadKind::Image, FRAME_UPLOAD_BYTES - 64));
    let mut ordered = vec![child, parent];
    ordered.sort_by_key(|coords| (u8::from(coords.z), *coords));
    assert!(upload_raster_layer(
        &mut resources,
        (&device, &queue),
        &world.tiles,
        ordered.clone(),
        &mut budget,
    ));
    world.resources.insert(Eventually::Initialized(resources));

    let sources = world
        .resources
        .get::<ViewTileSources>()
        .expect("sources")
        .of_kind(TileKind::Raster);
    assert!(
        sources.has_source_tile(&source, parent, &world),
        "uploaded parent stays ready as a stand-in"
    );
    assert!(
        !sources.has_source_tile(&source, child, &world),
        "deferred child does not displace its parent before GPU upload"
    );

    let Some(Eventually::Initialized(mut resources)) = world
        .resources
        .get_mut::<Eventually<RasterResources>>()
        .map(|slot| std::mem::replace(slot, Eventually::Uninitialized))
    else {
        return;
    };
    budget.reset();
    assert!(!upload_raster_layer(
        &mut resources,
        (&device, &queue),
        &world.tiles,
        ordered,
        &mut budget,
    ));
    world.resources.insert(Eventually::Initialized(resources));
    let sources = world
        .resources
        .get::<ViewTileSources>()
        .expect("sources")
        .of_kind(TileKind::Raster);
    assert!(sources.has_source_tile(&source, child, &world));
}
