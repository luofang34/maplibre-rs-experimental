#![allow(clippy::expect_used, clippy::panic)]

use std::{sync::Arc, time::Duration};

use super::{tests::*, *};
use crate::{
    background::BackgroundPlugin,
    headless::{create_headless_renderer_on_host_gpu_with_shared_assets, SharedMapAssets},
    io::resource_loader::SharedLoader,
    overlay::OverlayPlugin,
    raster::{DefaultRasterTransferables, RasterPlugin},
    render::{
        settings::{BufferPoolSizes, Msaa, RendererSettings},
        RenderPlugin,
    },
    terrain::{transferables::DefaultDemTransferables, DemTileComponent, TerrainPlugin},
    vector::{DefaultVectorTransferables, VectorPlugin},
};

#[test]
fn shared_map_assets_deduplicates_static_dem_and_pipelines_across_isolated_worlds() {
    crate::headless::map::test_async::run_async(async {
        let Some(gpu) = host_gpu().await else {
            return;
        };
        let client = CountingTileClient::default();
        let shared = SharedMapAssets::with_loader(SharedLoader::new(client.clone()));
        let settings = RendererSettings {
            msaa: Msaa { samples: 1 },
            symbol_pools: BufferPoolSizes::EMPTY,
            terrain_drape_size: 256,
            ..RendererSettings::default()
        };

        let build_domain_map = |bg: &str| {
            let (kernel, renderer) = create_headless_renderer_on_host_gpu_with_shared_assets(
                96,
                64,
                gpu.clone(),
                settings,
                &shared,
            )
            .expect("shared-asset renderer");
            HeadlessMap::new(
                terrain_raster_style(bg),
                renderer,
                kernel,
                vec![
                    Box::new(RenderPlugin),
                    Box::new(BackgroundPlugin),
                    Box::new(VectorPlugin::<DefaultVectorTransferables>::default()),
                    Box::new(RasterPlugin::<DefaultRasterTransferables>::default()),
                    Box::new(TerrainPlugin::<DefaultDemTransferables>::default()),
                    Box::new(OverlayPlugin),
                ],
            )
            .expect("domain map")
        };

        let mut truth_map = build_domain_map("#102030");
        let mut belief_map = build_domain_map("#401010");

        for _ in 0..16 {
            truth_map.run_frame().expect("truth frame");
            belief_map.run_frame().expect("belief frame");
            crate::headless::map::test_async::sleep(Duration::from_millis(5)).await;
        }

        assert!(client.total_fetches() > 0, "tiles should have been fetched");
        assert_eq!(
            client.max_fetches_for_any_url(),
            1,
            "TruthScene and BeliefAvionics must never fetch the same static DEM/raster tile twice"
        );
        assert!(
            shared.asset_cache().dem_decode_count() > 0,
            "shared AssetCache must decode DEM tiles"
        );
        assert!(
            shared.pipelines().pipeline_hit_count() > 0,
            "BeliefAvionics must hit SharedPipelines compiled by TruthScene"
        );

        let common_dem = loaded_dem_coords(&truth_map)
            .intersection(&loaded_dem_coords(&belief_map))
            .copied()
            .next()
            .expect("at least one shared DEM coordinate");
        let Some(DemTileComponent::Loaded(truth_dem)) = truth_map
            .world()
            .tiles
            .query::<&DemTileComponent>(common_dem)
        else {
            panic!("truth DEM missing");
        };
        let Some(DemTileComponent::Loaded(belief_dem)) = belief_map
            .world()
            .tiles
            .query::<&DemTileComponent>(common_dem)
        else {
            panic!("belief DEM missing");
        };
        assert!(
            Arc::ptr_eq(&truth_dem.tile, &belief_dem.tile) || truth_dem.tile == belief_dem.tile,
            "decoded DemTile data must match across domains"
        );

        // Verify strict domain isolation for Style, GeoJsonSource, and OverlayMesh.
        truth_map
            .mutate_style(|style| {
                style.set_paint_property("bg", "background-color", "#00ff00".into())
            })
            .expect("mutate truth style");
        truth_map
        .map_context
        .set_geojson_data(
            "tracks",
            crate::style::source::GeoJsonData::Inline(serde_json::json!({
                "type": "FeatureCollection",
                "features": [{
                    "type": "Feature",
                    "geometry": { "type": "LineString", "coordinates": [[8.0, 47.0], [8.1, 47.1]] },
                    "properties": {}
                }]
            })
            .into()),
        )
        .expect("set truth geojson");
        truth_map.set_overlay(fullscreen_overlay(96, 64, [0.0, 0.0, 1.0, 1.0]));

        truth_map.run_frame().expect("truth isolated frame");
        belief_map.run_frame().expect("belief isolated frame");

        let truth_px = crate::headless::map::readback::read_blocking(
            &truth_map,
            truth_map.color_texture().expect("truth color"),
            wgpu::TextureAspect::All,
        )
        .expect("read truth");
        let belief_px = crate::headless::map::readback::read_blocking(
            &belief_map,
            belief_map.color_texture().expect("belief color"),
            wgpu::TextureAspect::All,
        )
        .expect("read belief");
        assert_ne!(
            truth_px, belief_px,
            "TruthScene overlay/GeoJSON/style mutations must never leak into BeliefAvionics"
        );
    })
}
