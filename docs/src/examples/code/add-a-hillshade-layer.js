export default async function style() {
    return {
        version: 8,
        sources: {
            hillshadeSource: {
                type: 'raster-dem',
                url: 'https://demotiles.maplibre.org/terrain-tiles/tiles.json',
                tileSize: 256
            }
        },
        layers: [
            {
                id: 'hillshade',
                type: 'hillshade',
                source: 'hillshadeSource',
                paint: {
                    'hillshade-method': 'standard',
                    'hillshade-illumination-direction': 315,
                    'hillshade-shadow-color': '#000000',
                    'hillshade-highlight-color': '#FFFFFF',
                    'hillshade-accent-color': '#000000',
                    'hillshade-exaggeration': 0.5
                }
            }
        ],
        center: [11.491, 47.274],
        zoom: 10,
        pitch: 0
    };
}
