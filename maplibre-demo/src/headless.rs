use maplibre::{
    coords::{LatLon, WorldTileCoords},
    headless::{
        create_headless_renderer,
        map::{HeadlessMap, ProcessedLayers},
        HeadlessPlugin,
    },
    plugin::Plugin,
    raster::{DefaultRasterTransferables, RasterPlugin},
    render::RenderPlugin,
    style::Style,
    util::grid::google_mercator,
    vector::{DefaultVectorTransferables, VectorPlugin},
};
use tile_grid::{extent_wgs84_to_merc, Extent, GridIterator};

pub async fn run_headless(
    tile_size: u32,
    min: LatLon,
    max: LatLon,
) -> Result<(), Box<dyn std::error::Error>> {
    let (kernel, renderer) = create_headless_renderer(tile_size, tile_size, None).await?;

    let style = Style::default();

    let requested_layers = style.layers.clone();

    let plugins: Vec<Box<dyn Plugin<_>>> = vec![
        Box::new(RenderPlugin),
        Box::new(VectorPlugin::<DefaultVectorTransferables>::default()),
        Box::new(RasterPlugin::<DefaultRasterTransferables>::default()),
        Box::new(HeadlessPlugin::new(true)),
    ];

    let mut map = HeadlessMap::new(style, renderer, kernel, plugins)?;

    let tile_limits = google_mercator().tile_limits(
        extent_wgs84_to_merc(&Extent {
            minx: min.longitude,
            miny: min.latitude,
            maxx: max.longitude,
            maxy: max.latitude,
        }),
        0,
    );

    for (z, x, y) in GridIterator::new(10, 10, tile_limits) {
        let coords = WorldTileCoords::from((x as i32, y as i32, z.into()));
        let tile = map.fetch_tile(coords).await?;
        let mut layers = ProcessedLayers::default();
        for layer in &requested_layers {
            layers.append(&mut map.process_tile(tile.clone(), layer).await?);
        }
        map.render_tile(layers)?;
    }
    Ok(())
}
