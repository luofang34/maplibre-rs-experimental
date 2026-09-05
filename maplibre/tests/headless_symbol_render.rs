//! Renders one labelled point through the headless map and checks that its text reaches the
//! frame, so symbol buckets are not only returned by processing but also drawn.

#![allow(clippy::expect_used, clippy::panic)]

use maplibre::{
    background::BackgroundPlugin,
    coords::WorldTileCoords,
    headless::{
        create_headless_renderer,
        map::{process_tile_layers, HeadlessMap},
        HeadlessPlugin,
    },
    plugin::Plugin,
    projection::ProjectionType,
    render::RenderPlugin,
    sdf::SdfPlugin,
    style::Style,
    vector::{DefaultVectorTransferables, VectorPlugin},
};

/// A `route_points` layer with one point at the tile centre, named `FALLBACK` and labelled
/// `V12`.
const LABELLED_POINT_TILE: &str = "1a460a0c726f7574655f706f696e7473121108011204000001011801220509802080201a046e616d651a056c6162656c220a0a0846414c4c4241434b22050a035631322880207802";
/// Where the headless plugin writes the first frame, relative to the working directory.
const FRAME: &str = "frame_0.png";
const SIZE: u32 = 512;

fn hex_bytes(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&hex[index..index + 2], 16).expect("hex digit pair"))
        .collect()
}

fn read_rgba_png(path: &str) -> (u32, Vec<u8>) {
    let decoder = png::Decoder::new(std::fs::File::open(path).expect("frame written"));
    let mut reader = decoder.read_info().expect("frame is a png");
    let mut pixels = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut pixels).expect("frame decodes");
    assert_eq!(info.color_type, png::ColorType::Rgba);
    assert_eq!((info.width, info.height), (SIZE, SIZE));
    pixels.truncate(info.buffer_size());
    (info.width, pixels)
}

/// Pixels of the central region that are not the white background.
fn inked_centre_pixels(width: u32, pixels: &[u8]) -> usize {
    let centre = SIZE / 2;
    let half = 48;
    let mut inked = 0;
    for y in centre - half..centre + half {
        for x in centre - half..centre + half {
            let offset = ((y * width + x) * 4) as usize;
            let [r, g, b] = [pixels[offset], pixels[offset + 1], pixels[offset + 2]];
            if r < 200 || g < 200 || b < 200 {
                inked += 1;
            }
        }
    }
    inked
}

#[tokio::test]
#[ignore = "renders through the GPU adapter of the machine running the test"]
async fn a_labelled_point_renders_its_text_headless() {
    let style: Style = serde_json::from_value(serde_json::json!({
        "version": 8,
        "center": [0.0, 0.0],
        "zoom": 0,
        "sources": {
            "chart": {"type": "vector", "tiles": ["http://127.0.0.1:1/{z}/{x}/{y}.mvt"]}
        },
        "layers": [
            {"id": "paper", "type": "background", "paint": {"background-color": "#ffffff"}},
            {
                "id": "label", "type": "symbol", "source": "chart",
                "source-layer": "route_points", "layout": {"text-field": "{label}"}
            }
        ]
    }))
    .expect("style parses");
    let label = style
        .layers
        .iter()
        .find(|layer| layer.id == "label")
        .cloned()
        .expect("label layer");

    let (kernel, renderer) = create_headless_renderer(SIZE, SIZE, None)
        .await
        .expect("headless renderer");
    let plugins: Vec<Box<dyn Plugin<_>>> = vec![
        Box::new(RenderPlugin),
        Box::new(BackgroundPlugin),
        Box::new(VectorPlugin::<DefaultVectorTransferables>::default()),
        Box::new(SdfPlugin::<DefaultVectorTransferables>::default()),
        Box::new(HeadlessPlugin::new(true)),
    ];
    let mut map = HeadlessMap::new(style, renderer, kernel, plugins).expect("headless map");

    let layers = process_tile_layers(
        &hex_bytes(LABELLED_POINT_TILE),
        &label,
        WorldTileCoords::default(),
        ProjectionType::Mercator,
    )
    .expect("tile processes");
    assert_eq!(
        layers.symbols.len(),
        1,
        "the label's symbol bucket survives processing"
    );

    std::fs::remove_file(FRAME).ok();
    map.render_tile(layers).expect("frame renders");
    let (width, pixels) = read_rgba_png(FRAME);
    std::fs::remove_file(FRAME).ok();

    let inked = inked_centre_pixels(width, &pixels);
    assert!(
        inked > 0,
        "the label's glyphs leave ink at the tile centre, found {inked} inked pixels"
    );
}
