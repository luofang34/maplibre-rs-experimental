//! Synchronizes `feature-state` during tessellation and records per-entry feature attributes.

use std::sync::Arc;

use lyon::tessellation::{geometry_builder::MaxIndex, VertexId};

use super::ZeroTessellator;
use crate::{
    style::feature_state::{sync_properties_feature_state, SourceFeatureState},
    vector::feature_paint::{BucketEntryKind, BucketFeatureEntry, FeaturePaintRetention},
};

/// Source `promoteId` and active runtime `feature-state` used while tessellating one source layer.
#[derive(Clone, Debug, Default)]
pub(crate) struct FeatureStateSyncContext {
    /// Property promoted as the feature ID for this layer, if any.
    pub promote_property: Option<String>,
    /// Shared runtime `feature-state` snapshot for the tile's source.
    pub source_state: Option<Arc<SourceFeatureState>>,
    /// Source layer name (`None` for GeoJSON).
    pub source_layer: Option<String>,
}

impl<I> ZeroTessellator<I>
where
    I: std::ops::Add + From<VertexId> + MaxIndex + Copy + Into<u32>,
{
    #[inline]
    pub(super) fn props(&self) -> &crate::style::expression::FeatureProperties {
        self.current_shared_props
            .as_deref()
            .unwrap_or(&self.feature_properties)
    }

    pub(super) fn begin_feature(&mut self, idx: u64) {
        if self.buffer.vertices.capacity() >= 32_768
            && self.buffer.vertices.capacity() - self.buffer.vertices.len() < 4096
        {
            self.buffer.vertices.reserve_exact(32_768);
        }
        if self.buffer.indices.capacity() >= 65_536
            && self.buffer.indices.capacity() - self.buffer.indices.len() < 8192
        {
            self.buffer.indices.reserve_exact(65_536);
        }
        let index = usize::try_from(idx).ok();
        self.current_raw_id = index
            .and_then(|i| self.source_ids.get(i).copied())
            .flatten();
        self.current_shared_props = index.and_then(|i| self.shared_properties.get(i).cloned());
        self.state_synced = self.current_shared_props.is_some();
        self.current_extrusion_heights = None;
    }

    pub(super) fn ensure_feature_state_synced(&mut self) {
        if self.state_synced {
            return;
        }
        self.state_synced = true;
        if let Some(ctx) = &self.feature_state_ctx {
            sync_properties_feature_state(
                &mut self.feature_properties,
                self.current_raw_id,
                ctx.promote_property.as_deref(),
                ctx.source_state.as_deref(),
                ctx.source_layer.as_deref(),
            );
        }
    }

    pub(super) fn finish_feature(&mut self) {
        self.ensure_feature_state_synced();
        self.current_extrusion_heights = None;
        let polygons = std::mem::take(&mut self.polygon_entries);
        let first_entry = self.feature_indices.len() - polygons;
        if polygons == 0 {
            self.update_feature_indices();
        }
        let line_length = self.line_length;
        let main_count = polygons.max(1);
        if self.record_outline_entries {
            self.outline_entries
                .resize(self.outline_entries.len() + main_count, false);
        }
        let pattern = self.push_feature_color_and_pattern(line_length, main_count);
        let outline_added = self.append_outline();
        self.record_feature_paint_entries(
            first_entry..self.feature_indices.len(),
            pattern,
            line_length,
            main_count,
            outline_added,
        );
        self.current_raw_id = None;
        self.state_synced = false;
    }

    fn push_feature_color_and_pattern(
        &mut self,
        line_length: f32,
        main_count: usize,
    ) -> Option<f32> {
        if self.omit_feature_colors {
            self.line_length = 0.0;
            return None;
        }
        let mut color = if let Some(style) = &self.style_property {
            if let Some(c) = style.evaluate_for(self.props(), self.zoom) {
                [c.r, c.g, c.b, c.a]
            } else {
                tracing::debug!(
                    "Style evaluation failed for feature properties: {:?}, style: {:?}",
                    self.props(),
                    style
                );
                self.fallback_color
            }
        } else {
            self.fallback_color
        };
        if self.line_gradient {
            color = [line_length, 0.0, 0.0, 1.0];
            self.line_length = 0.0;
        }
        if let Some((opacity, zoom)) = &self.feature_opacity {
            color[3] *= opacity
                .evaluate_for(self.props(), *zoom)
                .unwrap_or(1.0)
                .clamp(0.0, 1.0);
        }
        // Features sharing a pattern end up side by side, so one draw can bind its image.
        let pattern = self.pattern_property.as_ref().map(|property| {
            crate::style::pattern_key::pattern_value(
                property
                    .evaluate_for(self.props(), self.zoom)
                    .as_ref()
                    .map(|name| name.0.as_str()),
            )
        });
        if let Some(value) = pattern {
            color[0] = value;
        }
        let color = self.circle_feature_color(color);
        for _ in 0..main_count {
            self.feature_colors.push(color);
        }
        pattern
    }

    fn record_feature_paint_entries(
        &mut self,
        entry_range: std::ops::Range<usize>,
        pattern: Option<f32>,
        line_length: f32,
        main_count: usize,
        outline_added: bool,
    ) {
        let retention = self.feature_paint_retention;
        let main_kind = if self.is_line_layer {
            BucketEntryKind::Line {
                length: line_length,
            }
        } else {
            BucketEntryKind::Feature
        };
        let (raw_id, props) = if retention != FeaturePaintRetention::Full {
            let props_ref = self
                .current_shared_props
                .as_deref()
                .unwrap_or(&self.feature_properties);
            self.sort_key.record(
                props_ref,
                self.zoom,
                entry_range,
                (self.buffer.indices.len(), pattern),
            );
            self.current_shared_props = None;
            self.feature_properties.clear();
            if retention != FeaturePaintRetention::SlotKinds {
                return;
            }
            (None, Arc::clone(&self.slot_kind_properties))
        } else {
            let props = self
                .current_shared_props
                .take()
                .unwrap_or_else(|| Arc::new(std::mem::take(&mut self.feature_properties)));
            self.sort_key.record(
                &props,
                self.zoom,
                entry_range,
                (self.buffer.indices.len(), pattern),
            );
            (self.current_raw_id, props)
        };
        for _ in 0..main_count {
            self.feature_paint.push(BucketFeatureEntry {
                raw_id,
                properties: props.clone(),
                kind: main_kind,
            });
        }
        if outline_added {
            self.feature_paint.push(BucketFeatureEntry {
                raw_id,
                properties: props,
                kind: BucketEntryKind::FillOutline,
            });
        }
    }
}
