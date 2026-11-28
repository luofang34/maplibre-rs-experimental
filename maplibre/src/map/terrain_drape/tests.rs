#![allow(clippy::expect_used)]

#[test]
fn terrain_tiles_draw_in_gl_js_covering_order() {
    use cgmath::Deg;

    use crate::{
        coords::{WorldCoords, WorldTileCoords, Zoom},
        render::view_state::ViewState,
        window::PhysicalSize,
    };
    let world_size = 512.0 * 2_f64.powf(9.64);
    let view = ViewState::new(
        PhysicalSize::new(512, 512).expect("size"),
        WorldCoords::from((0.185_461 * world_size, 0.392_890 * world_size)),
        Zoom::new(9.64),
        Deg(0.0),
        Deg(36.0),
    );
    let key = super::targets::draw_order_key(&crate::style::Style::default(), &view, 256.0);
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
    assert_eq!(order, [9, 8, 7, 4, 10]);
}

#[test]
fn a_level_eyes_drapes_fit_the_texture_budget_its_horizon_would_exceed() {
    use crate::{
        render::memory_budget::MemoryBudget, tcs::world::World,
        terrain::request_system::DrapeRequests,
    };
    let (style, view) = crate::render::projection::sea_eye();
    let mut world = World::default();
    let surfaces = crate::render::projection::terrain_region(
        &style,
        &view,
        &world,
        256,
        crate::render::view_state::ViewStatePadding::Tight,
    )
    .expect("covering")
    .expect("region")
    .iter()
    .count();
    let allowed = MemoryBudget::default().drape_textures_allowed();
    assert!(
        surfaces > allowed,
        "the eye sees {surfaces} surface tiles, too few to test the {allowed}-texture budget"
    );
    super::targets::target_specs(&style, &view, &mut world).expect("targets");
    let drapes = world
        .resources
        .get::<DrapeRequests>()
        .map_or(0, |requests| requests.0.len());
    assert!(
        (1..=allowed).contains(&drapes),
        "{drapes} drapes for {surfaces} surface tiles, {allowed} allowed"
    );
}
