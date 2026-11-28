//! The world area a tile cuts its features from, and the area a feature lies in.
//!
//! Both are in world coordinates, where the world is `0..1`. A feature is in a tile exactly when
//! their areas intersect, so a change to a feature reaches only the tiles whose window meets it.

use serde_json::Value;

use super::{bounds_of, read_geometry, Geometry};
pub(crate) use crate::tcs::tile_window::TileWindow;

/// The world bounds of a feature's geometry; `None` for a feature the index leaves out because
/// it has no geometry or no positions.
pub(crate) fn feature_bounds(feature: &Value) -> Result<Option<[f64; 4]>, &'static str> {
    let Some(geometry) = feature.get("geometry").filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let mut parsed = Geometry::default();
    read_geometry(geometry, &mut parsed)?;
    Ok(bounds_of(&parsed))
}
