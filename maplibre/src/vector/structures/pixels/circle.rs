//! A circle stands on the terrain under its centre, as GL JS draws it, so it stays on the
//! ground it marks instead of sinking to sea level beneath a raised terrain mesh.
use super::read_blocking;
use crate::{
    coords::{WorldTileCoords, ZoomLevel},
    headless::{
        create_headless_renderer,
        map::{HeadlessMap, ProcessedLayers},
    },
    render::RenderPlugin,
    style::{layer::StyleLayer, Style},
};

/// The z10 tile north of the equator east of the prime meridian; it spans about 39 km.
fn root() -> WorldTileCoords {
    WorldTileCoords::from((512, 511, ZoomLevel::new(10)))
}

/// The z9 tile that holds [`root`].
fn parent() -> WorldTileCoords {
    WorldTileCoords::from((256, 255, ZoomLevel::new(9)))
}

/// The four z11 tiles inside [`root`].
fn children() -> Vec<WorldTileCoords> {
    [(1024, 1022), (1025, 1022), (1024, 1023), (1025, 1023)]
        .into_iter()
        .map(|(x, y)| WorldTileCoords::from((x, y, ZoomLevel::new(11))))
        .collect()
}

/// The village, inside [`root`], that the camera looks at.
const CENTER: [f64; 2] = [0.18, 0.18];
/// Half the side of the draped square around the village, in degrees: about 670 m.
const HALF_SIDE: f64 = 0.006;

/// Terrarium pixels of 0 m and of 500 m.
const SEA_LEVEL: image::Rgba<u8> = image::Rgba([128, 0, 0, 255]);
const PLATEAU: image::Rgba<u8> = image::Rgba([129, 244, 0, 255]);

fn style(terrain: bool, projection: &str) -> Style {
    let mut style = serde_json::json!({
        "version": 8, "projection": {"type": projection},
        "center": CENTER, "zoom": 11.0, "pitch": 60.0,
        "sources": {
            "places": {"type": "vector", "tiles": ["offline://places"]},
            "dem": {"type": "raster-dem", "tiles": ["offline://dem"], "encoding": "terrarium"}
        },
        "layers": [
            {"id": "background", "type": "background",
             "paint": {"background-color": "#990000"}},
            {"id": "parish", "type": "fill", "source": "places", "source-layer": "places",
             "paint": {"fill-color": "#0000ff"}},
            {"id": "villages", "type": "circle", "source": "places", "source-layer": "places",
             "paint": {"circle-color": "#00ff00", "circle-radius": 5}}
        ]
    });
    if terrain {
        style["terrain"] = serde_json::json!({"source": "dem"});
    }
    let mut style: Style = serde_json::from_value(style).expect("style");
    for (index, layer) in style.layers.iter_mut().enumerate() {
        layer.index = index as u32;
    }
    style
}

/// A pitched map over uniform `ground` at `dem`, with a circle on a fill draped around it.
async fn village_map(
    terrain: bool,
    projection: &str,
    (dem, ground): (WorldTileCoords, image::Rgba<u8>),
) -> HeadlessMap {
    let style = style(terrain, projection);
    let (fill, circle) = (style.layers[1].clone(), style.layers[2].clone());
    let (kernel, renderer) = create_headless_renderer(64, 64, None)
        .await
        .expect("renderer");
    let mut map = HeadlessMap::new(
        style,
        renderer,
        kernel,
        vec![
            Box::new(RenderPlugin),
            Box::new(crate::background::BackgroundPlugin),
            Box::new(crate::vector::VectorPlugin::<
                crate::vector::DefaultVectorTransferables,
            >::default()),
            Box::new(crate::terrain::TerrainPlugin::<
                crate::terrain::DefaultDemTransferables,
            >::default()),
            Box::new(
                crate::headless::HeadlessPlugin::new(false)
                    .preserve_tile_sources()
                    .retain_supplied_tiles(),
            ),
        ],
    )
    .expect("map");
    if terrain {
        map.load_dem_tiles(vec![(dem, image::RgbaImage::from_pixel(2, 2, ground))])
            .expect("DEM");
    }
    let [x, y] = CENTER;
    let square = serde_json::json!({"type": "Feature", "properties": {},
        "geometry": {"type": "Polygon", "coordinates": [[
            [x - HALF_SIDE, y - HALF_SIDE], [x + HALF_SIDE, y - HALF_SIDE],
            [x + HALF_SIDE, y + HALF_SIDE], [x - HALF_SIDE, y + HALF_SIDE],
            [x - HALF_SIDE, y - HALF_SIDE]]]}});
    let point = serde_json::json!({"type": "Feature", "properties": {},
        "geometry": {"type": "Point", "coordinates": CENTER}});
    let mut processed = ProcessedLayers::default();
    for coords in map.required_tile_coords().expect("view tiles") {
        for (feature, layer) in [(&square, &fill), (&point, &circle)] {
            processed.append(&mut process(&mut map, feature, layer, coords));
        }
    }
    assert!(!processed.vector.is_empty(), "processed geometry");
    map.render_frames_with_terrain(processed, Vec::new(), Vec::new(), 16)
        .expect("frames");
    map
}

fn process(
    map: &mut HeadlessMap,
    feature: &serde_json::Value,
    layer: &StyleLayer,
    coords: WorldTileCoords,
) -> ProcessedLayers {
    map.process_geojson(
        feature,
        "places",
        vec![layer.clone()],
        coords,
        crate::projection::ProjectionType::Mercator,
    )
    .expect("places")
}

/// Where the circle and the draped square lie on screen.
struct Frame {
    pixels: Vec<u8>,
}

impl Frame {
    fn read(map: &HeadlessMap) -> Self {
        Self {
            pixels: read_blocking(map),
        }
    }

    fn rows(&self, matches: impl Fn(&[u8]) -> bool) -> Vec<usize> {
        (0..64)
            .flat_map(|y| (0..64).map(move |x| (x, y)))
            .filter(|(x, y)| matches(&self.pixels[(y * 64 + x) * 4..][..4]))
            .map(|(_, y)| y)
            .collect()
    }

    fn circle_rows(&self) -> Vec<usize> {
        self.rows(|p| p[1] > 150 && p[0] < 100 && p[2] < 100)
    }

    /// The rows of the square, including those of the circle drawn on it.
    fn square_rows(&self) -> Vec<usize> {
        self.rows(|p| (p[1] > 150 || p[2] > 150) && p[0] < 100)
    }

    fn picture(&self) -> String {
        self.pixels
            .as_chunks::<4>()
            .0
            .chunks(64)
            .map(|row| {
                row.iter()
                    .map(|p| match (p[0] > 120, p[1] > 150, p[2] > 150) {
                        (_, true, _) => 'G',
                        (false, _, true) => 'B',
                        (true, false, false) => '.',
                        _ => '#',
                    })
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Asserts the circle is drawn and centred on the square it marks, vertically, where the
    /// pitched view separates sea level from the raised ground.
    fn assert_circle_on_square(&self, case: &str) {
        let circle = self.circle_rows();
        let square = self.square_rows();
        let mean = |rows: &[usize]| rows.iter().sum::<usize>() as f64 / rows.len().max(1) as f64;
        let picture = self.picture();
        assert!(circle.len() > 20, "{case}: no circle\n{picture}");
        let (top, bottom) = (square.iter().min(), square.iter().max());
        let (Some(&top), Some(&bottom)) = (top, bottom) else {
            panic!("{case}: no square\n{picture}");
        };
        let middle = (top + bottom) as f64 / 2.0;
        assert!(
            (mean(&circle) - middle).abs() < 2.0,
            "{case}: circle centred on row {:.1}, the square it marks spans rows {top}..={bottom}\n{picture}",
            mean(&circle),
        );
    }
}

#[test]
fn circle_stands_on_raised_terrain() {
    futures::executor::block_on(async {
        for projection in ["mercator", "globe"] {
            Frame::read(&village_map(false, projection, (root(), SEA_LEVEL)).await)
                .assert_circle_on_square(&format!("{projection} without terrain"));
            Frame::read(&village_map(true, projection, (root(), PLATEAU)).await)
                .assert_circle_on_square(&format!("{projection} on a 500 m plateau"));
        }
    })
}

#[test]
fn circle_stands_on_the_terrain_drawn_when_finer_dem_arrives() {
    futures::executor::block_on(async {
        let mut map = village_map(true, "mercator", (parent(), SEA_LEVEL)).await;
        Frame::read(&map).assert_circle_on_square("coarse sea-level terrain");
        // The view's terrain tiles draw the root's DEM once it arrives, so the circle must take its
        // 500 m rather than the sea level of the still finer tiles that arrive with it.
        let mut dem = vec![(root(), image::RgbaImage::from_pixel(2, 2, PLATEAU))];
        dem.extend(
            children()
                .into_iter()
                .map(|coords| (coords, image::RgbaImage::from_pixel(2, 2, SEA_LEVEL))),
        );
        map.render_frames_with_terrain(ProcessedLayers::default(), Vec::new(), dem, 16)
            .expect("frames over the plateau");
        let index = map
            .world()
            .resources
            .get::<crate::terrain::TerrainCoverageIndex>()
            .expect("terrain coverage");
        assert!(
            index
                .rendered_tiles()
                .all(|coords| index.loaded_dem_for(coords) == Some(root())),
            "the drawn terrain samples the root DEM"
        );
        Frame::read(&map).assert_circle_on_square("500 m root under finer sea-level tiles");
    })
}
