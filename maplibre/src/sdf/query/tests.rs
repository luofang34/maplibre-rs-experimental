use super::*;
#[test]
fn a_map_without_placement_returns_no_hits() {
    let world = World::default();
    let style = Style::default();
    assert!(query_rendered_symbols(&world, &style, [0.0, 0.0], None).is_empty());
    assert!(query_rendered_symbols(&world, &style, [f64::NAN, 0.0], None).is_empty());
}
