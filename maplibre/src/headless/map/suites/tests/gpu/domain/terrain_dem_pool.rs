#![cfg_attr(test, allow(clippy::expect_used))]

use crate::{
    coords::{WorldTileCoords, ZoomLevel},
    headless::create_headless_renderer,
    io::tile_sources::RasterSourceId,
    terrain::{gpu_dem_pool::GpuDemPool, DemTile, LoadedDem},
};

#[test]
fn gpu_dem_pool_evicts_least_recently_used_beyond_capacity() {
    futures::executor::block_on(async {
        let (_, renderer) = create_headless_renderer(32, 32, None)
            .await
            .expect("renderer");
        let mut pool = GpuDemPool::with_capacity(2);
        let source = RasterSourceId::from("dem");
        let img = image::RgbaImage::new(4, 4);
        let dem = LoadedDem::new(
            DemTile::from_image(&img, [256.0, 1.0, 1.0 / 256.0, -32768.0]).expect("dem"),
        );
        let [t0, t1, t2] = [0, 1, 2].map(|x| WorldTileCoords::from((x, 0, ZoomLevel::new(4))));

        pool.upload(&renderer.device, &renderer.queue, &source, t0, &dem);
        pool.upload(&renderer.device, &renderer.queue, &source, t1, &dem);
        // Touch t0 so t1 becomes the least recently used entry.
        assert!(pool.get(&source, t0).is_some());
        pool.upload(&renderer.device, &renderer.queue, &source, t2, &dem);

        assert_eq!(pool.texture_count(), 2);
        assert!(
            pool.get(&source, t0).is_some(),
            "recently touched DEM stays"
        );
        assert!(pool.get(&source, t2).is_some(), "newly uploaded DEM stays");
        assert!(pool.get(&source, t1).is_none(), "oldest DEM was evicted");
    })
}
