use super::NodeId;

/// An edge, which connects two [`Nodes`](super::Node) in
/// a [`RenderGraph`](crate::render::graph::RenderGraph).
///
/// They are used to describe the ordering (which node has to run first)
/// and may be of two kinds: [`NodeEdge`](Self::NodeEdge) and [`SlotEdge`](Self::SlotEdge).
///
/// Edges are added with [`RenderGraph::add_node_edge`](super::RenderGraph::add_node_edge) and
/// [`RenderGraph::add_slot_edge`](super::RenderGraph::add_slot_edge).
///
/// The former simply states that the `output_node` has to be run before the `input_node`,
/// while the latter connects an output slot of the `output_node`
/// with an input slot of the `input_node` to pass additional data along.
/// For more information see [`SlotType`](super::SlotType).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Edge {
    /// An edge ordering both nodes (`output_node` before `input_node`)
    /// and connecting the output slot at the `output_index` of the output_node
    /// with the slot at the `input_index` of the `input_node`.
    SlotEdge {
        /// Consumer that must wait for the output resource.
        input_node: NodeId,
        /// Zero-based slot position in the consumer's input declarations.
        input_index: usize,
        /// Producer whose execution supplies the resource.
        output_node: NodeId,
        /// Zero-based slot position in the producer's output declarations.
        output_index: usize,
    },
    /// An ordering dependency (`output_node` before `input_node`) without resource transfer.
    NodeEdge {
        /// Node that must wait for the dependency.
        input_node: NodeId,
        /// Node that must execute first.
        output_node: NodeId,
    },
}

impl Edge {
    /// Returns the id of the `input_node`.
    pub fn get_input_node(&self) -> NodeId {
        match self {
            Edge::SlotEdge { input_node, .. } => *input_node,
            Edge::NodeEdge { input_node, .. } => *input_node,
        }
    }

    /// Returns the id of the `output_node`.
    pub fn get_output_node(&self) -> NodeId {
        match self {
            Edge::SlotEdge { output_node, .. } => *output_node,
            Edge::NodeEdge { output_node, .. } => *output_node,
        }
    }
}

#[derive(PartialEq, Eq)]
/// Expected connection state for [`RenderGraph::validate_edge`](super::RenderGraph::validate_edge).
pub enum EdgeExistence {
    /// Require the edge to be registered on both connected nodes.
    Exists,
    /// Require the edge to be absent before insertion.
    DoesNotExist,
}
