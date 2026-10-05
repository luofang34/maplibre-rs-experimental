//! A shield's SVG as straight-alpha pixels for the label atlas.

use maplibre::{sdf::assets::ProvidedImage, style::StyleImage};
use resvg::{
    tiny_skia::{Pixmap, Transform},
    usvg::{ImageHrefResolver, Options, Tree},
};
use roadshield::ShieldSymbol;

use crate::ShieldRenderError;

/// The longest side, in device pixels, a shield may have; a label image beyond it is a
/// malformed rule, not a shield.
const MAX_SIDE: u32 = 1024;

/// Draws `symbol` at `pixel_ratio` device pixels per layout pixel, anchored at its shield
/// body so banners above it do not move the body off the road.
pub(crate) fn rasterize(
    symbol: &ShieldSymbol,
    pixel_ratio: f32,
) -> Result<ProvidedImage, ShieldRenderError> {
    // A shield is self-contained; an SVG that reaches for other files or URLs draws nothing
    // from them.
    let options = Options {
        resources_dir: None,
        image_href_resolver: ImageHrefResolver {
            resolve_data: Box::new(|_, _, _| None),
            resolve_string: Box::new(|_, _| None),
        },
        ..Options::default()
    };
    let tree = Tree::from_str(&symbol.svg, &options).map_err(|source| ShieldRenderError::Svg {
        key: symbol.semantic_key.clone(),
        source,
    })?;
    let ratio = f64::from(pixel_ratio);
    let side = |logical: f64| (logical * ratio).ceil().max(1.0).min(f64::from(u32::MAX)) as u32;
    let (width, height) = (side(symbol.width), side(symbol.height));
    let too_large = || ShieldRenderError::TooLarge {
        key: symbol.semantic_key.clone(),
        width,
        height,
    };
    if width > MAX_SIDE || height > MAX_SIDE {
        return Err(too_large());
    }
    let mut pixmap = Pixmap::new(width, height).ok_or_else(too_large)?;
    resvg::render(
        &tree,
        Transform::from_scale(pixel_ratio, pixel_ratio),
        &mut pixmap.as_mut(),
    );
    // The atlas premultiplies colour images itself and takes straight alpha.
    let data = pixmap
        .pixels()
        .iter()
        .flat_map(|pixel| {
            let pixel = pixel.demultiply();
            [pixel.red(), pixel.green(), pixel.blue(), pixel.alpha()]
        })
        .collect();
    let (anchor_x, anchor_y) = symbol.anchor;
    Ok(ProvidedImage {
        image: StyleImage {
            width,
            height,
            data,
            pixel_ratio,
            sdf: false,
        },
        anchor: Some([(anchor_x * ratio) as f32, (anchor_y * ratio) as f32]),
    })
}
