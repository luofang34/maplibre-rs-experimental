use crate::style::{property::StyleProperty, Style};

/// What fills a line instead of its colour, when something does.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) enum LineImage {
    /// The line takes its colour from the layer.
    #[default]
    None,
    /// Colours along the line, from `line-gradient`.
    Gradient(Vec<[u8; 4]>),
    /// An image repeated along the line, from `line-pattern`: its size in layout pixels and its
    /// RGBA bytes.
    Pattern {
        width: u32,
        height: u32,
        display: [f32; 2],
        data: Vec<u8>,
    },
}

/// The uniform of a dash entry: the dash period, what fills the line, and the size of a pattern
/// or, for a dash, whether its texture has the rows of round caps and how far the fractional
/// zoom stretches it.
pub(super) fn image_uniform(period: f32, image: &LineImage, round: bool, scale: f32) -> [f32; 4] {
    let round = f32::from(round);
    match image {
        LineImage::None => [period, 0.0, round, scale],
        // A stepped gradient's ramp is spread over the progress on a logarithmic scale.
        LineImage::Gradient(texels)
            if texels.len() == crate::style::line_gradient::STEP_RAMP_TEXELS =>
        {
            [period, 1.25, round, scale]
        }
        LineImage::Gradient(_) => [period, 1.0, round, scale],
        LineImage::Pattern { display, .. } => [period, 2.0 + scale, display[0], display[1]],
    }
}

/// The image a line layer draws instead of its colour at a zoom.
pub(super) fn line_image(
    paint: &crate::style::layer::LinePaint,
    style: &Style,
    zoom: f64,
) -> LineImage {
    if let Some(gradient) = &paint.line_gradient {
        return LineImage::Gradient(crate::style::line_gradient::ramp(gradient));
    }
    let Some(pattern) = &paint.line_pattern else {
        return LineImage::None;
    };
    let name = StyleProperty::<crate::style::layer::TextField>::parse(pattern)
        .evaluate_at_zoom(zoom)
        .map(|name| name.0);
    name.and_then(|name| style.images.get(&name))
        .and_then(pattern_image)
        .unwrap_or(LineImage::None)
}

/// The pattern an image makes, when its pixels fit its size.
pub(super) fn pattern_image(image: &crate::style::StyleImage) -> Option<LineImage> {
    if image.width == 0
        || image.height == 0
        || image.data.len() != image.width as usize * image.height as usize * 4
    {
        return None;
    }
    let ratio = image.pixel_ratio.max(0.01);
    Some(LineImage::Pattern {
        width: image.width,
        height: image.height,
        display: [image.width as f32 / ratio, image.height as f32 / ratio],
        data: super::super::pattern::premultiplied(&image.data),
    })
}

/// The gradient ramp or pattern as a texture, or one white texel for a plain line.
pub(super) fn ramp_texture(
    device: &wgpu::Device,
    queue: &crate::render::upload_queue::UploadQueue,
    image: &LineImage,
) -> (wgpu::Texture, wgpu::TextureView, wgpu::Sampler) {
    let white = [255_u8; 4];
    let (width, height, bytes): (u32, u32, &[u8]) = match image {
        LineImage::None => (1, 1, &white),
        LineImage::Gradient(texels) => (texels.len() as u32, 1, bytemuck::cast_slice(texels)),
        LineImage::Pattern {
            width,
            height,
            data,
            ..
        } => (*width, *height, data),
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("line image"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        texture.as_image_copy(),
        bytes,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width * 4),
            rows_per_image: Some(height),
        },
        texture.size(),
    );
    // The hard edges of a stepped gradient stay hard.
    let filter = match image {
        LineImage::Gradient(texels) if texels.len() > crate::style::line_gradient::RAMP_TEXELS => {
            wgpu::FilterMode::Nearest
        }
        _ => wgpu::FilterMode::Linear,
    };
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        mag_filter: filter,
        min_filter: filter,
        ..Default::default()
    });
    let view = texture.create_view(&Default::default());
    (texture, view, sampler)
}
