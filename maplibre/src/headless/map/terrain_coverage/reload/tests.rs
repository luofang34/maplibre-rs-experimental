use super::*;

#[tokio::test]
async fn headless_replacement_updates_cpu_and_gpu() {
    replace(Delivery::Headless, false).await;
}

#[tokio::test]
async fn worker_replacement_updates_cpu_and_gpu() {
    replace(Delivery::Worker, false).await;
}

#[tokio::test]
async fn headless_replacement_updates_both_sides_of_neighbour_borders() {
    replace(Delivery::Headless, true).await;
}

#[tokio::test]
async fn worker_replacement_updates_both_sides_of_neighbour_borders() {
    replace(Delivery::Worker, true).await;
}

#[tokio::test]
async fn headless_reload_cannot_alias_a_retained_gpu_texture() {
    evict_and_reload(Delivery::Headless, false).await;
}

#[tokio::test]
async fn worker_reload_cannot_alias_a_retained_gpu_texture() {
    evict_and_reload(Delivery::Worker, false).await;
}

#[tokio::test]
async fn headless_reload_after_cpu_and_gpu_eviction_updates_both() {
    evict_and_reload(Delivery::Headless, true).await;
}

#[tokio::test]
async fn worker_reload_after_cpu_and_gpu_eviction_updates_both() {
    evict_and_reload(Delivery::Worker, true).await;
}
