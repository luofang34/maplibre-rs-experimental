//! GPU-facing projection data shared by tile shaders.

pub use crate::render::{
    error::ProjectionStateError, resolved_view::ShaderProjectionData,
    view_state::globe_camera_for_view,
};
pub(crate) use crate::render::{
    resolved_view::{
        fixed_symbol_scale, flat_shader_projection, globe_shader_projection, PRECISE_GLOBE_ZOOM,
    },
    view_state::mercator_world_to_lat_lon,
};
use crate::{render::view_state::ViewState, style::Style};
pub(crate) mod covering;
#[cfg(test)]
pub(crate) use covering::tests::sea_eye;
pub(crate) use covering::{
    compute_globe_anchor, covering_region_with_history, eligible_globe_anchor_key,
    needs_vector_stencil_borders, style_elevated_headroom_meters, FrameLodHistory,
    GlobeCoveringAnchor,
};
pub use covering::{
    covering_region, terrain_region, view_region_for_projection, CoveringRequest,
    TerrainElevationBridge, TerrainElevationSource,
};

/// Which projection uniform a draw binds at group zero.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ProjectionBinding {
    /// The camera's projection for drawing to the screen.
    #[default]
    View,
    /// A flat identity projection for drawing into a drape texture.
    Flat,
}

/// GPU buffer and binding shared by projection-aware pipelines.
pub struct ProjectionGpuResources {
    bind_group_layout: wgpu::BindGroupLayout,
    bind_group: wgpu::BindGroup,
    buffer: wgpu::Buffer,
    flat_buffer: wgpu::Buffer,
    /// Never-updated default projection for draws into drape textures.
    flat_bind_group: wgpu::BindGroup,
    /// Latest uploaded `ShaderProjectionData::transition` (`f32::to_bits`) for pipeline selection.
    transition_bits: std::sync::atomic::AtomicU32,
    last_uploaded: Option<[u8; std::mem::size_of::<ShaderProjectionData>()]>,
}

impl Drop for ProjectionGpuResources {
    fn drop(&mut self) {
        self.buffer.destroy();
        self.flat_buffer.destroy();
    }
}

impl AsRef<wgpu::BindGroupLayout> for ProjectionGpuResources {
    #[inline]
    fn as_ref(&self) -> &wgpu::BindGroupLayout {
        &self.bind_group_layout
    }
}

impl ProjectionGpuResources {
    /// Allocates the projection uniform and its stable bind-group layout.
    pub fn new(device: &wgpu::Device, queue: &crate::render::upload_queue::UploadQueue) -> Self {
        let bind_group_layout = crate::render::resource::create_bind_group_layout_cached(
            device,
            Some("projection uniform layout"),
            &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(
                        std::mem::size_of::<ShaderProjectionData>() as u64,
                    ),
                },
                count: None,
            }],
        );
        let initial_data = ShaderProjectionData::default();
        let buffer = queue.create_buffer_init(
            device,
            &wgpu::util::BufferInitDescriptor {
                label: Some("projection uniform buffer"),
                contents: bytemuck::bytes_of(&initial_data),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            },
        );
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("projection uniform bind group"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        });
        let flat_buffer = queue.create_buffer_init(
            device,
            &wgpu::util::BufferInitDescriptor {
                label: Some("flat projection uniform buffer"),
                contents: bytemuck::bytes_of(&initial_data),
                usage: wgpu::BufferUsages::UNIFORM,
            },
        );
        let flat_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("flat projection uniform bind group"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: flat_buffer.as_entire_binding(),
            }],
        });
        Self {
            bind_group_layout,
            bind_group,
            buffer,
            flat_buffer,
            flat_bind_group,
            transition_bits: std::sync::atomic::AtomicU32::new(initial_data.transition.to_bits()),
            last_uploaded: None,
        }
    }

    /// Returns whether `binding` resolves to an active globe projection (`transition > 0.0`).
    pub fn is_globe_active(&self, binding: ProjectionBinding) -> bool {
        matches!(binding, ProjectionBinding::View)
            && f32::from_bits(
                self.transition_bits
                    .load(std::sync::atomic::Ordering::Relaxed),
            ) > 0.0
    }

    /// Returns the bind group a draw asks for: the camera's projection or the flat default.
    #[inline]
    pub fn bind_group_for(&self, binding: ProjectionBinding) -> &wgpu::BindGroup {
        match binding {
            ProjectionBinding::View => &self.bind_group,
            ProjectionBinding::Flat => &self.flat_bind_group,
        }
    }

    /// Uploads projection state for the current frame when its GPU bytes changed.
    pub fn upload(
        &mut self,
        queue: &crate::render::upload_queue::UploadQueue,
        data: ShaderProjectionData,
    ) {
        let bytes = bytemuck::bytes_of(&data);
        if self
            .last_uploaded
            .as_ref()
            .is_some_and(|prev| prev.as_slice() == bytes)
        {
            return;
        }
        self.transition_bits.store(
            data.transition.to_bits(),
            std::sync::atomic::Ordering::Relaxed,
        );
        queue.write_buffer(&self.buffer, 0, bytes);
        if let Ok(arr) = bytes.try_into() {
            self.last_uploaded = Some(arr);
        }
    }
}

/// Derives the view-wide projection uniform from style and camera state.
pub fn projection_data_for_view(
    style: &Style,
    view_state: &ViewState,
) -> Result<ShaderProjectionData, ProjectionStateError> {
    let transition = style.projection.as_ref().map_or(0.0, |specification| {
        specification
            .projection_type
            .globe_transition(view_state.zoom().value())
    });
    if transition == 0.0 {
        return Ok(flat_shader_projection(view_state));
    }
    let globe = globe_camera_for_view(view_state)?;
    globe_shader_projection(view_state, transition, &globe)
}

#[cfg(test)]
mod tests;

#[cfg(test)]
#[path = "projection/coverage/tests.rs"]
mod coverage_tests;
