use std::borrow::Cow;

use thiserror::Error;

use super::{NodeState, RenderGraph, SlotInfos, SlotLabel, SlotType, SlotValue};
use crate::render::resource::TextureView;

/// A command that signals the graph runner to run the sub graph corresponding to the `name`
/// with the specified `inputs` next.
pub struct RunSubGraph {
    /// Name registered in the currently executing graph's subgraph collection.
    pub name: Cow<'static, str>,
    /// Values in the subgraph input node's declared slot order.
    pub inputs: Vec<SlotValue>,
}

/// The context with all graph information required to run a [`Node`](super::Node).
/// This context is created for each node by the `RenderGraphRunner`.
///
/// The slot input can be read from here and the outputs must be written back to the context for
/// passing them onto the next node.
///
/// Sub graphs can be queued for running by adding a [`RunSubGraph`] command to the context.
/// After the node has finished running the graph runner is responsible for executing the sub graphs.
pub struct RenderGraphContext<'a> {
    graph: &'a RenderGraph,
    node: &'a NodeState,
    inputs: &'a [SlotValue],
    outputs: &'a mut [Option<SlotValue>],
    run_sub_graphs: Vec<RunSubGraph>,
}

impl<'a> RenderGraphContext<'a> {
    /// Borrows the node's inputs and output storage for one execution.
    /// Each slice uses the corresponding declared slot order; missing storage yields a slot error.
    pub fn new(
        graph: &'a RenderGraph,
        node: &'a NodeState,
        inputs: &'a [SlotValue],
        outputs: &'a mut [Option<SlotValue>],
    ) -> Self {
        Self {
            graph,
            node,
            inputs,
            outputs,
            run_sub_graphs: Vec::new(),
        }
    }

    /// Returns the input slot values for the node.
    #[inline]
    pub fn inputs(&self) -> &[SlotValue] {
        self.inputs
    }

    /// Returns the [`SlotInfos`] of the inputs.
    pub fn input_info(&self) -> &SlotInfos {
        &self.node.input_slots
    }

    /// Returns the [`SlotInfos`] of the outputs.
    pub fn output_info(&self) -> &SlotInfos {
        &self.node.output_slots
    }

    /// Retrieves an input by name or zero-based index; missing declarations or values are errors.
    pub fn get_input(&self, label: impl Into<SlotLabel>) -> Result<&SlotValue, InputSlotError> {
        let label = label.into();
        let index = self
            .input_info()
            .get_slot_index(label.clone())
            .ok_or_else(|| InputSlotError::InvalidSlot(label.clone()))?;
        self.inputs
            .get(index)
            .ok_or(InputSlotError::InvalidSlot(label))
    }

    /// Retrieves the input slot value referenced by the `label` as a [`TextureView`].
    /// Returns an error for a missing slot or a value of another resource type.
    pub fn get_input_texture(
        &self,
        label: impl Into<SlotLabel>,
    ) -> Result<&TextureView, InputSlotError> {
        let label = label.into();
        match self.get_input(label.clone())? {
            SlotValue::TextureView(value) => Ok(value),
            value => Err(InputSlotError::MismatchedSlotType {
                label,
                actual: value.slot_type(),
                expected: SlotType::TextureView,
            }),
        }
    }

    /// Retrieves the input slot value referenced by the `label` as a [`Sampler`](wgpu::Sampler).
    /// Returns an error for a missing slot or a value of another resource type.
    pub fn get_input_sampler(
        &self,
        label: impl Into<SlotLabel>,
    ) -> Result<&wgpu::Sampler, InputSlotError> {
        let label = label.into();
        match self.get_input(label.clone())? {
            SlotValue::Sampler(value) => Ok(value),
            value => Err(InputSlotError::MismatchedSlotType {
                label,
                actual: value.slot_type(),
                expected: SlotType::Sampler,
            }),
        }
    }

    /// Retrieves the input slot value referenced by the `label` as a [`Buffer`](wgpu::Buffer).
    /// Returns an error for a missing slot or a value of another resource type.
    pub fn get_input_buffer(
        &self,
        label: impl Into<SlotLabel>,
    ) -> Result<&wgpu::Buffer, InputSlotError> {
        let label = label.into();
        match self.get_input(label.clone())? {
            SlotValue::Buffer(value) => Ok(value),
            value => Err(InputSlotError::MismatchedSlotType {
                label,
                actual: value.slot_type(),
                expected: SlotType::Buffer,
            }),
        }
    }

    /// Replaces an output by name or zero-based index after checking its declared resource type.
    /// Missing declarations, missing storage and incompatible values return errors without writing.
    pub fn set_output(
        &mut self,
        label: impl Into<SlotLabel>,
        value: impl Into<SlotValue>,
    ) -> Result<(), OutputSlotError> {
        let label = label.into();
        let value = value.into();
        let slot_index = self
            .output_info()
            .get_slot_index(label.clone())
            .ok_or_else(|| OutputSlotError::InvalidSlot(label.clone()))?;
        let slot = self
            .output_info()
            .get_slot(slot_index)
            .ok_or_else(|| OutputSlotError::InvalidSlot(label.clone()))?;
        if value.slot_type() != slot.slot_type {
            return Err(OutputSlotError::MismatchedSlotType {
                label,
                actual: value.slot_type(),
                expected: slot.slot_type,
            });
        }
        let output = self
            .outputs
            .get_mut(slot_index)
            .ok_or(OutputSlotError::InvalidSlot(label))?;
        *output = Some(value);
        Ok(())
    }

    /// Queues up a sub graph for execution after the node has finished running.
    /// Validates its name and required input types before queuing; extra inputs are ignored by
    /// graphs with an input node, while graphs without one reject any supplied inputs.
    pub fn run_sub_graph(
        &mut self,
        name: impl Into<Cow<'static, str>>,
        inputs: Vec<SlotValue>,
    ) -> Result<(), RunSubGraphError> {
        let name = name.into();
        let sub_graph = self
            .graph
            .get_sub_graph(&name)
            .ok_or_else(|| RunSubGraphError::MissingSubGraph(name.clone()))?;
        if let Some(input_node) = sub_graph.input_node() {
            for (i, input_slot) in input_node.input_slots.iter().enumerate() {
                if let Some(input_value) = inputs.get(i) {
                    if input_slot.slot_type != input_value.slot_type() {
                        return Err(RunSubGraphError::MismatchedInputSlotType {
                            graph_name: name,
                            slot_index: i,
                            actual: input_value.slot_type(),
                            expected: input_slot.slot_type,
                            label: input_slot.name.clone().into(),
                        });
                    }
                } else {
                    return Err(RunSubGraphError::MissingInput {
                        slot_index: i,
                        slot_name: input_slot.name.clone(),
                        graph_name: name,
                    });
                }
            }
        } else if !inputs.is_empty() {
            return Err(RunSubGraphError::SubGraphHasNoInputs(name));
        }

        self.run_sub_graphs.push(RunSubGraph { name, inputs });

        Ok(())
    }

    /// Finishes the context for this [`Node`](super::Node) by
    /// returning the sub graphs to run next.
    pub fn finish(self) -> Vec<RunSubGraph> {
        self.run_sub_graphs
    }
}

#[derive(Error, Debug, Eq, PartialEq)]
/// A subgraph request cannot be queued with the supplied name and input values.
pub enum RunSubGraphError {
    /// No subgraph is registered under this name.
    #[error("attempted to run sub-graph `{0}`, but it does not exist")]
    MissingSubGraph(Cow<'static, str>),
    /// Values were supplied to a subgraph without an input node.
    #[error("attempted to pass inputs to sub-graph `{0}`, which has no input slots")]
    SubGraphHasNoInputs(Cow<'static, str>),
    /// The supplied vector ends before a required input slot.
    #[error("sub graph (name: `{graph_name:?}`) could not be run because slot `{slot_name}` at index {slot_index} has no value")]
    MissingInput {
        /// Zero-based position of the missing value.
        slot_index: usize,
        /// Declared name of the missing input.
        slot_name: Cow<'static, str>,
        /// Subgraph receiving the inputs.
        graph_name: Cow<'static, str>,
    },
    /// A supplied value has a different type from its declared input slot.
    #[error("attempted to use the wrong type for input slot")]
    MismatchedInputSlotType {
        /// Subgraph receiving the inputs.
        graph_name: Cow<'static, str>,
        /// Zero-based position of the incompatible value.
        slot_index: usize,
        /// Name of the declared input slot.
        label: SlotLabel,
        /// Resource type required by the subgraph input declaration.
        expected: SlotType,
        /// Resource type of the supplied value.
        actual: SlotType,
    },
}

#[derive(Error, Debug, Eq, PartialEq)]
/// A node could not assign an output resource.
pub enum OutputSlotError {
    /// The declaration or writable storage is missing for the requested slot.
    #[error("output slot `{0:?}` does not exist")]
    InvalidSlot(SlotLabel),
    /// The supplied value cannot satisfy the output's declared type.
    #[error("attempted to output a value of type `{actual}` to output slot `{label:?}`, which has type `{expected}`")]
    MismatchedSlotType {
        /// Name or zero-based index used for the assignment.
        label: SlotLabel,
        /// Resource type declared for the output.
        expected: SlotType,
        /// Resource type of the supplied value.
        actual: SlotType,
    },
}

#[derive(Error, Debug, Eq, PartialEq)]
/// A node could not retrieve an input resource in the requested form.
pub enum InputSlotError {
    /// The declaration or supplied value is missing for the requested slot.
    #[error("input slot `{0:?}` does not exist")]
    InvalidSlot(SlotLabel),
    /// A typed getter requested a different resource type from the stored input value.
    #[error("attempted to retrieve input slot `{label:?}` as `{expected}`, but its value has type `{actual}`")]
    MismatchedSlotType {
        /// Name or zero-based index used for the lookup.
        label: SlotLabel,
        /// Resource type requested by the getter.
        expected: SlotType,
        /// Resource type of the stored value.
        actual: SlotType,
    },
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;
