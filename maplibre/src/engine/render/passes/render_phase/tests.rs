use super::{
    DrawState, LayerItem, MaskGroup, RenderPhase, TileMaskItem, TranslucentItem, UNSET_ENTRY_SLOT,
};
use crate::{
    background::render_commands::{DrawBackgroundQuad, SetBackgroundPipeline},
    render::{
        render_commands::DrawMasks, render_phase::ProjectionBinding,
        tile_view_pattern::ShapeDrawData,
    },
    style::StyleLayerId,
    tcs::tiles::Tile,
};

fn mask(generate_borders: bool) -> TileMaskItem {
    TileMaskItem {
        projection: ProjectionBinding::View,
        draw_function: &DrawState::<TileMaskItem, DrawMasks>::INSTANCE,
        source_shape: ShapeDrawData::default(),
        generate_borders,
        is_vector: false,
    }
}

fn raster(index: u32, generate_borders: bool) -> LayerItem {
    LayerItem {
        projection: ProjectionBinding::View,
        draw_function:
            &DrawState::<LayerItem, (SetBackgroundPipeline, DrawBackgroundQuad)>::INSTANCE,
        index,
        generate_borders,
        style_layer: StyleLayerId::intern("raster"),
        entry_slot: UNSET_ENTRY_SLOT,
        tile: Tile {
            coords: Default::default(),
        },
        source_shape: ShapeDrawData::default(),
        mask_group: MaskGroup::Mutating,
        draw_kind: super::LayerDrawKind::Other,
        run: None,
    }
}

#[test]
fn phase_items_are_copy_and_cache_line_compact() {
    fn assert_copy<T: Copy>() {}
    assert_copy::<LayerItem>();
    assert_copy::<TranslucentItem>();
    assert_copy::<TileMaskItem>();
    assert!(std::mem::size_of::<LayerItem>() <= 96);
    assert!(std::mem::size_of::<TranslucentItem>() <= 80);
    assert!(std::mem::size_of::<TileMaskItem>() <= 64);
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

#[test]
fn raster_interiors_sort_before_borders_without_crossing_style_layers() {
    let mut phase = RenderPhase::default();
    phase.add(raster(2, false));
    phase.add(raster(1, false));
    phase.add(raster(2, true));
    phase.add(raster(1, true));

    phase.sort();

    let keys = phase
        .items
        .iter()
        .map(|item| (item.index, item.generate_borders))
        .collect::<Vec<_>>();
    assert_eq!(keys, vec![(1, false), (1, true), (2, false), (2, true)]);
}
