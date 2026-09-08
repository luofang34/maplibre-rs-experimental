use super::*;
use crate::{coords::ZoomLevel, terrain::drape_targets::ShapeSpec};
fn tile(x: i32, y: i32, z: u8) -> WorldTileCoords {
    WorldTileCoords {
        x,
        y,
        z: ZoomLevel::new(z),
    }
}

#[test]
fn reducing_texture_budget_preserves_every_mesh_coordinate_and_dem_zoom() {
    let fine = vec![tile(0, 0, 8), tile(1, 0, 8), tile(0, 1, 8), tile(1, 1, 8)];
    let coarse = tile(0, 0, 7);
    let mut world = World::default();
    world.resources.insert(SurfaceTiles(fine.clone()));
    let drapes = vec![TargetSpec {
        coords: coarse,
        shapes: Vec::new(),
    }];
    let (surfaces, sources) = for_frame(&world, &drapes, &[Some(coarse)]);
    assert_eq!(
        surfaces.iter().map(|spec| spec.coords).collect::<Vec<_>>(),
        fine
    );
    assert_eq!(sources, vec![Some(coarse); 4]);
    for spec in surfaces {
        assert_eq!(
            crate::terrain::dem_tile_coords(spec.coords, 0, 14),
            Some(coarse)
        );
    }
}

#[test]
fn metadata_shortage_defers_whole_textures_instead_of_caching_partial_coverage() {
    let source = tile(0, 0, 0);
    let specs: Vec<_> = [2, 3, 1]
        .into_iter()
        .map(|count| TargetSpec {
            coords: source,
            shapes: (0..count)
                .map(|_| ShapeSpec {
                    source,
                    vector_layers: Vec::new(),
                    raster_layers: Vec::new(),
                })
                .collect(),
        })
        .collect();
    let mut ready = vec![true; 3];
    fit_metadata(&specs, &mut ready, 4);
    assert_eq!(ready, [true, false, true]);
}
