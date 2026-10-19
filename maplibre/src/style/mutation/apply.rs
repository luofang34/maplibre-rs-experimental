//! Bringing a running map's tiles up to date with a style change.

use super::{StyleChange, StyleMutationError};
use crate::{
    context::MapContext,
    geojson::update::GeoJsonVersions,
    io::tile_retry::{self, RequestKind},
    style::Style,
};

impl MapContext {
    /// Applies a style change and fetches loaded vector tiles again when it needs that; the old
    /// content stays on screen until the new tiles arrive. GeoJSON data the change replaced, as
    /// with [`Style::set_geojson_data`], refreshes the tiles it changed in by itself, so a caller
    /// that only replaces data returns [`StyleChange::default()`]: `redraw_tiles` would fetch
    /// every loaded tile of every source again.
    pub fn mutate_style(
        &mut self,
        apply: impl FnOnce(&mut Style) -> Result<StyleChange, StyleMutationError>,
    ) -> Result<StyleChange, StyleMutationError> {
        let geojson = GeoJsonVersions::of(&self.style);
        let change = apply(&mut self.style)?;
        crate::render::frame_signals::mark_dirty(&mut self.world);
        if !change.removed_layers.is_empty() {
            crate::vector::content::purge_layers(&mut self.world, &change.removed_layers);
        }
        if change.redraw_tiles {
            tile_retry::refresh(&mut self.world, RequestKind::Vector);
        } else {
            geojson.refresh_changed(&self.style, &mut self.world);
        }
        for name in &change.removed_raster_sources {
            crate::raster::forget_source(&mut self.world, name);
        }
        for name in &change.reloaded_image_sources {
            crate::raster::image_source::reload(&mut self.world, &self.style, name);
        }
        Ok(change)
    }
}
