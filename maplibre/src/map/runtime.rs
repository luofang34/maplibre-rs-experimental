//! Runtime frame loop, plugins, stage systems, and memory/retention orchestration.

pub(crate) mod binned_phase_plugin;
pub mod frame;
pub(crate) mod render_plugin;
mod resource_system;
pub(crate) mod retention_system;
mod stage_systems;
pub(crate) mod tile_memory;
mod tile_view_pattern_system;

pub use binned_phase_plugin::BinnedRenderPhasePlugin;
pub use render_plugin::RenderPlugin;
