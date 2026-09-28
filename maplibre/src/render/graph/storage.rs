//! Storage, identity and lookup of render graph nodes.
mod connections;

use std::{borrow::Cow, collections::HashMap, fmt::Debug};

use super::{
    Edge, EdgeExistence, Node, NodeId, NodeLabel, NodeRunError, NodeState, RenderGraphContext,
    RenderGraphError, SlotInfo, SlotLabel,
};
use crate::{
    render::{graph::RenderContext, RenderResources},
    tcs::world::World,
};

/// The render graph configures the modular, parallel and re-usable render logic.
/// It is a retained and stateless (nodes itself my have their internal state) structure,
/// which can not be modified while it is executed by the graph runner.
///
/// The `RenderGraphRunner` is responsible for executing the entire graph each frame.
///
/// It consists of three main components: [`Nodes`](Node), [`Edges`](Edge)
/// and [`Slots`](super::SlotType).
///
/// Nodes are responsible for generating draw calls and operating on input and output slots.
/// Edges specify the order of execution for nodes and connect input and output slots together.
/// Slots describe the render resources created or used by the nodes.
///
/// Additionally a render graph can contain multiple sub graphs, which are run by the
/// corresponding nodes. Every render graph can have it’s own optional input node.
///
/// ## Example
/// Here is a simple render graph example with two nodes connected by a node edge.
/// ```
/// #
/// # use maplibre::tcs::world::World;
/// use maplibre::render::graph::{Node, NodeRunError, RenderContext, RenderGraph, RenderGraphContext};
/// # use maplibre::render::{RenderResources};
/// # struct MyNode;
/// #
/// # impl Node for MyNode {
/// #     fn run(&self,
/// #               graph: &mut RenderGraphContext,
/// #               render_context: &mut RenderContext,
/// #               state: &RenderResources,
/// #               world: &World) -> Result<(), NodeRunError> {
/// #         unimplemented!()
/// #     }
/// # }
/// #
/// let mut graph = RenderGraph::default();
/// graph.add_node("input_node", MyNode);
/// graph.add_node("output_node", MyNode);
/// graph.add_node_edge("output_node", "input_node").unwrap();
/// ```
#[derive(Default)]
pub struct RenderGraph {
    nodes: HashMap<NodeId, NodeState>,
    node_names: HashMap<Cow<'static, str>, NodeId>,
    sub_graphs: HashMap<Cow<'static, str>, RenderGraph>,
    input_node: Option<NodeId>,

    current_id: usize,
}

impl RenderGraph {
    /// The name of the [`GraphInputNode`] of this graph. Used to connect other nodes to it.
    pub const INPUT_NODE_NAME: &'static str = "GraphInputNode";

    /// Updates all nodes and sub graphs of the render graph. Should be called before executing it.
    pub fn update(&mut self, state: &mut RenderResources) {
        for node in self.nodes.values_mut() {
            node.node.update(state);
        }

        for sub_graph in self.sub_graphs.values_mut() {
            sub_graph.update(state);
        }
    }

    /// Creates an [`GraphInputNode`] with the specified slots if not already present.
    pub fn set_input(&mut self, inputs: Vec<SlotInfo>) -> NodeId {
        assert!(self.input_node.is_none(), "Graph already has an input node");

        let id = self.add_node("GraphInputNode", GraphInputNode { inputs });
        self.input_node = Some(id);
        id
    }

    /// Returns the [`NodeState`] of the input node of this graph..
    #[inline]
    pub fn input_node(&self) -> Option<&NodeState> {
        self.input_node.and_then(|id| self.get_node_state(id).ok())
    }

    /// Adds the `node` with the `name` to the graph.
    /// If the name is already present replaces it instead.
    pub fn add_node<T>(&mut self, name: impl Into<Cow<'static, str>>, node: T) -> NodeId
    where
        T: Node,
    {
        let id = NodeId::new(self.current_id);
        self.current_id = self.current_id.wrapping_add(1);
        let name = name.into();
        let mut node_state = NodeState::new(id, node);
        node_state.name = Some(name.clone());
        self.nodes.insert(id, node_state);
        self.node_names.insert(name, id);
        id
    }

    /// Removes the `node` with the `name` from the graph.
    /// If the name is does not exist, nothing happens.
    pub fn remove_node(
        &mut self,
        name: impl Into<Cow<'static, str>>,
    ) -> Result<(), RenderGraphError> {
        let name = name.into();
        if let Some(id) = self.node_names.remove(&name) {
            if let Some(node_state) = self.nodes.remove(&id) {
                // Remove all edges from other nodes to this one. Note that as we're removing this
                // node, we don't need to remove its input edges
                for input_edge in node_state.edges.input_edges().iter() {
                    match input_edge {
                        Edge::SlotEdge {
                            output_node,
                            output_index: _,
                            input_node: _,
                            input_index: _,
                        } => {
                            if let Ok(output_node) = self.get_node_state_mut(*output_node) {
                                output_node.edges.remove_output_edge(input_edge.clone())?;
                            }
                        }
                        Edge::NodeEdge {
                            input_node: _,
                            output_node,
                        } => {
                            if let Ok(output_node) = self.get_node_state_mut(*output_node) {
                                output_node.edges.remove_output_edge(input_edge.clone())?;
                            }
                        }
                    }
                }
                // Remove all edges from this node to other nodes. Note that as we're removing this
                // node, we don't need to remove its output edges
                for output_edge in node_state.edges.output_edges().iter() {
                    match output_edge {
                        Edge::SlotEdge {
                            output_node: _,
                            output_index: _,
                            input_node,
                            input_index: _,
                        } => {
                            if let Ok(input_node) = self.get_node_state_mut(*input_node) {
                                input_node.edges.remove_input_edge(output_edge.clone())?;
                            }
                        }
                        Edge::NodeEdge {
                            output_node: _,
                            input_node,
                        } => {
                            if let Ok(input_node) = self.get_node_state_mut(*input_node) {
                                input_node.edges.remove_input_edge(output_edge.clone())?;
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Retrieves the [`NodeState`] referenced by the `label`.
    pub fn get_node_state(
        &self,
        label: impl Into<NodeLabel>,
    ) -> Result<&NodeState, RenderGraphError> {
        let label = label.into();
        let node_id = self.get_node_id(&label)?;
        self.nodes
            .get(&node_id)
            .ok_or(RenderGraphError::InvalidNode(label))
    }

    /// Retrieves the [`NodeState`] referenced by the `label` mutably.
    pub fn get_node_state_mut(
        &mut self,
        label: impl Into<NodeLabel>,
    ) -> Result<&mut NodeState, RenderGraphError> {
        let label = label.into();
        let node_id = self.get_node_id(&label)?;
        self.nodes
            .get_mut(&node_id)
            .ok_or(RenderGraphError::InvalidNode(label))
    }

    /// Retrieves the [`NodeId`] referenced by the `label`.
    pub fn get_node_id(&self, label: impl Into<NodeLabel>) -> Result<NodeId, RenderGraphError> {
        let label = label.into();
        match label {
            NodeLabel::Id(id) => Ok(id),
            NodeLabel::Name(ref name) => self
                .node_names
                .get(name)
                .cloned()
                .ok_or(RenderGraphError::InvalidNode(label)),
        }
    }

    /// Retrieves the [`Node`] referenced by the `label`.
    pub fn get_node<T>(&self, label: impl Into<NodeLabel>) -> Result<&T, RenderGraphError>
    where
        T: Node,
    {
        self.get_node_state(label).and_then(|n| n.node())
    }

    /// Retrieves the [`Node`] referenced by the `label` mutably.
    pub fn get_node_mut<T>(
        &mut self,
        label: impl Into<NodeLabel>,
    ) -> Result<&mut T, RenderGraphError>
    where
        T: Node,
    {
        self.get_node_state_mut(label).and_then(|n| n.node_mut())
    }

    /// Returns an iterator over the [`NodeStates`](NodeState).
    pub fn iter_nodes(&self) -> impl Iterator<Item = &NodeState> {
        self.nodes.values()
    }

    /// Returns an iterator over the [`NodeStates`](NodeState), that allows modifying each value.
    pub fn iter_nodes_mut(&mut self) -> impl Iterator<Item = &mut NodeState> {
        self.nodes.values_mut()
    }

    /// Returns an iterator over the sub graphs.
    pub fn iter_sub_graphs(&self) -> impl Iterator<Item = (&str, &RenderGraph)> {
        self.sub_graphs
            .iter()
            .map(|(name, graph)| (name.as_ref(), graph))
    }

    /// Returns an iterator over the sub graphs, that allows modifying each value.
    pub fn iter_sub_graphs_mut(&mut self) -> impl Iterator<Item = (&str, &mut RenderGraph)> {
        self.sub_graphs
            .iter_mut()
            .map(|(name, graph)| (name.as_ref(), graph))
    }

    /// Returns an iterator over a tuple of the input edges and the corresponding output nodes
    /// for the node referenced by the label.
    pub fn iter_node_inputs(
        &self,
        label: impl Into<NodeLabel>,
    ) -> Result<impl Iterator<Item = (&Edge, &NodeState)>, RenderGraphError> {
        let node = self.get_node_state(label)?;
        Ok(node
            .edges
            .input_edges()
            .iter()
            .map(|edge| (edge, edge.get_output_node()))
            .map(move |(edge, output_node_id)| {
                (edge, self.get_node_state(output_node_id).unwrap())
            }))
    }

    /// Returns an iterator over a tuple of the output edges and the corresponding input nodes
    /// for the node referenced by the label.
    pub fn iter_node_outputs(
        &self,
        label: impl Into<NodeLabel>,
    ) -> Result<impl Iterator<Item = (&Edge, &NodeState)>, RenderGraphError> {
        let node = self.get_node_state(label)?;
        Ok(node
            .edges
            .output_edges()
            .iter()
            .map(|edge| (edge, edge.get_input_node()))
            .map(move |(edge, input_node_id)| (edge, self.get_node_state(input_node_id).unwrap())))
    }

    /// Adds the `sub_graph` with the `name` to the graph.
    /// If the name is already present replaces it instead.
    pub fn add_sub_graph(&mut self, name: impl Into<Cow<'static, str>>, sub_graph: RenderGraph) {
        self.sub_graphs.insert(name.into(), sub_graph);
    }

    /// Removes the `sub_graph` with the `name` from the graph.
    /// If the name does not exist then nothing happens.
    pub fn remove_sub_graph(&mut self, name: impl Into<Cow<'static, str>>) {
        self.sub_graphs.remove(&name.into());
    }

    /// Retrieves the sub graph corresponding to the `name`.
    pub fn get_sub_graph(&self, name: impl AsRef<str>) -> Option<&RenderGraph> {
        self.sub_graphs.get(name.as_ref())
    }

    /// Retrieves the sub graph corresponding to the `name` mutably.
    pub fn get_sub_graph_mut(&mut self, name: impl AsRef<str>) -> Option<&mut RenderGraph> {
        self.sub_graphs.get_mut(name.as_ref())
    }
}

impl Debug for RenderGraph {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for node in self.iter_nodes() {
            writeln!(f, "{:?}", node.id)?;
            writeln!(f, "  in: {:?}", node.input_slots)?;
            writeln!(f, "  out: {:?}", node.output_slots)?;
        }

        Ok(())
    }
}

/// A [`Node`] which acts as an entry point for a [`RenderGraph`] with custom inputs.
/// It has the same input and output slots and simply copies them over when run.
pub struct GraphInputNode {
    inputs: Vec<SlotInfo>,
}

impl Node for GraphInputNode {
    fn input(&self) -> Vec<SlotInfo> {
        self.inputs.clone()
    }

    fn output(&self) -> Vec<SlotInfo> {
        self.inputs.clone()
    }

    fn run(
        &self,
        graph: &mut RenderGraphContext,
        _render_context: &mut RenderContext,
        _state: &RenderResources,
        _world: &World,
    ) -> Result<(), NodeRunError> {
        for i in 0..graph.inputs().len() {
            let input = graph.inputs()[i].clone();
            graph.set_output(i, input)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
