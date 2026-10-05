//! Ground truly standing in front of a label hides it at every pixel ratio: markers behind a
//! ridge stay hidden while markers before it show, across the whole width of the target.

use super::*;

/// The zoom of the DEM tiles the ridge is laid out in.
const RIDGE_ZOOM: u32 = 12;
/// The DEM tile the view looks across, near the equator.
const TILE: [u32; 2] = [2048, 2048];
/// Where the ridge crosses [`TILE`], as fractions of it from its north edge; it runs on
/// through the tiles either side.
const RIDGE: std::ops::Range<f64> = 0.35..0.6;
const RIDGE_METRES: f64 = 4000.0;
/// The zoom at one device pixel per layout pixel, looking north across the ridge.
const VIEW_ZOOM: f64 = 12.5;
const PITCH: f64 = 60.0;
/// The view's center, south of the ridge, as fractions of [`TILE`].
const CENTER: [f64; 2] = [0.5, 0.85];
/// Markers before the ridge and behind it, as fractions of [`TILE`], across the target.
const BEFORE: [[f64; 2]; 3] = [[0.25, 0.75], [0.5, 0.75], [0.75, 0.75]];
const BEHIND: [[f64; 2]; 3] = [[0.25, 0.2], [0.5, 0.2], [0.75, 0.2]];

/// The terrarium DEM tile `z/x/y`, `metres` high where it covers the ridge, which every zoom
/// shows at the same place.
fn dem_tile([z, x, y]: [u32; 3], metres: f64) -> Vec<u8> {
    let scale = f64::from(1_u32 << RIDGE_ZOOM.saturating_sub(z));
    let tile = image::RgbaImage::from_fn(256, 256, |column, row| {
        let east = (f64::from(x) + (f64::from(column) + 0.5) / 256.0) * scale;
        let south = (f64::from(y) + (f64::from(row) + 0.5) / 256.0) * scale - f64::from(TILE[1]);
        let across = (f64::from(TILE[0]) - 1.0..f64::from(TILE[0]) + 2.0).contains(&east);
        let height = if across && RIDGE.contains(&south) {
            metres
        } else {
            0.0
        };
        let value = height + 32768.0;
        let red = (value / 256.0).floor();
        image::Rgba([red as u8, (value - red * 256.0) as u8, 0, 255])
    });
    let mut png = std::io::Cursor::new(Vec::new());
    tile.write_to(&mut png, image::ImageFormat::Png)
        .expect("PNG");
    png.into_inner()
}

/// Serves the fixtures' fonts and sprites, and a DEM `metres` high along the ridge and at sea
/// level elsewhere.
pub(super) fn server_with_ridges(metres: f64) -> AssetServer {
    let server = AssetServer::default();
    for z in 0..=RIDGE_ZOOM {
        let shift = RIDGE_ZOOM - z;
        let y = TILE[1] >> shift;
        for x in (TILE[0] - 1) >> shift..=(TILE[0] + 1) >> shift {
            server.serve(
                &format!("https://dem.test/{z}/{x}/{y}."),
                dem_tile([z, x, y], metres),
            );
        }
    }
    server.serve("https://dem.test/", dem_tile([RIDGE_ZOOM, 0, 0], 0.0));
    server
}

/// Serves the fixtures' fonts and sprites, and a DEM with its ridges.
pub(super) fn server() -> AssetServer {
    server_with_ridges(RIDGE_METRES)
}

/// The longitude and latitude at fractions of [`TILE`].
fn in_tile([x, y]: [f64; 2]) -> LatLon {
    let world = f64::from(1_u32 << RIDGE_ZOOM);
    let longitude = (f64::from(TILE[0]) + x) / world * 360.0 - 180.0;
    let mercator = std::f64::consts::PI * (1.0 - 2.0 * (f64::from(TILE[1]) + y) / world);
    LatLon::new(mercator.sinh().atan().to_degrees(), longitude)
}

fn ridge_style(ratio: f64) -> Style {
    let mut points: Vec<(LatLon, &str)> = BEFORE.map(|at| (in_tile(at), "before")).to_vec();
    points.extend(BEHIND.map(|at| (in_tile(at), "behind")));
    let mut layers = vec![serde_json::json!({"id":"background","type":"background",
        "paint":{"background-color":"#223344"}})];
    layers.extend(marker_layers("before", &COLUMN, &SEEN));
    layers.extend(marker_layers("behind", &COLUMN, &HIDDEN));
    let center = in_tile(CENTER);
    serde_json::from_value(serde_json::json!({
        "version":8,"center":[center.longitude,center.latitude],
        "zoom":VIEW_ZOOM - ratio.log2(),"pitch":PITCH,"glyphs":GLYPHS,"sprite":SPRITE,
        "sources":{"places":{"type":"geojson","data":features(&points)},
            "dem":{"type":"raster-dem","tiles":["https://dem.test/{z}/{x}/{y}.png"],
                "encoding":"terrarium","maxzoom":RIDGE_ZOOM,"tileSize":256}},
        "terrain":{"source":"dem","exaggeration":1},
        "layers":layers
    }))
    .expect("style")
}

/// The pixels of each kind of marker in `colours` anywhere on the target.
fn totals(pixels: &[u8], width: u32, colours: &[[u8; 3]; 3]) -> [usize; 3] {
    counts_near(pixels, width, &[[0.0, 0.0]], colours)[0]
}

#[tokio::test]
async fn markers_behind_a_ridge_stay_hidden_and_those_before_it_show_at_every_pixel_ratio() {
    let mut failures = Vec::new();
    for size in [WIDE, SQUARE] {
        // On flat ground the markers behind the ridge's place are on the target.
        let mut flat =
            SymbolMap::serving_sized(ridge_style(1.0), server_with_ridges(0.0), size).await;
        flat.map
            .image_providers()
            .expect("registry")
            .register("shield", Shields::new(Answer::PerRoute));
        let open = totals(&flat.settle().await, size[0], &HIDDEN.colours);
        assert!(
            open.iter()
                .zip(MINIMUM)
                .all(|(have, least)| *have >= 3 * least),
            "{size:?}: without the ridge the markers behind it show: {open:?}"
        );
        let mut reference = [0; 3];
        for ratio in RATIOS {
            let pixels = map_at(ridge_style(ratio), size, ratio).await.settle().await;
            let seen = totals(&pixels, size[0], &SEEN.colours);
            let hidden = totals(&pixels, size[0], &HIDDEN.colours);
            if ratio == 1.0 {
                reference = seen;
            }
            for (kind, name) in KIND_NAMES.iter().enumerate() {
                let area = ratio * ratio;
                let scaled = seen[kind] as f64 / (reference[kind] as f64 * area);
                if (seen[kind] as f64) < (3 * MINIMUM[kind]) as f64 * area
                    || !(0.7..1.4).contains(&scaled)
                {
                    failures.push(format!(
                        "{size:?} at {ratio}x: the {name}s before the ridge show {} pixels, {} at 1x",
                        seen[kind], reference[kind]
                    ));
                }
                if hidden[kind] > 4 {
                    failures.push(format!(
                        "{size:?} at {ratio}x: the {name}s behind the ridge show {} pixels",
                        hidden[kind]
                    ));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
