//! Stroke tessellation, `line-round-limit` shallow-miter conversion, and GeoJSON `lineMetrics`
//! fractional progress scaling.

use geozero::error::GeozeroError;
use lyon::{
    path::{Event, Path},
    tessellation::{geometry_builder::MaxIndex, BuffersBuilder, StrokeOptions, VertexId},
};

use super::{
    concave_join, line_elevation, line_origin, line_style::PackedLine, sharp_corners, GeoResult,
    ZeroTessellator, DEFAULT_TOLERANCE,
};
use crate::style::{
    expression::{FeatureProperties, Value},
    line_stroke::{LineCap, LineJoin},
};

/// Internal property key storing the fractional start `[0.0, 1.0]` of a clipped `lineMetrics` segment.
pub(crate) const LINE_CLIP_START_PROPERTY: &str = "\u{1}clip_start";
/// Internal property key storing the fractional end `[0.0, 1.0]` of a clipped `lineMetrics` segment.
pub(crate) const LINE_CLIP_END_PROPERTY: &str = "\u{1}clip_end";

/// Total length of `path`'s segments in tile units.
pub(super) fn path_length(path: &Path) -> f32 {
    path.iter()
        .map(|event| match event {
            Event::Line { from, to } => (to - from).length(),
            Event::End {
                last,
                first,
                close: true,
            } => (first - last).length(),
            _ => 0.0,
        })
        .sum()
}

fn corner_miter_ratio(u_in: [f32; 2], u_out: [f32; 2]) -> f32 {
    let dot = (u_in[0] * u_out[0] + u_in[1] * u_out[1]).clamp(-1.0, 1.0);
    // `m = 1 / sin((pi - delta) / 2) = 1 / cos(delta / 2) = sqrt(2 / (1 + dot))`.
    let denom = 1.0 + dot;
    if denom <= 1e-6 {
        f32::INFINITY
    } else {
        (2.0 / denom).sqrt()
    }
}

/// Returns `true` when `path` has at least one interior join and every join's miter ratio
/// (`1 / sin(half_angle)`) is at or below `round_limit`, allowing `LineJoin::Round` to collapse to
/// shallow miter joins with `StrokeOptions::with_miter_limit(round_limit)`.
pub(crate) fn joins_within_round_limit(path: &Path, round_limit: f32) -> bool {
    if round_limit < 1.0 {
        return false;
    }
    let mut prev_dir: Option<[f32; 2]> = None;
    let mut first_dir: Option<[f32; 2]> = None;
    let mut saw_join = false;
    for event in path.iter() {
        match event {
            Event::Begin { .. } => {
                prev_dir = None;
                first_dir = None;
            }
            Event::Line { from, to } => {
                let d = to - from;
                let len = d.length();
                if len <= 1e-5 {
                    continue;
                }
                let dir = [d.x / len, d.y / len];
                if let Some(prev) = prev_dir {
                    saw_join = true;
                    if corner_miter_ratio(prev, dir) > round_limit {
                        return false;
                    }
                } else {
                    first_dir = Some(dir);
                }
                prev_dir = Some(dir);
            }
            Event::End {
                last,
                first,
                close: true,
            } => {
                let d = first - last;
                let len = d.length();
                if len > 1e-5 {
                    let close_dir = [d.x / len, d.y / len];
                    if let Some(prev) = prev_dir {
                        saw_join = true;
                        if corner_miter_ratio(prev, close_dir) > round_limit {
                            return false;
                        }
                    }
                    if let Some(first_d) = first_dir {
                        saw_join = true;
                        if corner_miter_ratio(close_dir, first_d) > round_limit {
                            return false;
                        }
                    }
                } else if let (Some(prev), Some(first_d)) = (prev_dir, first_dir) {
                    saw_join = true;
                    if corner_miter_ratio(prev, first_d) > round_limit {
                        return false;
                    }
                }
                prev_dir = None;
                first_dir = None;
            }
            _ => {}
        }
    }
    saw_join
}

fn clip_metrics(props: &FeatureProperties) -> Option<(f64, f64)> {
    let start = match props.get(LINE_CLIP_START_PROPERTY)? {
        Value::Number(n) => *n,
        _ => return None,
    };
    let end = match props.get(LINE_CLIP_END_PROPERTY)? {
        Value::Number(n) => *n,
        _ => return None,
    };
    (start.is_finite() && end.is_finite() && start >= 0.0 && end > start && end <= 1.0)
        .then_some((start, end))
}

impl<I> ZeroTessellator<I>
where
    I: std::ops::Add + From<VertexId> + MaxIndex + Copy + Into<u32>,
{
    pub(super) fn tessellate_strokes(&mut self) -> GeoResult<()> {
        let mut path = self.path_builder.replace(Path::builder()).build();
        if self.line_elevation.is_some() {
            path = line_elevation::clip_elevated_path(&path);
        }
        if self.split_corners {
            path = sharp_corners::split(&path, self.sharp_corner_offset);
        }
        let seg_len = path_length(&path);
        let mut gradient_offset = 0.0_f32;
        if self.line_gradient {
            if let Some((clip_start, clip_end)) = clip_metrics(self.props()) {
                if seg_len > 0.0 {
                    let full_len = (f64::from(seg_len) / (clip_end - clip_start)) as f32;
                    self.line_length = self.line_length.max(full_len);
                    gradient_offset = (clip_start * f64::from(full_len)) as f32;
                }
            } else {
                self.line_length = self.line_length.max(seg_len);
            }
        } else {
            self.line_length = self.line_length.max(seg_len);
        }

        let constructor = line_origin::StrokeOrigins {
            origins: if self.line_gradient {
                Vec::new()
            } else {
                line_origin::entry_distances(&path)
            },
            gradient_offset,
            packed: self
                .line_feature_style
                .as_ref()
                .map_or(PackedLine::LAYER, |style| style.pack(self.props())),
        };

        let declared_join = self.feature_join();
        let (effective_join, effective_miter_limit) = if declared_join == LineJoin::Round
            && joins_within_round_limit(&path, self.stroke.round_limit)
        {
            (LineJoin::Miter, self.stroke.round_limit.max(1.0))
        } else {
            (declared_join, self.stroke.miter_limit)
        };

        let vertex_start = self.buffer.vertices.len();
        let stroke_index_start = self.buffer.indices.len();
        self.stroke_tessellator
            .tessellate_path(
                &path,
                &StrokeOptions::tolerance(DEFAULT_TOLERANCE)
                    .with_line_cap(match self.stroke.cap {
                        LineCap::Butt => lyon::tessellation::LineCap::Butt,
                        LineCap::Round => lyon::tessellation::LineCap::Round,
                        LineCap::Square => lyon::tessellation::LineCap::Square,
                    })
                    .with_line_join(match effective_join {
                        LineJoin::Miter => lyon::tessellation::LineJoin::Miter,
                        LineJoin::Bevel => lyon::tessellation::LineJoin::Bevel,
                        LineJoin::Round => lyon::tessellation::LineJoin::Round,
                    })
                    .with_miter_limit(effective_miter_limit),
                &mut BuffersBuilder::new(&mut self.buffer, constructor),
            )
            .map_err(|error| GeozeroError::Geometry(error.to_string()))?;
        let max_normal = match effective_join {
            LineJoin::Miter => (effective_miter_limit * 2.0).max(2.0),
            LineJoin::Bevel | LineJoin::Round => 2.0,
        };
        concave_join::split_unclipped_concave_joins(
            &mut self.buffer,
            vertex_start,
            stroke_index_start,
            max_normal,
        );
        if let Some(options) = &self.line_elevation {
            let (metres, datum) = options.evaluate(self.props());
            line_elevation::apply_elevation(
                &mut self.buffer.vertices[vertex_start..],
                metres,
                datum,
            );
        }
        Ok(())
    }

    fn feature_join(&self) -> LineJoin {
        self.join_property
            .as_ref()
            .and_then(|property| property.evaluate_for(self.props(), self.zoom))
            .and_then(|name| crate::style::line_stroke::LineStroke::join_named(&name))
            .unwrap_or(self.stroke.join)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]
    use geozero::GeomProcessor;

    use crate::{
        style::layer::StyleLayer,
        vector::tessellation::{IndexDataType, ZeroTessellator},
    };

    fn stroke_vertex_count(round_limit: f32) -> usize {
        let layer: StyleLayer = serde_json::from_value(serde_json::json!({
            "id": "roads",
            "type": "line",
            "source": "s",
            "layout": {
                "line-join": "round",
                "line-round-limit": round_limit
            }
        }))
        .expect("layer");
        let mut tess = ZeroTessellator::<IndexDataType> {
            is_line_layer: true,
            ..Default::default()
        };
        tess.set_layer_strokes(&layer);
        // 90-degree corner has miter ratio sqrt(2) ~= 1.4142.
        tess.linestring_begin(true, 3, 0).expect("begin");
        tess.xy(100.0, 100.0, 0).expect("p0");
        tess.xy(300.0, 100.0, 1).expect("p1");
        tess.xy(300.0, 300.0, 2).expect("p2");
        tess.linestring_end(true, 0).expect("end");
        tess.buffer.vertices.len()
    }

    #[test]
    fn line_round_limit_converts_round_join_to_miter_when_threshold_exceeds_corner_ratio() {
        let round_verts = stroke_vertex_count(1.05);
        let miter_verts = stroke_vertex_count(1.6);
        assert!(
            miter_verts < round_verts,
            "round_limit=1.6 should convert 90-deg round join (ratio ~1.414) to miter ({miter_verts} < {round_verts})"
        );
    }
}
