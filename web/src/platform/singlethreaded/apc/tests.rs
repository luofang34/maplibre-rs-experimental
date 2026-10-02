#![allow(clippy::expect_used, clippy::panic)]

use std::{any::TypeId, error::Error};

use wasm_bindgen_test::*;

use super::*;

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
        source_client: SourceClient::new(HttpSourceClient::new(
            crate::platform::http_client::web_http_client(),
        )),
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

#[wasm_bindgen_test]
fn final_outcomes_round_trip_with_attempt_and_retry_state() {
    use maplibre::io::tile_retry::RequestDisposition;
    for kind in [RequestKind::Raster, RequestKind::Dem, RequestKind::Vector] {
        for disposition in [RequestDisposition::Complete, RequestDisposition::Retry] {
            let expected = TileRequestOutcome {
                coords: Default::default(),
                kind,
                attempt: Some(73),
                disposition,
            };
            let message = IntoMessage::into(expected);
            let message = message.with_attempt(73);
            let (tag, buffer) = prepare_message(message).expect("outcome encoded");
            let wire_tag = WebMessageTag::from_u32(tag as u32).expect("wire tag");
            let message = decode_message(wire_tag, buffer).expect("outcome decoded");
            assert!(message.has_tag(kind.message_tag()));
            assert_eq!(message.attempt(), Some(73));
            let actual = message
                .into_transferable::<TileRequestOutcome>()
                .expect("outcome payload");
            assert_eq!(actual.coords, expected.coords);
            assert_eq!(actual.kind, expected.kind);
            assert_eq!(actual.attempt, expected.attempt);
            assert_eq!(actual.disposition, expected.disposition);
        }
    }
}

#[wasm_bindgen_test]
fn malformed_outcome_payloads_retain_typed_errors() {
    let error = prepare_message(Message::new(
        RequestKind::Raster.message_tag(),
        Box::new(17_u32),
    ))
    .expect_err("wrong native payload");
    assert!(matches!(error, SendError::Payload(_)));
    let error = decode_message(
        WebMessageTag::TileRequestOutcome,
        Uint8Array::from(&[255_u8][..]).buffer(),
    )
    .expect_err("invalid encoded outcome");
    let CallError::Deserialize(cause) = error else {
        panic!("decode error");
    };
    assert!(cause.is::<serde_json::Error>());
}

#[wasm_bindgen_test]
fn tracked_vector_payloads_round_trip_without_losing_tags_or_attempt_bits() {
    for tag in [
        WebMessageTag::TileTessellated,
        WebMessageTag::LayerMissing,
        WebMessageTag::LayerTessellated,
        WebMessageTag::SymbolLayerTessellated,
        WebMessageTag::LayerIndexed,
    ] {
        let expected = [1_u8, 3, 5, 9];
        let data = FlatBufferTransferable::from_array_buffer(
            tag,
            Uint8Array::from(expected.as_slice()).buffer(),
        );
        let (wire, buffer) =
            prepare_message(IntoMessage::into(data).with_attempt(u64::MAX - 7)).expect("encoded");
        assert_eq!(wire, WebMessageTag::TrackedPayload);
        let message = decode_message(wire, buffer).expect("decoded");
        assert!(message.has_tag(tag.to_static()));
        assert_eq!(message.attempt(), Some(u64::MAX - 7));
        assert_eq!(
            message
                .into_transferable::<FlatBufferTransferable>()
                .expect("payload")
                .data(),
            expected
        );
    }
}

#[wasm_bindgen_test]
fn malformed_tracking_headers_are_typed_errors() {
    let error = decode_message(WebMessageTag::TrackedPayload, ArrayBuffer::new(11))
        .expect_err("short header");
    let CallError::Deserialize(cause) = error else {
        panic!("decode error");
    };
    assert!(cause.is::<TrackedPayloadError>());
    let mut bytes = [0_u8; 12];
    bytes[..4].copy_from_slice(&(WebMessageTag::TrackedPayload as u32).to_le_bytes());
    let error = decode_message(
        WebMessageTag::TrackedPayload,
        Uint8Array::from(bytes.as_slice()).buffer(),
    )
    .expect_err("nested envelope");
    let CallError::Deserialize(cause) = error else {
        panic!("decode error");
    };
    assert!(cause.is::<TrackedPayloadError>());
}

mod image_payloads;
