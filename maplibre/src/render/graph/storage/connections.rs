//! Connections and slot compatibility of render graph nodes.
use super::*;

impl RenderGraph {
    /// Adds the [`Edge::SlotEdge`] to the graph. This guarantees that the `output_node`
    /// is run before the `input_node` and also connects the `output_slot` to the `input_slot`.
    pub fn add_slot_edge(
        &mut self,
        output_node: impl Into<NodeLabel>,
        output_slot: impl Into<SlotLabel>,
        input_node: impl Into<NodeLabel>,
        input_slot: impl Into<SlotLabel>,
    ) -> Result<(), RenderGraphError> {
        let output_slot = output_slot.into();
        let input_slot = input_slot.into();
        let output_node_id = self.get_node_id(output_node)?;
        let input_node_id = self.get_node_id(input_node)?;

        let output_index = self
            .get_node_state(output_node_id)?
            .output_slots
            .get_slot_index(output_slot.clone())
            .ok_or(RenderGraphError::InvalidOutputNodeSlot(output_slot))?;
        let input_index = self
            .get_node_state(input_node_id)?
            .input_slots
            .get_slot_index(input_slot.clone())
            .ok_or(RenderGraphError::InvalidInputNodeSlot(input_slot))?;

        let edge = Edge::SlotEdge {
            output_node: output_node_id,
            output_index,
            input_node: input_node_id,
            input_index,
        };

        self.validate_edge(&edge, EdgeExistence::DoesNotExist)?;

        {
            let output_node = self.get_node_state_mut(output_node_id)?;
            output_node.edges.add_output_edge(edge.clone())?;
        }
        let input_node = self.get_node_state_mut(input_node_id)?;
        input_node.edges.add_input_edge(edge)?;

        Ok(())
    }

    /// Removes a resource connection, returning an error if its nodes, slots or edge are absent.
    pub fn remove_slot_edge(
        &mut self,
        output_node: impl Into<NodeLabel>,
        output_slot: impl Into<SlotLabel>,
        input_node: impl Into<NodeLabel>,
        input_slot: impl Into<SlotLabel>,
    ) -> Result<(), RenderGraphError> {
        let output_slot = output_slot.into();
        let input_slot = input_slot.into();
        let output_node_id = self.get_node_id(output_node)?;
        let input_node_id = self.get_node_id(input_node)?;

        let output_index = self
            .get_node_state(output_node_id)?
            .output_slots
            .get_slot_index(output_slot.clone())
            .ok_or(RenderGraphError::InvalidOutputNodeSlot(output_slot))?;
        let input_index = self
            .get_node_state(input_node_id)?
            .input_slots
            .get_slot_index(input_slot.clone())
            .ok_or(RenderGraphError::InvalidInputNodeSlot(input_slot))?;

        let edge = Edge::SlotEdge {
            output_node: output_node_id,
            output_index,
            input_node: input_node_id,
            input_index,
        };

        self.validate_edge(&edge, EdgeExistence::Exists)?;

        {
            let output_node = self.get_node_state_mut(output_node_id)?;
            output_node.edges.remove_output_edge(edge.clone())?;
        }
        let input_node = self.get_node_state_mut(input_node_id)?;
        input_node.edges.remove_input_edge(edge)?;

        Ok(())
    }

    /// Adds the [`Edge::NodeEdge`] to the graph. This guarantees that the `output_node`
    /// is run before the `input_node`.
    pub fn add_node_edge(
        &mut self,
        output_node: impl Into<NodeLabel>,
        input_node: impl Into<NodeLabel>,
    ) -> Result<(), RenderGraphError> {
        let output_node_id = self.get_node_id(output_node)?;
        let input_node_id = self.get_node_id(input_node)?;

        let edge = Edge::NodeEdge {
            output_node: output_node_id,
            input_node: input_node_id,
        };

        self.validate_edge(&edge, EdgeExistence::DoesNotExist)?;

        {
            let output_node = self.get_node_state_mut(output_node_id)?;
            output_node.edges.add_output_edge(edge.clone())?;
        }
        let input_node = self.get_node_state_mut(input_node_id)?;
        input_node.edges.add_input_edge(edge)?;

        Ok(())
    }

    /// Removes an ordering dependency, returning an error if its nodes or edge are absent.
    pub fn remove_node_edge(
        &mut self,
        output_node: impl Into<NodeLabel>,
        input_node: impl Into<NodeLabel>,
    ) -> Result<(), RenderGraphError> {
        let output_node_id = self.get_node_id(output_node)?;
        let input_node_id = self.get_node_id(input_node)?;

        let edge = Edge::NodeEdge {
            output_node: output_node_id,
            input_node: input_node_id,
        };

        self.validate_edge(&edge, EdgeExistence::Exists)?;

        {
            let output_node = self.get_node_state_mut(output_node_id)?;
            output_node.edges.remove_output_edge(edge.clone())?;
        }
        let input_node = self.get_node_state_mut(input_node_id)?;
        input_node.edges.remove_input_edge(edge)?;

        Ok(())
    }

    /// Checks the expected edge presence and, for slot edges, declarations and single-producer
    /// compatibility. This does not check the graph for cycles.
    pub fn validate_edge(
        &mut self,
        edge: &Edge,
        should_exist: EdgeExistence,
    ) -> Result<(), RenderGraphError> {
        if should_exist == EdgeExistence::Exists && !self.has_edge(edge) {
            return Err(RenderGraphError::EdgeDoesNotExist(edge.clone()));
        } else if should_exist == EdgeExistence::DoesNotExist && self.has_edge(edge) {
            return Err(RenderGraphError::EdgeAlreadyExists(edge.clone()));
        }

        match *edge {
            Edge::SlotEdge {
                output_node,
                output_index,
                input_node,
                input_index,
            } => {
                let output_node_state = self.get_node_state(output_node)?;
                let input_node_state = self.get_node_state(input_node)?;

                let output_slot = output_node_state
                    .output_slots
                    .get_slot(output_index)
                    .ok_or(RenderGraphError::InvalidOutputNodeSlot(SlotLabel::Index(
                        output_index,
                    )))?;
                let input_slot = input_node_state.input_slots.get_slot(input_index).ok_or(
                    RenderGraphError::InvalidInputNodeSlot(SlotLabel::Index(input_index)),
                )?;

                if let Some(Edge::SlotEdge {
                    output_node: current_output_node,
                    ..
                }) = input_node_state.edges.input_edges().iter().find(|e| {
                    if let Edge::SlotEdge {
                        input_index: current_input_index,
                        ..
                    } = e
                    {
                        input_index == *current_input_index
                    } else {
                        false
                    }
                }) {
                    if should_exist == EdgeExistence::DoesNotExist {
                        return Err(RenderGraphError::NodeInputSlotAlreadyOccupied {
                            node: input_node,
                            input_slot: input_index,
                            occupied_by_node: *current_output_node,
                        });
                    }
                }

                if output_slot.slot_type != input_slot.slot_type {
                    return Err(RenderGraphError::MismatchedNodeSlots {
                        output_node,
                        output_slot: output_index,
                        input_node,
                        input_slot: input_index,
                    });
                }
            }
            Edge::NodeEdge { .. } => { /* nothing to validate here */ }
        }

        Ok(())
    }

    /// Checks whether the `edge` already exists in the graph.
    pub fn has_edge(&self, edge: &Edge) -> bool {
        let output_node_state = self.get_node_state(edge.get_output_node());
        let input_node_state = self.get_node_state(edge.get_input_node());
        if let Ok(output_node_state) = output_node_state {
            if output_node_state.edges.output_edges().contains(edge) {
                if let Ok(input_node_state) = input_node_state {
                    if input_node_state.edges.input_edges().contains(edge) {
                        return true;
                    }
                }
            }
        }

        false
    }
}
