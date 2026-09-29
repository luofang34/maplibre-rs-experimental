#![allow(clippy::expect_used)]

use super::*;
use std::error::Error;

#[test]
fn graph_initialization_error_exposes_its_original_cause() {
    let error = MapError::RenderGraphInit(RenderGraphError::WrongNodeType);
    assert!(matches!(
        error
            .source()
            .expect("graph cause")
            .downcast_ref::<RenderGraphError>(),
        Some(RenderGraphError::WrongNodeType)
    ));
}

#[test]
fn renderer_initialization_error_preserves_the_nested_graph_cause() {
    let error = MapError::DeviceInit(RenderError::Graph(RenderGraphError::WrongNodeType));
    let renderer = error
        .source()
        .expect("renderer cause")
        .downcast_ref::<RenderError>()
        .expect("original renderer type");
    assert!(matches!(
        renderer
            .source()
            .expect("graph cause")
            .downcast_ref::<RenderGraphError>(),
        Some(RenderGraphError::WrongNodeType)
    ));
}
