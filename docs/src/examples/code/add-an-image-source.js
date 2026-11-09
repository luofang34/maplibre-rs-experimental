export default async function style() {
    const style = await (await fetch('https://tiles.openfreemap.org/styles/bright')).json();
    style.projection = { type: 'globe' };
    style.center = [-75.789, 41.874];
    style.zoom = 5;

    style.sources['radar'] = {
        type: 'image',
        url: 'https://maplibre.org/maplibre-gl-js/docs/assets/radar.gif',
        coordinates: [
            [-80.425, 46.437],
            [-71.516, 46.437],
            [-71.516, 37.936],
            [-80.425, 37.936]
        ]
    };

    style.layers.push({
        id: 'radar-layer',
        type: 'raster',
        source: 'radar',
        paint: {
            'raster-opacity': 0.85,
            'raster-fade-duration': 0
        }
    });

    return style;
}
