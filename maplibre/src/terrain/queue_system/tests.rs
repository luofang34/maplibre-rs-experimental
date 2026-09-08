use super::{awaiting_first_draw, budget_redraws, DrapeState};

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
