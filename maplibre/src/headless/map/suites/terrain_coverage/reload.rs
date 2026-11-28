//! Same-coordinate DEM replacement through the headless and worker ingestion paths.

use super::{prepared_map, target, HeadlessMap, Rgba, RgbaImage, WorldTileCoords};
use crate::{
    headless::environment::HeadlessEnvironment,
    io::apc::{tests::reply_context, Context},
    render::eventually::Eventually,
    schedule::system::System,
    terrain::{
        dem::gpu_readback::{pipeline, sample_gpu_blocking},
        populate_world_system::PopulateWorldSystem,
        resources::TerrainResources,
        DefaultDemTransferables, DefaultLayerDem, DemTileComponent, LayerDem,
    },
};

#[derive(Clone, Copy)]
enum Delivery {
    Headless,
    Worker,
}

fn image(height: u16) -> RgbaImage {
    let encoded = 32768 + height;
    RgbaImage::from_pixel(
        256,
        256,
        Rgba([(encoded >> 8) as u8, encoded as u8, 0, 255]),
    )
}

fn ingest(map: &mut HeadlessMap, delivery: Delivery, coords: WorldTileCoords, height: u16) {
    match delivery {
        Delivery::Headless => map
            .load_dem_tiles(vec![(coords, image(height))])
            .expect("headless DEM ingestion"),
        Delivery::Worker => {
            if map
                .map_context
                .world
                .tiles
                .query::<&DemTileComponent>(coords)
                .is_none()
            {
                map.map_context
                    .world
                    .tiles
                    .spawn_mut(coords)
                    .expect("requested tile")
                    .insert(DemTileComponent::Pending);
            }
            reply_context(map.kernel.apc())
                .send_back(DefaultLayerDem::build_from(coords, image(height)))
                .expect("worker result");
            PopulateWorldSystem::<HeadlessEnvironment, DefaultDemTransferables>::new(&map.kernel)
                .run(&mut map.map_context)
                .expect("worker DEM ingestion");
            map.run_frame().expect("upload and render");
        }
    }
}

fn assert_uploaded(map: &HeadlessMap, coords: WorldTileCoords, expected: f64) {
    let world = map.world();
    let dem_comp = world.tiles.query::<&DemTileComponent>(coords);
    assert!(
        matches!(dem_comp, Some(DemTileComponent::Loaded(_))),
        "decoded DEM"
    );
    let Some(DemTileComponent::Loaded(dem)) = dem_comp else {
        return;
    };
    assert_eq!(dem.elevation_at_tile_coords(2048.0, 2048.0), expected);
    let terrain_res = world.resources.get::<Eventually<TerrainResources>>();
    assert!(
        matches!(terrain_res, Some(Eventually::Initialized(_))),
        "terrain resources"
    );
    let Some(Eventually::Initialized(resources)) = terrain_res else {
        return;
    };
    assert!(resources.has_dem_texture(coords));
    let pipeline = pipeline(map.device());
    let heights = sample_gpu_blocking(
        map.device(),
        map.upload_queue(),
        &pipeline,
        dem,
        resources.dem_texture(Some(coords)),
    );
    for (index, actual) in heights.into_iter().enumerate() {
        let x = (index % 9) as f64 * 512.0;
        let y = (index / 9) as f64 * 512.0;
        let expected = dem.elevation_at_tile_coords(x, y);
        assert!(
            (f64::from(actual) - expected).abs() < 0.001,
            "DEM {coords} at ({x}, {y}): GPU {actual}, CPU {expected}"
        );
    }
}

async fn replace(delivery: Delivery, neighbour: bool) {
    let mut map = prepared_map(true).await;
    let coords = target();
    let east = WorldTileCoords {
        x: coords.x + 1,
        ..coords
    };
    if neighbour {
        ingest(&mut map, delivery, east, 200);
    }
    assert_uploaded(&map, coords, 0.0);
    ingest(&mut map, delivery, coords, 100);
    assert_uploaded(&map, coords, 100.0);
    if neighbour {
        assert_uploaded(&map, east, 200.0);
    }
}

async fn evict_and_reload(delivery: Delivery, drop_gpu: bool) {
    let mut map = prepared_map(true).await;
    assert!(map.map_context.world.tiles.remove(target()));
    if drop_gpu {
        let terrain_res = map
            .map_context
            .world
            .resources
            .get_mut::<Eventually<TerrainResources>>();
        assert!(
            matches!(terrain_res, Some(Eventually::Initialized(_))),
            "terrain resources"
        );
        if let Some(Eventually::Initialized(resources)) = terrain_res {
            resources.drop_dem(target());
        }
    }
    ingest(&mut map, delivery, target(), 100);
    assert_uploaded(&map, target(), 100.0);
}

mod tests;

use std::rc::Rc;

use crate::{
    kernel::Kernel,
    plugin::Plugin,
    raster::resource::RasterResources,
    render::tile_view_pattern::WgpuTileViewPattern,
    schedule::{system::SystemResult, RenderStageLabel, Schedule},
};

#[derive(Default)]
pub(super) struct Faults {
    pub(super) missing_upload: Option<WorldTileCoords>,
    pub(super) metadata_capacity: Option<usize>,
    pub(super) applied: bool,
    saved_pattern: Option<WgpuTileViewPattern>,
}

pub(super) struct FaultPlugin;

impl Plugin<HeadlessEnvironment> for FaultPlugin {
    fn build(
        &self,
        schedule: &mut Schedule,
        _kernel: Rc<Kernel<HeadlessEnvironment>>,
        world: &mut crate::tcs::world::World,
        _renderer: &mut crate::render::Renderer,
    ) -> Result<(), crate::plugin::PluginError> {
        world.resources.insert(Faults::default());
        schedule.add_system_to_stage(RenderStageLabel::Queue, inject)?;
        schedule.add_system_to_stage(RenderStageLabel::Cleanup, restore)?;
        Ok(())
    }
}

fn inject(context: &mut crate::context::MapContext) -> SystemResult {
    let resources = &mut context.world.resources;
    let mut faults = std::mem::take(resources.get_mut::<Faults>().expect("fault controls"));
    if let Some(coords) = faults.missing_upload {
        let raster_res = resources.get_mut::<Eventually<RasterResources>>();
        assert!(
            matches!(raster_res, Some(Eventually::Initialized(_))),
            "raster resources"
        );
        if let Some(Eventually::Initialized(raster)) = raster_res {
            assert!(
                raster.get_bound_texture(&"paint".into(), &coords).is_some(),
                "evict a real upload"
            );
            raster.remove_texture(coords);
            faults.applied = true;
        }
    }
    if let Some(capacity) = faults.metadata_capacity {
        let pattern_res = resources.get_mut::<Eventually<WgpuTileViewPattern>>();
        assert!(
            matches!(pattern_res, Some(Eventually::Initialized(_))),
            "tile pattern"
        );
        if let Some(Eventually::Initialized(pattern)) = pattern_res {
            let limited =
                WgpuTileViewPattern::new(crate::render::resource::BackingBufferDescriptor::new(
                    pattern.buffer().clone(),
                    (capacity * std::mem::size_of::<crate::render::shaders::ShaderTileMetadata>())
                        as u64,
                ));
            faults.saved_pattern = Some(std::mem::replace(pattern, limited));
            faults.applied = true;
        }
    }
    resources.insert(faults);
    Ok(())
}

fn restore(context: &mut crate::context::MapContext) -> SystemResult {
    let resources = &mut context.world.resources;
    if let Some(pattern) = resources
        .get_mut::<Faults>()
        .expect("fault controls")
        .saved_pattern
        .take()
    {
        resources.insert(Eventually::Initialized(pattern));
    }
    Ok(())
}
