//! Shared style parsing and default plugin wiring for main-thread and offscreen maps.

use maplibre::{plugin::Plugin, style::Style};

use crate::{error::JSError, platform};

pub(crate) fn parse_style(style_json: &str) -> Result<Style, JSError> {
    let mut style = Style::from_json_str(style_json).map_err(JSError::InvalidStyle)?;
    // Layer order becomes render order; index 0 is reserved for the depth clear.
    for (index, layer) in style.layers.iter_mut().enumerate() {
        layer.index = index as u32 + 1;
    }
    Ok(style)
}

pub(crate) fn default_plugins_for<E>(_style: &Style) -> Vec<Box<dyn Plugin<E>>>
where
    E: maplibre::environment::Environment<
        OffscreenKernelEnvironment = platform::UsedOffscreenKernelEnvironment,
    >,
    <E::MapWindowConfig as maplibre::window::MapWindowConfig>::MapWindow:
        maplibre::window::HeadedMapWindow,
{
    let base: Vec<Box<dyn Plugin<E>>> = vec![
        Box::<maplibre::RenderPlugin>::default(),
        Box::<maplibre::background::BackgroundPlugin>::default(),
        Box::<maplibre::vector::VectorPlugin<platform::UsedVectorTransferables>>::default(),
        Box::new(maplibre::sdf::SdfPlugin::<platform::UsedVectorTransferables>::default()),
        Box::new(maplibre::heatmap::HeatmapPlugin),
        Box::new(maplibre::raster::RasterPlugin::<
            platform::UsedRasterTransferables,
        >::default()),
        Box::new(maplibre::hillshade::HillshadePlugin),
        Box::new(maplibre::map::TerrainPlugin::<platform::UsedDemTransferables>::default()),
        Box::new(maplibre::hud::NavigationHudPlugin),
    ];
    #[cfg(debug_assertions)]
    {
        let mut plugins = base;
        plugins.push(Box::<maplibre::debug::DebugPlugin>::default());
        plugins
    }
    #[cfg(not(debug_assertions))]
    {
        base
    }
}
