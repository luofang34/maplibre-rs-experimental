//! Event-bus synchronization and missing-image tracking for [`MapContext`].

use rustc_hash::FxHashSet as HashSet;

use crate::{
    context::MapContext,
    event::{MapErrorKind, MapEvent},
    io::tile_retry::RequestKind,
    render::frame_signals::ResourceReady,
    sdf::assets::ImageProviders,
    style::{
        layer::{ImageName, LayerPaint, PatternName, StyleProperty},
        mutation::{StyleChange, StyleMutationError},
        source::Source,
        Style, StyleImage,
    },
};

impl MapContext {
    /// Adds or replaces a runtime style image (`id`) and refreshes symbol and pattern layers so
    /// loaded tiles can pack it into their atlases.
    pub fn add_style_image(
        &mut self,
        id: &str,
        image: StyleImage,
    ) -> Result<StyleChange, StyleMutationError> {
        let bus = self.event_bus();
        match self.mutate_style(|style| {
            let mut change = style.add_image(id, image);
            for layer in &style.layers {
                if matches!(
                    layer.type_.as_str(),
                    "fill" | "fill-extrusion" | "line" | "background"
                ) && !change.layers.contains(&layer.id)
                {
                    change.layers.push(layer.id.clone());
                    if layer.type_ != "background" {
                        change.redraw_tiles = true;
                    }
                }
            }
            Ok(change)
        }) {
            Ok(change) => {
                bus.mark_busy();
                bus.emit(MapEvent::StyleDataLoaded);
                Ok(change)
            }
            Err(error) => {
                bus.emit_error(MapErrorKind::Style, error.to_string());
                Err(error)
            }
        }
    }

    /// Removes a runtime style image (`id`) and clears its missing-image deduplication entry so
    /// subsequent requests for `id` emit [`MapEvent::StyleImageMissing`] again.
    pub fn remove_style_image(&mut self, id: &str) -> Result<StyleChange, StyleMutationError> {
        let bus = self.event_bus();
        bus.clear_missing_image(id);
        match self.mutate_style(|style| Ok(style.remove_image(id))) {
            Ok(change) => {
                bus.mark_busy();
                bus.emit(MapEvent::StyleDataLoaded);
                Ok(change)
            }
            Err(error) => {
                bus.emit_error(MapErrorKind::Style, error.to_string());
                Err(error)
            }
        }
    }

    /// Marks [`MapEventBus`](crate::event::MapEventBus) busy if this context currently requires a
    /// redraw or `extra_busy` holds.
    pub(crate) fn mark_busy_if_needed(&mut self, extra_busy: bool) {
        if self.needs_redraw() || extra_busy {
            self.event_bus().mark_busy();
        }
    }

    /// Synchronizes frame-end lifecycle, ready-source, and missing-image events into
    /// [`MapEventBus`](crate::event::MapEventBus).
    pub(crate) fn sync_frame_events(
        &mut self,
        image_providers: Option<&ImageProviders>,
        extra_busy: bool,
    ) {
        let bus = self.event_bus();
        if !bus.mark_initial_style_emitted() {
            bus.emit(MapEvent::StyleDataLoaded);
        }

        let ready = self.take_ready_resources();
        if !ready.is_empty() {
            let mut loaded_sources = HashSet::default();
            for item in &ready {
                if let ResourceReady::Tile {
                    kind, loaded: true, ..
                } = item
                {
                    for (name, source) in &self.style.sources {
                        let matches_kind = match kind {
                            RequestKind::Vector => {
                                matches!(source, Source::Vector(_) | Source::GeoJson(_))
                            }
                            RequestKind::Raster => matches!(
                                source,
                                Source::Raster(_)
                                    | Source::Image(_)
                                    | Source::Video(_)
                                    | Source::Canvas(_)
                            ),
                            RequestKind::Dem => matches!(source, Source::RasterDem(_)),
                        };
                        if matches_kind {
                            loaded_sources.insert(name.clone());
                        }
                    }
                }
            }
            bus.push_ready_resources(ready);
            for source in loaded_sources {
                bus.emit(MapEvent::SourceDataLoaded { source });
            }
        }

        let mut missing_ids = HashSet::default();
        if let Some(providers) = image_providers {
            for id in providers.take_missing() {
                missing_ids.insert(id);
            }
        }
        collect_visible_missing_style_images(
            &self.style,
            self.view_state.zoom().value(),
            image_providers,
            &mut missing_ids,
        );
        for id in missing_ids {
            if !self.style.images.contains_key(&id) {
                bus.emit_missing_image(id);
            }
        }

        let settled = !self.needs_redraw() && !extra_busy;
        bus.observe_frame_settled(settled);
    }
}

fn collect_visible_missing_style_images(
    style: &Style,
    zoom: f64,
    providers: Option<&ImageProviders>,
    out: &mut HashSet<String>,
) {
    let is_supplied = |name: &str| -> bool {
        style.images.contains_key(name) || providers.is_some_and(|p| p.provides(name))
    };
    for layer in &style.layers {
        if !layer.is_visible_at(zoom) {
            continue;
        }
        let Some(paint) = &layer.paint else {
            continue;
        };
        let pattern_val = match paint {
            LayerPaint::Fill(p) => p.fill_pattern.as_ref(),
            LayerPaint::FillExtrusion(p) => p.fill_extrusion_pattern.as_ref(),
            LayerPaint::Background(p) => p.background_pattern.as_ref(),
            LayerPaint::Line(p) => p.line_pattern.as_ref(),
            _ => None,
        };
        if let Some(val) = pattern_val {
            let prop = StyleProperty::<PatternName>::parse(val);
            if prop.is_feature_constant()
                && let Some(PatternName(name)) = prop.evaluate_at_zoom(zoom)
                && !name.is_empty()
                && style.sprite.is_none()
                && !is_supplied(&name)
            {
                out.insert(name);
            }
        }
        if style.sprite.is_none()
            && let LayerPaint::Symbol(sym) = paint
            && let Some(val) = sym.properties.get("icon-image")
        {
            let prop = StyleProperty::<ImageName>::parse(val);
            if prop.is_feature_constant()
                && let Some(ImageName(name)) = prop.evaluate_at_zoom(zoom)
                && !name.is_empty()
                && !is_supplied(&name)
            {
                out.insert(name);
            }
        }
    }
}
