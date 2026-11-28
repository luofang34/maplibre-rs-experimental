//! Symbol anchor/offset primitives (`CollectedSymbol`, `anchor_fractions`, `text-offset`,
//! `text-radial-offset`, `text-variable-anchor`, and shared quad emission).
use geo_types::Point;
use lyon::tessellation::VertexBuffers;

use crate::{
    render::shaders::ShaderSymbolVertex,
    sdf::assets::AtlasEntry,
    style::{expression::FeatureProperties, layer::SymbolPaint},
};

/// A polyline in tile units and the arc length from its start to a label's anchor.
pub(super) type LineContext = (std::sync::Arc<[[f32; 2]]>, f32);

/// The source feature a label is drawn for, as a query reports it.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct SourceFeature {
    pub id: Option<u64>,
    pub kind: crate::style::filter::GeometryType,
    pub feature_index: Option<u32>,
}

pub(super) struct CollectedSymbol {
    pub source: SourceFeature,
    pub line: Option<LineContext>,
    pub anchor: Point<f64>,
    pub properties: std::sync::Arc<FeatureProperties>,
    pub angle: f32,
    /// How the text is written when the style leaves it open: `Some(true)` vertically.
    pub vertical: Option<bool>,
    /// Whether the label is the second way of writing a text that has two.
    pub fallback: bool,
    /// What makes the label a repeat of the others along lines, see `layout::repeat_key`.
    pub repeat_key: Option<std::sync::Arc<str>>,
}

/// The `[x, y]` offset a layout property gives one symbol, times `scale`; zero without one.
pub(super) fn offset(
    paint: &SymbolPaint,
    name: &str,
    scale: f32,
    (properties, zoom): (&FeatureProperties, f64),
) -> [f32; 2] {
    let Some(value) = paint.properties.get(name) else {
        return [0.0; 2];
    };
    // A written pair is the common case and needs no expression.
    let [x, y] = match crate::style::translation::Pair::from_json(value) {
        Some(pair) => pair.0,
        None => {
            crate::style::property::StyleProperty::<crate::style::translation::Pair>::parse(value)
                .evaluate_for(properties, zoom)
                .map_or([0.0; 2], |pair| pair.0)
        }
    };
    [x as f32 * scale, y as f32 * scale]
}

pub(super) fn anchor_fractions(anchor: &str) -> [f32; 2] {
    [
        if anchor.contains("left") {
            0.0
        } else if anchor.contains("right") {
            1.0
        } else {
            0.5
        },
        if anchor.contains("top") {
            0.0
        } else if anchor.contains("bottom") {
            1.0
        } else {
            0.5
        },
    ]
}

pub(super) fn quad(
    buffer: &mut VertexBuffers<ShaderSymbolVertex, u32>,
    anchor: Point<f64>,
    bounds: [f32; 4],
    image: &AtlasEntry,
    height: f32,
    angle: f32,
    rotation: f32,
) {
    let base = buffer.vertices.len() as u32;
    let (sin, cos) = rotation.sin_cos();
    let [x, y, width, height_pixels] = image.rect;
    for (index, (u, v)) in [
        (x, y),
        (x + width, y),
        (x + width, y + height_pixels),
        (x, y + height_pixels),
    ]
    .into_iter()
    .enumerate()
    {
        let px = if index == 0 || index == 3 {
            bounds[0]
        } else {
            bounds[2]
        };
        let py = if index < 2 { bounds[1] } else { bounds[3] };
        // The quad turns about the anchor, offsets included, as GL JS rotates its corners.
        let (px, py) = (px * cos - py * sin, px * sin + py * cos);
        buffer.vertices.push(ShaderSymbolVertex::new(
            [anchor.x().round() as i32, anchor.y().round() as i32],
            [(px * 32.0).round() as i32, (py * 32.0).round() as i32],
            [u, v],
            image.kind,
            height,
            angle,
        ));
    }
    buffer
        .indices
        .extend([base, base + 1, base + 2, base, base + 2, base + 3]);
}

/// The glyphs appear to start at the baseline, which the text box edge sits above by this much.
const BASELINE: f32 = 7.0;

/// The anchors a label may take, best first: the entries of `text-variable-anchor-offset`, or
/// `text-variable-anchor`; empty for a label with one anchor.
pub(super) fn variable_anchors(paint: &SymbolPaint) -> Vec<String> {
    if let Some(pairs) = anchor_offsets(paint) {
        return pairs.into_iter().map(|(anchor, _)| anchor).collect();
    }
    paint
        .properties
        .get("text-variable-anchor")
        .and_then(|value| value.as_array())
        .map(|anchors| {
            anchors
                .iter()
                .filter_map(|anchor| anchor.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// `text-variable-anchor-offset` as written: each anchor with its offset in ems.
fn anchor_offsets(paint: &SymbolPaint) -> Option<Vec<(String, [f32; 2])>> {
    let items = paint
        .properties
        .get("text-variable-anchor-offset")?
        .as_array()?;
    let pairs = items
        .as_chunks::<2>()
        .0
        .iter()
        .filter_map(|pair| {
            let anchor = pair[0].as_str()?.to_owned();
            let offset = pair[1].as_array()?;
            let read = |index: usize| offset.get(index).and_then(serde_json::Value::as_f64);
            Some((anchor, [read(0)? as f32, read(1)? as f32]))
        })
        .collect::<Vec<_>>();
    (!pairs.is_empty()).then_some(pairs)
}

fn from_radial(anchor: &str, radial: f32) -> [f32; 2] {
    let radial = radial.max(0.0);
    let diagonal = radial / std::f32::consts::SQRT_2;
    let y = match anchor {
        "top-right" | "top-left" => diagonal - BASELINE,
        "bottom-right" | "bottom-left" => -diagonal + BASELINE,
        "bottom" => -radial + BASELINE,
        "top" => radial - BASELINE,
        _ => 0.0,
    };
    let x = match anchor {
        "top-right" | "bottom-right" => -diagonal,
        "top-left" | "bottom-left" => diagonal,
        "left" => radial,
        "right" => -radial,
        _ => 0.0,
    };
    [x, y]
}

fn from_text_offset(anchor: &str, [x, y]: [f32; 2]) -> [f32; 2] {
    let (x, y) = (x.abs(), y.abs());
    let y = match anchor {
        "top-right" | "top-left" | "top" => y - BASELINE,
        "bottom-right" | "bottom-left" | "bottom" => -y + BASELINE,
        _ => 0.0,
    };
    let x = match anchor {
        "top-right" | "bottom-right" | "right" => -x,
        "top-left" | "bottom-left" | "left" => x,
        _ => 0.0,
    };
    [x, y]
}

/// The text offset in layout pixels for an anchor. A label with one anchor applies
/// `text-offset` as written, or `text-radial-offset` along the anchor's direction when one is
/// set; a label with variable anchors turns either into the offset each anchor needs.
pub(super) fn anchored_offset(
    paint: &SymbolPaint,
    variable: &[String],
    anchor: &str,
    (properties, zoom): (&FeatureProperties, f64),
) -> [f32; 2] {
    let context = (properties, zoom);
    let base = if let Some(pairs) = anchor_offsets(paint) {
        let [x, y] = pairs
            .iter()
            .find(|(name, _)| name == anchor)
            .map_or([0.0; 2], |(_, offset)| offset.map(|em| em * 24.0));
        let shift = if anchor.starts_with("top") {
            -BASELINE
        } else if anchor.starts_with("bottom") {
            BASELINE
        } else {
            0.0
        };
        [x, y + shift]
    } else if variable.is_empty() {
        let radial = paint.number("text-radial-offset", properties, zoom, 0.0);
        if radial != 0.0 {
            from_radial(anchor, radial * 24.0)
        } else {
            offset(paint, "text-offset", 24.0, context)
        }
    } else if paint.properties.contains_key("text-radial-offset") {
        from_radial(
            anchor,
            paint.number("text-radial-offset", properties, zoom, 0.0) * 24.0,
        )
    } else {
        from_text_offset(anchor, offset(paint, "text-offset", 24.0, context))
    };
    if variable.is_empty() {
        return base;
    }
    // A label with variable anchors sits at its anchor by the box the collision test uses,
    // which `text-padding` grows on every side, so the text keeps that gap from the point.
    let size = paint
        .text_size
        .as_ref()
        .and_then(|value| value.evaluate_for(properties, zoom))
        .unwrap_or(16.0);
    let gap = paint.number("text-padding", properties, zoom, 2.0) * 24.0 / size.max(1.0);
    let fractions = anchor_fractions(anchor);
    [
        base[0] - (fractions[0] - 0.5) * 2.0 * gap,
        base[1] - (fractions[1] - 0.5) * 2.0 * gap,
    ]
}

/// Resolved anchor, justification, and offsets for a laid-out text block.
pub(super) struct BlockPlacement {
    pub anchor: String,
    pub anchors: Vec<String>,
    pub fractions: [f32; 2],
    pub justify: f32,
    pub offset: [f32; 2],
    pub line_offset_y: f32,
}

/// Resolves a label's primary anchor, justification fraction, and pixel offset.
pub(super) fn resolve_block_placement(
    paint: &SymbolPaint,
    properties: &FeatureProperties,
    zoom: f64,
    vertical: bool,
) -> BlockPlacement {
    let anchors = variable_anchors(paint);
    let anchor = anchors.first().cloned().unwrap_or_else(|| {
        paint
            .text("text-anchor", properties, zoom)
            .unwrap_or_else(|| "center".into())
    });
    let fractions = anchor_fractions(&anchor);
    let justify = paint
        .text("text-justify", properties, zoom)
        .unwrap_or_else(|| "center".into());
    let justify = match justify.as_str() {
        _ if vertical => 0.0,
        "left" => 0.0,
        "right" => 1.0,
        "auto" => fractions[0],
        _ => 0.5,
    };
    let mut offset = anchored_offset(paint, &anchors, &anchor, (properties, zoom));
    let line_offset_y = if super::is_line_placed(paint) {
        std::mem::take(&mut offset[1])
    } else {
        0.0
    };
    BlockPlacement {
        anchor,
        anchors,
        fractions,
        justify,
        offset,
        line_offset_y,
    }
}
