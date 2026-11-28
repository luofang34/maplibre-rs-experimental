//! Symbol bounds measured once during layout, shared by collision and near-plane clipping.
use serde::{Deserialize, Serialize};

use crate::render::shaders::ShaderSymbolVertex;

/// Pixel bounds and alignment of a text or icon component at its layout size.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct SymbolBounds {
    /// Left, top, right, bottom relative to the anchor.
    pub bounds: [f64; 4],
    /// Height offset in metres.
    pub height: f64,
    /// Map tangent in radians.
    pub angle: f64,
    /// Whether these bounds belong to glyphs.
    pub text: bool,
}

pub(crate) fn measure(vertices: &mut [ShaderSymbolVertex]) -> [Option<SymbolBounds>; 3] {
    let mut parts: [Option<SymbolBounds>; 3] = [None, None, None];
    for vertex in vertices.iter() {
        let kind = vertex.kind();
        let Some(slot) = parts.get_mut(kind as usize) else {
            continue;
        };
        let part = slot.get_or_insert_with(|| SymbolBounds {
            bounds: [
                f64::INFINITY,
                f64::INFINITY,
                f64::NEG_INFINITY,
                f64::NEG_INFINITY,
            ],
            height: f64::from(vertex.height()),
            angle: f64::from(vertex.angle()),
            text: kind == 0,
        });
        let [x, y] = vertex.offset_px();
        part.bounds[0] = part.bounds[0].min(x);
        part.bounds[1] = part.bounds[1].min(y);
        part.bounds[2] = part.bounds[2].max(x);
        part.bounds[3] = part.bounds[3].max(y);
    }
    for vertex in vertices {
        if let Some(part) = parts.get(vertex.kind() as usize).copied().flatten() {
            let radius = part.bounds[0]
                .abs()
                .max(part.bounds[2].abs())
                .hypot(part.bounds[1].abs().max(part.bounds[3].abs()));
            vertex.set_radius(radius);
        }
    }
    parts
}
