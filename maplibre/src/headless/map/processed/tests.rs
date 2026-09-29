#![allow(clippy::expect_used, clippy::panic)]

use std::error::Error;

use super::*;

#[test]
fn wrong_vector_and_symbol_payloads_return_their_message_context() {
    for tag in [VectorLayer::message_tag(), SymbolLayer::message_tag()] {
        let error = ProcessedLayers::from_messages(vec![Message::new(tag, Box::new(17_u32))])
            .expect_err("payload does not match layer type");
        let source = error
            .source()
            .expect("payload cause")
            .downcast_ref::<crate::io::apc::MessageError>()
            .expect("typed payload cause");
        assert_eq!(source.actual, std::any::TypeId::of::<u32>());
        assert_eq!(source.tag, tag);
    }
}
