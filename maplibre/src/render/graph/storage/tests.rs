use std::collections::HashSet;

use super::{
    Edge, Node, NodeId, NodeRunError, RenderGraph, RenderGraphContext, RenderGraphError, SlotInfo,
};
use crate::{
    render::{
        graph::{RenderContext, SlotType},
        RenderResources,
    },
    tcs::world::World,
};

#[derive(Debug)]
struct TestNode {
    inputs: Vec<SlotInfo>,
    outputs: Vec<SlotInfo>,
}

impl TestNode {
    pub fn new(inputs: usize, outputs: usize) -> Self {
        TestNode {
            inputs: (0..inputs)
                .map(|i| SlotInfo::new(format!("in_{i}"), SlotType::TextureView))
                .collect(),
            outputs: (0..outputs)
                .map(|i| SlotInfo::new(format!("out_{i}"), SlotType::TextureView))
                .collect(),
        }
    }
}

impl Node for TestNode {
    fn input(&self) -> Vec<SlotInfo> {
        self.inputs.clone()
    }

    fn output(&self) -> Vec<SlotInfo> {
        self.outputs.clone()
    }

    fn run(
        &self,
        _graph: &mut RenderGraphContext,
        _render_context: &mut RenderContext,
        _state: &RenderResources,
        _world: &World,
    ) -> Result<(), NodeRunError> {
        Ok(())
    }
}

#[test]
fn test_graph_edges() {
    let mut graph = RenderGraph::default();
    let a_id = graph.add_node("A", TestNode::new(0, 1));
    let b_id = graph.add_node("B", TestNode::new(0, 1));
    let c_id = graph.add_node("C", TestNode::new(1, 1));
    let d_id = graph.add_node("D", TestNode::new(1, 0));

    graph.add_slot_edge("A", "out_0", "C", "in_0").unwrap();
    graph.add_node_edge("B", "C").unwrap();
    graph.add_slot_edge("C", 0, "D", 0).unwrap();

    fn input_nodes(name: &'static str, graph: &RenderGraph) -> HashSet<NodeId> {
        graph
            .iter_node_inputs(name)
            .unwrap()
            .map(|(_edge, node)| node.id)
            .collect::<HashSet<NodeId>>()
    }

    fn output_nodes(name: &'static str, graph: &RenderGraph) -> HashSet<NodeId> {
        graph
            .iter_node_outputs(name)
            .unwrap()
            .map(|(_edge, node)| node.id)
            .collect::<HashSet<NodeId>>()
    }

    assert!(input_nodes("A", &graph).is_empty(), "A has no inputs");
    assert_eq!(
        output_nodes("A", &graph),
        HashSet::from_iter(vec![c_id]),
        "A outputs to C"
    );

    assert!(input_nodes("B", &graph).is_empty(), "B has no inputs");
    assert_eq!(
        output_nodes("B", &graph),
        HashSet::from_iter(vec![c_id]),
        "B outputs to C"
    );

    assert_eq!(
        input_nodes("C", &graph),
        HashSet::from_iter(vec![a_id, b_id]),
        "A and B input to C"
    );
    assert_eq!(
        output_nodes("C", &graph),
        HashSet::from_iter(vec![d_id]),
        "C outputs to D"
    );

    assert_eq!(
        input_nodes("D", &graph),
        HashSet::from_iter(vec![c_id]),
        "C inputs to D"
    );
    assert!(output_nodes("D", &graph).is_empty(), "D has no outputs");
}

#[test]
fn test_get_node_typed() {
    struct MyNode {
        value: usize,
    }

    impl Node for MyNode {
        fn run(
            &self,
            _graph: &mut RenderGraphContext,
            _render_context: &mut RenderContext,
            _state: &RenderResources,
            _world: &World,
        ) -> Result<(), NodeRunError> {
            Ok(())
        }
    }

    let mut graph = RenderGraph::default();

    graph.add_node("A", MyNode { value: 42 });

    let node: &MyNode = graph.get_node("A").unwrap();
    assert_eq!(node.value, 42, "node value matches");

    let result: Result<&TestNode, RenderGraphError> = graph.get_node("A");
    assert_eq!(
        result.unwrap_err(),
        RenderGraphError::WrongNodeType,
        "expect a wrong node type error"
    );
}

#[test]
fn test_slot_already_occupied() {
    let mut graph = RenderGraph::default();

    graph.add_node("A", TestNode::new(0, 1));
    graph.add_node("B", TestNode::new(0, 1));
    graph.add_node("C", TestNode::new(1, 1));

    graph.add_slot_edge("A", 0, "C", 0).unwrap();
    assert_eq!(
        graph.add_slot_edge("B", 0, "C", 0),
        Err(RenderGraphError::NodeInputSlotAlreadyOccupied {
            node: graph.get_node_id("C").unwrap(),
            input_slot: 0,
            occupied_by_node: graph.get_node_id("A").unwrap(),
        }),
        "Adding to a slot that is already occupied should return an error"
    );
}

#[test]
fn test_edge_already_exists() {
    let mut graph = RenderGraph::default();

    graph.add_node("A", TestNode::new(0, 1));
    graph.add_node("B", TestNode::new(1, 0));

    graph.add_slot_edge("A", 0, "B", 0).unwrap();
    assert_eq!(
        graph.add_slot_edge("A", 0, "B", 0),
        Err(RenderGraphError::EdgeAlreadyExists(Edge::SlotEdge {
            output_node: graph.get_node_id("A").unwrap(),
            output_index: 0,
            input_node: graph.get_node_id("B").unwrap(),
            input_index: 0,
        })),
        "Adding to a duplicate edge should return an error"
    );
}

#[test]
fn node_ids_wrap_without_overflowing() {
    let mut graph = RenderGraph {
        current_id: usize::MAX,
        ..Default::default()
    };
    let last = graph.add_node("last", TestNode::new(0, 0));
    let wrapped = graph.add_node("wrapped", TestNode::new(0, 0));
    assert_eq!(last, NodeId::new(usize::MAX));
    assert_eq!(wrapped, NodeId::new(0));
    assert!(graph.get_node_state(last).is_ok());
    assert!(graph.get_node_state(wrapped).is_ok());
}
