//! Hillshade lighting of known slopes: light side, illumination anchor, exaggeration and seams.
use super::*;

/// Metres of elevation a tile gains per pixel, steep enough to shade strongly at zoom 12.
const RISE_PER_PIXEL: f64 = 30.0;

#[derive(Clone, Copy)]
enum Rise {
    Flat,
    /// Ground climbs toward the east, so it faces west.
    East,
    /// Ground climbs toward the west, so it faces east.
    West,
}

/// A Terrarium tile whose elevation follows `rise` across its pixels, starting at column `offset`.
fn dem_tile(rise: Rise, offset: u32) -> RgbaImage {
    dem_tile_with(rise, offset, RISE_PER_PIXEL)
}

fn dem_tile_with(rise: Rise, offset: u32, per_pixel: f64) -> RgbaImage {
    RgbaImage::from_fn(256, 256, |x, _| {
        let column = f64::from(x + offset);
        let meters = match rise {
            Rise::Flat => 1000.0,
            Rise::East => 1000.0 + per_pixel * column,
            Rise::West => 9000.0 - per_pixel * column,
        };
        let encoded = meters + 32768.0;
        let red = (encoded / 256.0).floor();
        let green = (encoded - red * 256.0).floor();
        let blue = ((encoded - red * 256.0 - green) * 256.0).floor();
        Rgba([red as u8, green as u8, blue as u8, 255])
    })
}

/// A style centred `columns` tiles east of the middle of the target tile.
fn hillshade_style(paint: serde_json::Value, bearing: f64, columns: f64) -> Style {
    let coords = target();
    let n = 2_f64.powi(i32::from(u8::from(coords.z)));
    let lon = (f64::from(coords.x) + 0.5 + columns) / n * 360.0 - 180.0;
    let lat = (std::f64::consts::PI * (1.0 - 2.0 * (f64::from(coords.y) + 0.5) / n))
        .sinh()
        .atan()
        .to_degrees();
    serde_json::from_value(serde_json::json!({
        "version":8,"center":[lon,lat],"zoom":12.125,"bearing":bearing,
        "sources":{"dem":{"type":"raster-dem","tiles":["offline://dem"],
            "tileSize":256,"maxzoom":14,"encoding":"terrarium"}},
        "layers":[
            {"id":"background","type":"background","paint":{"background-color":"#808080"}},
            {"id":"shade","type":"hillshade","source":"dem","paint":paint}]
    }))
    .expect("style")
}

async fn shaded(style: Style, tiles: Vec<(WorldTileCoords, RgbaImage)>) -> Vec<u8> {
    let (kernel, renderer) = create_headless_renderer(SIZE, SIZE, None)
        .await
        .expect("renderer");
    let mut map = HeadlessMap::new(
        style,
        renderer,
        kernel,
        vec![
            Box::new(RenderPlugin),
            Box::new(crate::background::BackgroundPlugin),
            Box::new(RasterPlugin::<DefaultRasterTransferables>::default()),
            Box::new(HillshadePlugin),
            Box::new(TerrainPlugin::<DefaultDemTransferables>::default()),
            Box::new(
                HeadlessPlugin::new(false)
                    .preserve_tile_sources()
                    .retain_supplied_tiles(),
            ),
        ],
    )
    .expect("map");
    let raster = tiles
        .into_iter()
        .map(|(coords, image)| AvailableRasterLayerData {
            coords,
            source: "dem".into(),
            image,
        })
        .collect();
    map.render_frames_with_terrain(ProcessedLayers::default(), raster, vec![], 3)
        .expect("hillshade frame");
    read_blocking(&map, "hillshade")
}

fn pixel(bytes: &[u8], x: u32, y: u32) -> [u8; 4] {
    let offset = ((y * SIZE + x) * 4) as usize;
    [
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ]
}

async fn center_of(rise: Rise, paint: serde_json::Value, bearing: f64) -> [u8; 4] {
    let bytes = shaded(
        hillshade_style(paint, bearing, 0.0),
        vec![(target(), dem_tile(rise, 0))],
    )
    .await;
    pixel(&bytes, SIZE / 2, SIZE / 2)
}

#[tokio::test]
async fn slopes_facing_the_light_are_lighter_than_slopes_facing_away() {
    let paint = serde_json::json!({"hillshade-exaggeration": 1.0});
    let flat = center_of(Rise::Flat, paint.clone(), 0.0).await;
    let lit = center_of(Rise::East, paint.clone(), 0.0).await;
    let shadowed = center_of(Rise::West, paint, 0.0).await;
    assert_eq!(flat, [128, 128, 128, 255], "flat ground is not shaded");
    assert!(lit[0] >= flat[0], "the lit side is not darker: {lit:?}");
    assert!(
        shadowed[0] + 20 < flat[0],
        "the side facing away is darker than flat ground: {shadowed:?} vs {flat:?}"
    );
}

#[tokio::test]
async fn zero_exaggeration_draws_nothing() {
    let paint = serde_json::json!({"hillshade-exaggeration": 0.0});
    assert_eq!(
        center_of(Rise::West, paint, 0.0).await,
        [128, 128, 128, 255]
    );
}

#[tokio::test]
async fn the_illumination_anchor_decides_whether_light_turns_with_the_map() {
    let anchored = |anchor: &str| serde_json::json!({"hillshade-exaggeration": 1.0, "hillshade-illumination-anchor": anchor});
    let map_north = center_of(Rise::West, anchored("map"), 0.0).await;
    let map_south = center_of(Rise::West, anchored("map"), 180.0).await;
    assert_eq!(
        map_north, map_south,
        "a light fixed to the map shades a slope the same at any bearing"
    );
    let screen_north = center_of(Rise::West, anchored("viewport"), 0.0).await;
    let screen_south = center_of(Rise::West, anchored("viewport"), 180.0).await;
    assert!(
        screen_south[0] > screen_north[0] + 20,
        "a light fixed to the screen lights the slope once the map is turned: \
         {screen_north:?} then {screen_south:?}"
    );
}

#[tokio::test]
async fn a_continuous_slope_shades_without_a_seam_between_tiles() {
    // A gentle slope, well below the derivative clamp, so a halved edge derivative would show.
    const GENTLE: f64 = 4.0;
    let west = target();
    let east = WorldTileCoords::from((west.x + 1, west.y, west.z));
    let bytes = shaded(
        hillshade_style(serde_json::json!({"hillshade-exaggeration": 1.0}), 0.0, 0.5),
        vec![
            (west, dem_tile_with(Rise::West, 0, GENTLE)),
            (east, dem_tile_with(Rise::West, 256, GENTLE)),
        ],
    )
    .await;
    let row = SIZE / 2;
    let colours: Vec<u8> = (32..SIZE - 32).map(|x| pixel(&bytes, x, row)[0]).collect();
    let (min, max) = colours.iter().fold((255u8, 0u8), |(min, max), value| {
        (min.min(*value), max.max(*value))
    });
    assert!(
        max + 8 < 128,
        "the slope is shaded across both tiles: {colours:?}"
    );
    assert!(
        max - min <= 1,
        "one plane across two tiles shades one colour, not {min}..{max}: {colours:?}"
    );
}

#[tokio::test]
async fn turning_the_light_half_way_around_swaps_the_lit_and_shadowed_sides() {
    let light = |direction: f64, method: &str| {
        serde_json::json!({
            "hillshade-exaggeration": 1.0,
            "hillshade-illumination-direction": direction,
            "hillshade-method": method
        })
    };
    for method in ["standard", "basic"] {
        let from_north_west = center_of(Rise::West, light(335.0, method), 0.0).await;
        let from_south_east = center_of(Rise::West, light(155.0, method), 0.0).await;
        assert!(
            from_south_east[0] > from_north_west[0] + 20,
            "{method}: a slope facing east is lit from the south-east: \
             {from_north_west:?} against {from_south_east:?}"
        );
    }
}
