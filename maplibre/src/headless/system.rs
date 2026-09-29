use std::borrow::Cow;

use crate::{
    context::MapContext,
    render::{
        resource::{BufferedTextureHead, Head},
        Renderer,
    },
    tcs::system::{System, SystemError},
};

/// Stage which writes the current contents of the GPU/CPU buffer in [`BufferedTextureHead`]
/// to disk as PNG.
pub struct WriteSurfaceBufferSystem {
    frame: u64,
    write_to_disk: bool,
}

impl WriteSurfaceBufferSystem {
    pub fn new(write_to_disk: bool) -> Self {
        Self {
            frame: 0,
            write_to_disk,
        }
    }
}

impl System for WriteSurfaceBufferSystem {
    fn name(&self) -> Cow<'static, str> {
        "write_surfaced_buffer".into()
    }

    fn run(
        &mut self,
        MapContext {
            renderer:
                Renderer {
                    resources: state,
                    device,
                    ..
                },
            ..
        }: &mut MapContext,
    ) -> Result<(), SystemError> {
        let surface = state.surface();
        match surface.head() {
            Head::Headed(_) => Err(SystemError::Setup),
            Head::Headless(buffered_texture) => {
                let path = self
                    .write_to_disk
                    .then(|| format!("frame_{}.png", self.frame));
                write_surface_buffer_blocking(buffered_texture, device, path.as_deref())?;

                self.frame = self.frame.wrapping_add(1);
                Ok(())
            }
        }
    }
}

fn write_surface_buffer_blocking(
    texture: &BufferedTextureHead,
    device: &wgpu::Device,
    path: Option<&str>,
) -> Result<(), SystemError> {
    let slice = texture.map_blocking(device)?;
    let result = (|| {
        let bytes = slice
            .get_mapped_range()
            .map_err(crate::render::resource::BufferReadbackError::from)?;
        if let Some(path) = path {
            texture.write_png(&bytes, path)?;
        }
        Ok(())
    })();
    // All mapped views must be dropped before unmapping, including on encoding or I/O failure.
    texture.unmap();
    result
}

#[cfg(test)]
mod tests;
