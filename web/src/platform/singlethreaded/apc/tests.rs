#![allow(clippy::expect_used, clippy::panic)]

use std::{any::TypeId, error::Error};

use super::*;
use wasm_bindgen_test::*;

#[wasm_bindgen_test]
fn every_wire_tag_preserves_payload_bytes() {
    let bytes = [1, 4, 7, 9];
    for tag in [
        WebMessageTag::TileTessellated,
        WebMessageTag::LayerMissing,
        WebMessageTag::LayerTessellated,
        WebMessageTag::LayerIndexed,
        WebMessageTag::LayerRaster,
        WebMessageTag::LayerRasterMissing,
        WebMessageTag::SymbolLayerTessellated,
        WebMessageTag::LayerDem,
        WebMessageTag::LayerDemMissing,
    ] {
        let source = Uint8Array::from(bytes.as_slice()).buffer();
        let payload = FlatBufferTransferable::from_array_buffer(tag, source);
        let (actual_tag, buffer) =
            prepare_message(IntoMessage::into(payload)).expect("supported tag");
        assert_eq!(actual_tag, tag);
        assert_eq!(Uint8Array::new(&buffer).to_vec(), bytes);
    }
}

#[wasm_bindgen_test]
fn foreign_tags_and_wrong_payloads_are_typed_errors() {
    let foreign = prepare_message(Message::new(&8_u32, Box::new(1_u32))).expect_err("foreign tag");
    assert!(matches!(foreign, SendError::UnsupportedTag { .. }));
    let error = prepare_message(Message::new(&WebMessageTag::LayerDem, Box::new(1_u32)))
        .expect_err("wrong payload");
    let source = error
        .source()
        .expect("payload cause")
        .downcast_ref::<maplibre::io::apc::MessageError>()
        .expect("typed payload cause");
    assert_eq!(source.actual, TypeId::of::<u32>());
}

#[wasm_bindgen_test]
fn dem_reply_outside_a_worker_returns_a_transport_error() {
    use maplibre::io::source_client::HttpSourceClient;

    let context = PassingContext {
        source_client: SourceClient::new(HttpSourceClient::new(UsedHttpClient::default())),
    };
    let payload = FlatBufferTransferable::from_array_buffer(
        WebMessageTag::LayerDemMissing,
        ArrayBuffer::new(0),
    );
    let error = context.send_back(payload).expect_err("no worker global");
    assert!(matches!(error, SendError::Transmission { .. }));
    assert!(matches!(
        error
            .source()
            .expect("transport cause")
            .downcast_ref::<WebError>(),
        Some(WebError::TypeError(_))
    ));
}
