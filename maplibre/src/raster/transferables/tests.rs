#![allow(clippy::expect_used)]

use super::*;

#[test]
fn decoded_source_identity_survives_native_message_transport() {
    let coords = WorldTileCoords::from((7, 3, 4_u8.into()));
    for source in [
        RasterSourceId::default(),
        "".into(),
        "raster".into(),
        "elevation".into(),
    ] {
        let image = RgbaImage::from_pixel(2, 1, image::Rgba([12, 34, 56, 255]));
        let message = IntoMessage::into(DefaultLayerRaster::build_from(
            coords,
            source.clone(),
            image.clone(),
        ));
        let decoded = message
            .into_transferable::<DefaultLayerRaster>()
            .expect("raster message");
        assert_eq!(decoded.coords(), coords);
        let data = decoded.to_layer();
        assert_eq!(data.source, source);
        assert_eq!(data.image, image);
    }
}

#[test]
fn missing_source_identity_survives_native_message_transport() {
    let coords = WorldTileCoords::from((7, 3, 4_u8.into()));
    for source in [
        RasterSourceId::default(),
        "".into(),
        "raster".into(),
        "elevation".into(),
    ] {
        let message = IntoMessage::into(DefaultLayerRasterMissing::build_from(
            coords,
            source.clone(),
        ));
        let decoded = message
            .into_transferable::<DefaultLayerRasterMissing>()
            .expect("missing message");
        assert_eq!(decoded.coords(), coords);
        let data = decoded.to_layer();
        assert_eq!(data.coords, coords);
        assert_eq!(data.source, source);
    }
}
