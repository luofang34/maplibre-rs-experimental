use super::*;

#[test]
fn headless_replacement_updates_cpu_and_gpu() {
    crate::headless::map::test_async::run_async(async {
        replace(Delivery::Headless, false).await;
    })
}

#[test]
fn worker_replacement_updates_cpu_and_gpu() {
    crate::headless::map::test_async::run_async(async {
        replace(Delivery::Worker, false).await;
    })
}

#[test]
fn headless_replacement_updates_both_sides_of_neighbour_borders() {
    crate::headless::map::test_async::run_async(async {
        replace(Delivery::Headless, true).await;
    })
}

#[test]
fn worker_replacement_updates_both_sides_of_neighbour_borders() {
    crate::headless::map::test_async::run_async(async {
        replace(Delivery::Worker, true).await;
    })
}

#[test]
fn headless_reload_cannot_alias_a_retained_gpu_texture() {
    crate::headless::map::test_async::run_async(async {
        evict_and_reload(Delivery::Headless, false).await;
    })
}

#[test]
fn worker_reload_cannot_alias_a_retained_gpu_texture() {
    crate::headless::map::test_async::run_async(async {
        evict_and_reload(Delivery::Worker, false).await;
    })
}

#[test]
fn headless_reload_after_cpu_and_gpu_eviction_updates_both() {
    crate::headless::map::test_async::run_async(async {
        evict_and_reload(Delivery::Headless, true).await;
    })
}

#[test]
fn worker_reload_after_cpu_and_gpu_eviction_updates_both() {
    crate::headless::map::test_async::run_async(async {
        evict_and_reload(Delivery::Worker, true).await;
    })
}

#[test]
fn failed_dem_refresh_retains_cpu_and_gpu_elevation() {
    crate::headless::map::test_async::run_async(async {
        use crate::terrain::{DefaultLayerDemMissing, LayerDemMissing};
        let mut map = prepared_map(true).await;
        ingest(&mut map, Delivery::Worker, target(), 100);
        assert_uploaded(&map, target(), 100.0);
        reply_context(map.kernel.apc())
            .send_back(DefaultLayerDemMissing::build_from(target()))
            .expect("failed worker result");
        PopulateWorldSystem::<HeadlessEnvironment, DefaultDemTransferables>::new(&map.kernel)
            .run(&mut map.map_context)
            .expect("failed refresh ingestion");
        map.run_frame().expect("retained render");
        assert_uploaded(&map, target(), 100.0);
    })
}
