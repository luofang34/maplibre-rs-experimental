//! GPU resource dependencies, node execution contracts and nested render graphs.

#![deny(missing_docs)]

pub use context::*;
pub use edge::*;
pub use node::*;
pub use node_slot::*;
pub use storage::*;
use thiserror::Error;

mod context;
mod edge;
mod node;
mod node_slot;
mod storage;

#[derive(Error, Debug, Eq, PartialEq)]
/// A node lookup or connection violates the graph's declared topology or slot types.
pub enum RenderGraphError {
    /// The supplied node name or ID cannot be resolved to a stored node.
    #[error("node does not exist")]
    InvalidNode(NodeLabel),
    /// The producing node has no output with the supplied label.
    #[error("output node slot does not exist")]
    InvalidOutputNodeSlot(SlotLabel),
    /// The consuming node has no input with the supplied label.
    #[error("input node slot does not exist")]
    InvalidInputNodeSlot(SlotLabel),
    /// A typed lookup requested a different concrete [`Node`] implementation.
    #[error("node does not match the given type")]
    WrongNodeType,
    /// A connection joins slots with different GPU resource types.
    #[error("attempted to connect a node output slot to an incompatible input node slot")]
    MismatchedNodeSlots {
        /// Node that produces the resource.
        output_node: NodeId,
        /// Zero-based output slot index on the producing node.
        output_slot: usize,
        /// Node that consumes the resource.
        input_node: NodeId,
        /// Zero-based input slot index on the consuming node.
        input_slot: usize,
    },
    /// An insertion or absence check found the same edge already connected.
    #[error("attempted to add an edge that already exists")]
    EdgeAlreadyExists(Edge),
    /// Removal or an existence check could not find the edge.
    #[error("attempted to remove an edge that does not exist")]
    EdgeDoesNotExist(Edge),
    /// A required input has no edge supplying its value.
    #[error("node has an unconnected input slot")]
    UnconnectedNodeInputSlot {
        /// Node requiring the input.
        node: NodeId,
        /// Zero-based index of the unconnected input.
        input_slot: usize,
    },
    /// Output validation found no edge consuming a declared output.
    #[error("node has an unconnected output slot")]
    UnconnectedNodeOutputSlot {
        /// Node producing the output.
        node: NodeId,
        /// Zero-based index of the unconnected output.
        output_slot: usize,
    },
    /// A second producer attempted to connect to an input that already has a producer.
    #[error("node input slot already occupied")]
    NodeInputSlotAlreadyOccupied {
        /// Node consuming the occupied input.
        node: NodeId,
        /// Zero-based index of the occupied input.
        input_slot: usize,
        /// Producer already connected to the input.
        occupied_by_node: NodeId,
    },
}
