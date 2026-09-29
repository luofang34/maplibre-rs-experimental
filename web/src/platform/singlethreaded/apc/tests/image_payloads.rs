use super::*;
use maplibre::{
    coords::{WorldTileCoords, ZoomLevel},
    raster::{LayerRaster, LayerRasterMissing, RasterSourceId},
    terrain::{LayerDem, LayerDemMissing},
};

fn round_trip(payload: FlatBufferTransferable, attempt: Option<u64>) -> FlatBufferTransferable {
    let message = IntoMessage::into(payload);
    let tag = *message
        .tag()
        .as_any()
        .downcast_ref::<WebMessageTag>()
        .expect("Web payload tag");
    let message = match attempt {
        Some(attempt) => message.with_attempt(attempt),
        None => message,
    };
    let (wire, buffer) = prepare_message(message).expect("image payload encoded");
    assert_eq!(
        wire,
        if attempt.is_some() {
            WebMessageTag::TrackedPayload
        } else {
            tag
        }
    );
    let message = decode_message(wire, buffer).expect("image payload decoded");
    assert!(message.has_tag(tag.to_static()));
    assert_eq!(message.attempt(), attempt);
    *message
        .into_transferable::<FlatBufferTransferable>()
        .expect("concrete image payload")
}

fn image_round_trip(attempt: Option<u64>, source: RasterSourceId) {
    let coords = WorldTileCoords {
        x: 3,
        y: 2,
        z: ZoomLevel::new(4),
    };
    let pixels = image::RgbaImage::from_pixel(2, 2, image::Rgba([128, 100, 0, 255]));
    let raster = round_trip(
        <FlatBufferTransferable as LayerRaster>::build_from(coords, source.clone(), pixels.clone()),
        attempt,
    );
    assert_eq!(LayerRaster::coords(&raster), coords);
    let raster = LayerRaster::to_layer(raster);
    assert_eq!(raster.source, source);
    assert_eq!(raster.image, pixels);
    let missing = round_trip(
        <FlatBufferTransferable as LayerRasterMissing>::build_from(coords, source.clone()),
        attempt,
    );
    assert_eq!(LayerRasterMissing::coords(&missing), coords);
    assert_eq!(LayerRasterMissing::to_layer(missing).source, source);
    let dem = round_trip(
        <FlatBufferTransferable as LayerDem>::build_from(coords, pixels.clone()),
        attempt,
    );
    assert_eq!(LayerDem::coords(&dem), coords);
    assert_eq!(LayerDem::into_image(dem), pixels);
    let missing = round_trip(
        <FlatBufferTransferable as LayerDemMissing>::build_from(coords),
        attempt,
    );
    assert_eq!(LayerDemMissing::coords(&missing), coords);
}

mod tests;
