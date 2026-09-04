use std::collections::BTreeSet;

use maplibre::{coords::WorldTileCoords, io::tile_sources::source_tiles_for};

/// Source tiles the harness loads for the view tiles a map requires, following the same zoom
/// rules as the request systems: `zoom_delta` levels away from each view tile, within the
/// source zoom range, and nothing below the source minimum zoom.
pub(crate) fn source_tile_coords(
    required: &[WorldTileCoords],
    zoom_delta: i32,
    min_zoom: Option<u8>,
    max_zoom: Option<u8>,
) -> BTreeSet<WorldTileCoords> {
    required
        .iter()
        .flat_map(|coords| source_tiles_for(*coords, zoom_delta, min_zoom, max_zoom))
        .collect()
}

#[cfg(test)]
mod tests {
    use maplibre::coords::{WorldTileCoords, ZoomLevel};

    use super::source_tile_coords;

    #[test]
    fn skips_tiles_below_the_source_minimum_zoom() {
        let root = WorldTileCoords::default();
        let selected = source_tile_coords(&[root], 0, Some(1), Some(1));

        assert!(selected.is_empty());
    }

    #[test]
    fn raster_tiles_one_level_down_cover_each_view_tile() {
        let root = WorldTileCoords::default();
        let selected = source_tile_coords(&[root], 1, Some(1), Some(1));

        assert_eq!(selected, root.get_children().into_iter().collect());
    }

    #[test]
    fn clamps_visible_children_to_source_maximum_zoom() {
        let child = WorldTileCoords {
            x: 3,
            y: 2,
            z: ZoomLevel::new(2),
        };
        let selected = source_tile_coords(&[child], 0, None, Some(1));

        assert_eq!(
            selected,
            [child.get_parent().expect("z2 has a parent")].into()
        );
    }
}
