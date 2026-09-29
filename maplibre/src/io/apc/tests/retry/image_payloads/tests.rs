use super::*;

#[tokio::test]
async fn old_raster_pixels_cannot_replace_recovered_request() {
    stale_success(Kind::Raster).await;
}
#[tokio::test]
async fn old_dem_samples_cannot_replace_recovered_request() {
    stale_success(Kind::Dem).await;
}
#[tokio::test]
async fn raster_fetch_and_decode_errors_cannot_complete_new_request() {
    stale_missing(Kind::Raster).await;
}
#[tokio::test]
async fn dem_fetch_and_decode_errors_cannot_complete_new_request() {
    stale_missing(Kind::Dem).await;
}
#[tokio::test]
async fn untracked_raster_reply_only_applies_without_admitted_request() {
    legacy(Kind::Raster).await;
}
#[tokio::test]
async fn untracked_dem_reply_only_applies_without_admitted_request() {
    legacy(Kind::Dem).await;
}
