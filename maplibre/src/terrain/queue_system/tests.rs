use super::{awaiting_first_draw, budget_redraws, DrapeState};

#[cfg(feature = "headless")]
#[path = "tests/dem_uniforms.rs"]
mod dem_uniforms;

#[test]
fn a_frame_draws_new_tiles_first_and_at_most_the_budget() {
    use DrapeState::{Changed, New, Unchanged};
    let states = [Changed, New, Unchanged, New, Changed, New];
    assert_eq!(
        budget_redraws(&states, 4),
        [true, true, false, true, false, true],
        "three new tiles and the first changed one"
    );
    assert_eq!(
        budget_redraws(&states, 10),
        [true, true, false, true, true, true],
        "everything but the unchanged tile fits"
    );
    assert_eq!(budget_redraws(&states, 0), [false; 6]);
}

#[test]
fn a_new_tile_the_budget_deferred_is_not_drawn_with_borrowed_content() {
    use DrapeState::{Changed, New, Unchanged};
    let states = [New, Changed, New, Unchanged, New];
    let redraw = budget_redraws(&states, 2);
    assert_eq!(redraw, [true, false, true, false, false]);
    assert_eq!(
        awaiting_first_draw(&states, &redraw),
        [false, false, false, false, true],
        "only the undrawn new tile waits; a changed tile keeps showing its last content"
    );
}

#[test]
fn terrain_tiles_draw_in_gl_js_covering_order() {
    use cgmath::Deg;

    use crate::{
        coords::{WorldCoords, WorldTileCoords, Zoom},
        render::view_state::ViewState,
        window::PhysicalSize,
    };
    // The terrain/skirts-auto view: a 256-pixel DEM covers the map with 512-pixel tiles, so
    // the nominal level is the floor of zoom 9.64.
    let world_size = 512.0 * 2_f64.powf(9.64);
    let view = ViewState::new(
        PhysicalSize::new(512, 512).expect("size"),
        WorldCoords::from((0.185_461 * world_size, 0.392_890 * world_size)),
        Zoom::new(9.64),
        Deg(0.0),
        Deg(36.0),
    );
    let key = super::targets::draw_order_key(&view, 256.0);
    let mut tiles: Vec<WorldTileCoords> = [
        (10, 188, 402),
        (4, 2, 5),
        (8, 48, 100),
        (9, 95, 201),
        (7, 24, 49),
    ]
    .into_iter()
    .map(|(z, x, y)| WorldTileCoords::from((x, y, crate::coords::ZoomLevel::new(z))))
    .collect();
    tiles.sort_by(|left, right| key(left).total_cmp(&key(right)));
    let order: Vec<u8> = tiles.iter().map(|tile| u8::from(tile.z)).collect();
    // GL JS measures from the centre at the nominal level to each tile's own column and row,
    // so tiles above that level come after every tile below it.
    assert_eq!(order, [9, 8, 7, 4, 10]);
}
