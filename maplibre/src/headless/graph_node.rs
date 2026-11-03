use crate::{
    render::{
        graph::{Node, RenderContext},
        resource::Head,
        RenderResources,
    },
    tcs::world::World,
};

/// Node which copies the contents of the GPU-side texture in [`BufferedTextureHead`] to an
/// unmapped GPU-side buffer. This buffer will be mapped in
/// [`crate::render::stages::write_surface_buffer_stage::WriteSurfaceBufferStage`].
#[derive(Default)]
pub struct CopySurfaceBufferNode;

impl Node for CopySurfaceBufferNode {
    fn run(
        &self,
        RenderContext {
            device,
            command_encoder,
        }: &mut RenderContext,
        state: &RenderResources,
        _world: &World,
    ) {
        let surface = state.surface();
        match surface.head() {
            Head::Headed(_) => {}
            Head::Headless(buffered_texture) => {
                let size = surface.size();
                command_encoder.copy_texture_to_buffer(
                    buffered_texture.copy_texture(),
                    wgpu::TexelCopyBufferInfo {
                        buffer: buffered_texture.ensure_buffer(device),
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(buffered_texture.bytes_per_row()),
                            rows_per_image: None,
                        },
                    },
                    wgpu::Extent3d {
                        width: size.width(),
                        height: size.height(),
                        depth_or_array_layers: 1,
                    },
                );
            }
        }
    }
}
