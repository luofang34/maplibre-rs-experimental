#![allow(clippy::expect_used, clippy::panic)]

use super::*;
use crate::render::graph::{EmptyNode, NodeId, SlotInfo};

fn sampler_node() -> NodeState {
    let mut node = NodeState::new(NodeId::new(0), EmptyNode);
    node.input_slots = [SlotInfo::new("input", SlotType::Sampler)].into();
    node.output_slots = [SlotInfo::new("output", SlotType::Sampler)].into();
    node
}

#[test]
fn slot_lookup_rejects_indices_outside_the_declared_slots() {
    let slots: SlotInfos = [SlotInfo::new("input", SlotType::Sampler)].into();
    assert_eq!(slots.get_slot_index("input"), Some(0));
    assert_eq!(slots.get_slot_index(0), Some(0));
    for index in [1, usize::MAX] {
        assert_eq!(slots.get_slot_index(index), None);
        assert!(slots.get_slot(index).is_none());
    }
    assert_eq!(SlotInfos::default().get_slot_index(0), None);
}

#[test]
fn input_lookup_returns_errors_for_missing_slots_or_values() {
    let graph = RenderGraph::default();
    let node = sampler_node();
    let mut outputs = [None];
    let context = RenderGraphContext::new(&graph, &node, &[], &mut outputs);
    for index in [0, 1, usize::MAX] {
        assert!(matches!(
            context.get_input(index),
            Err(InputSlotError::InvalidSlot(SlotLabel::Index(actual))) if actual == index
        ));
    }
    assert_eq!(
        context
            .get_input_sampler("input")
            .expect_err("missing value"),
        InputSlotError::InvalidSlot("input".into())
    );
}

async fn device() -> wgpu::Device {
    let instance = wgpu::Instance::default();
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions::default())
        .await
        .expect("GPU adapter");
    adapter
        .request_device(&wgpu::DeviceDescriptor::default())
        .await
        .expect("GPU device")
        .0
}

#[tokio::test]
async fn slot_type_errors_distinguish_supplied_values_from_declared_types() {
    let device = device().await;
    let sampler = SlotValue::from(device.create_sampler(&wgpu::SamplerDescriptor::default()));
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 4,
        usage: wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let graph = RenderGraph::default();
    let node = sampler_node();
    let inputs = [sampler.clone()];
    let mut outputs = [None];
    let mut context = RenderGraphContext::new(&graph, &node, &inputs, &mut outputs);
    assert!(context.get_input_sampler("input").is_ok());
    let input_error = context
        .get_input_buffer(0)
        .expect_err("sampler is not a buffer");
    assert_eq!(
        input_error,
        InputSlotError::MismatchedSlotType {
            label: 0.into(),
            expected: SlotType::Buffer,
            actual: SlotType::Sampler,
        }
    );
    assert_eq!(
        input_error.to_string(),
        "attempted to retrieve input slot `Index(0)` as `Buffer`, but its value has type `Sampler`"
    );
    assert_eq!(
        context.set_output("output", buffer),
        Err(OutputSlotError::MismatchedSlotType {
            label: "output".into(),
            expected: SlotType::Sampler,
            actual: SlotType::Buffer,
        })
    );
    assert!(context.set_output("output", sampler).is_ok());
    assert!(matches!(outputs[0], Some(SlotValue::Sampler(_))));
}

#[tokio::test]
async fn output_lookup_returns_errors_for_missing_slots_or_storage() {
    let device = device().await;
    let sampler = SlotValue::from(device.create_sampler(&wgpu::SamplerDescriptor::default()));
    let graph = RenderGraph::default();
    let node = sampler_node();
    let mut outputs = [None];
    let mut context = RenderGraphContext::new(&graph, &node, &[], &mut outputs);
    for index in [1, usize::MAX] {
        assert_eq!(
            context.set_output(index, sampler.clone()),
            Err(OutputSlotError::InvalidSlot(index.into()))
        );
    }
    let mut context = RenderGraphContext::new(&graph, &node, &[], &mut []);
    assert_eq!(
        context.set_output("output", sampler),
        Err(OutputSlotError::InvalidSlot("output".into()))
    );
}
