use super::*;

#[tokio::test]
async fn mercator_unequal_lod_edge_joins_different_dem_heights() {
    check_case(Case {
        projection: "mercator",
        fine: WorldTileCoords::from((136, 89, 8_u8.into())),
        coarse: WorldTileCoords::from((67, 44, 7_u8.into())),
        left_edge: true,
        altitude: 4000.0,
        pitch: 60.0,
        bearing: 80.0,
        fov: 80.0,
    })
    .await;
}

#[tokio::test]
async fn globe_unequal_lod_edge_joins_different_dem_heights() {
    check_case(Case {
        projection: "globe",
        fine: WorldTileCoords::from((272, 179, 9_u8.into())),
        coarse: WorldTileCoords::from((135, 89, 8_u8.into())),
        left_edge: true,
        altitude: 8000.0,
        pitch: 40.0,
        bearing: 0.0,
        fov: 60.0,
    })
    .await;
}
