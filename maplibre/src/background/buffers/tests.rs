use super::*;

#[test]
fn persistent_buffers_initialize_and_upload() {
    futures::executor::block_on(async {
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
        let queue = UploadQueue::new(queue);

        let mut gpu_buffers = BackgroundGpuBuffers::new(&device);
        assert_eq!(
            gpu_buffers.metadata_capacity(),
            BackgroundGpuBuffers::INITIAL_METADATA_CAPACITY
        );
        assert_eq!(
            gpu_buffers.tile_metadata_capacity(),
            BackgroundGpuBuffers::INITIAL_TILE_METADATA_CAPACITY
        );

        let meta = BackgroundLayerMetadata {
            color: [1.0, 0.0, 0.0, 1.0],
            z_index: 0.0,
            padding: [0.0; 3],
            horizon: [0.0; 4],
            viewport: [600.0, 0.0, 800.0, 600.0],
            world_rows: [[0.0; 4]; 2],
        };
        gpu_buffers.write_metadata(&device, &queue, &[meta]);
        assert!(queue.take_written_bytes() > 0);

        gpu_buffers.write_metadata(&device, &queue, &[meta]);
        assert_eq!(queue.take_written_bytes(), 0);

        let items: Vec<BackgroundLayerMetadata> = vec![meta; 32];
        gpu_buffers.write_metadata(&device, &queue, &items);
        assert_eq!(gpu_buffers.metadata_capacity(), 32);

        let items_64: Vec<BackgroundLayerMetadata> = vec![meta; 33];
        gpu_buffers.write_metadata(&device, &queue, &items_64);
        assert_eq!(gpu_buffers.metadata_capacity(), 64);
    })
}

#[test]
fn opacity_scales_alpha() {
    let layer = serde_json::json!({"id": "b", "type": "background", "paint": {"background-color": "blue", "background-opacity": 0.5}});
    let layer: crate::style::layer::StyleLayer =
        serde_json::from_value(layer).expect("valid background layer");
    let color = background_color(&layer, 0.0);
    assert_eq!(color, [0.0, 0.0, 1.0, 0.5]);
}

#[test]
fn colour_follows_zoom() {
    let paint = serde_json::json!({"background-color": {
        "stops": [[0, "red"], [10, "blue"]], "base": 1.0
    }});
    let layer = serde_json::json!({"id": "b", "type": "background", "paint": paint});
    let layer: crate::style::layer::StyleLayer =
        serde_json::from_value(layer).expect("valid background layer");
    let start = background_color(&layer, 0.0);
    let end = background_color(&layer, 10.0);
    assert_eq!(start[..3], [1.0, 0.0, 0.0]);
    assert_eq!(end[..3], [0.0, 0.0, 1.0]);
}

#[test]
fn a_reordered_globe_covering_keeps_the_instance_tiles() {
    let tile = |x, y| WorldTileCoords {
        x,
        y,
        z: crate::coords::ZoomLevel::new(2),
    };
    let mut scratch = BackgroundScratch::default();
    assert!(scratch.sync_globe_tiles(Some(&[tile(0, 1), tile(1, 1), tile(2, 1)])));
    let instances = scratch.globe_tiles.clone();

    assert!(!scratch.sync_globe_tiles(Some(&[tile(2, 1), tile(0, 1), tile(1, 1)])));
    assert_eq!(scratch.globe_tiles, instances);

    assert!(scratch.sync_globe_tiles(Some(&[tile(2, 1), tile(0, 1), tile(3, 1)])));
    assert_ne!(scratch.globe_tiles, instances);
}
