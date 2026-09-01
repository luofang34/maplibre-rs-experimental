use super::{DrawState, RenderPhase, TileMaskItem};
use crate::render::render_commands::DrawMasks;

fn mask(generate_borders: bool) -> TileMaskItem {
    TileMaskItem {
        draw_function: Box::new(DrawState::<TileMaskItem, DrawMasks>::new()),
        source_shape: Default::default(),
        generate_borders,
    }
}

#[test]
fn bordered_stencil_pass_sorts_before_borderless_pass() {
    let mut phase = RenderPhase::default();
    phase.add(mask(false));
    phase.add(mask(true));

    phase.sort();

    assert!(phase.items[0].generate_borders);
    assert!(!phase.items[1].generate_borders);
}
