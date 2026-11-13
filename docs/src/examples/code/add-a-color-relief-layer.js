export default async function style() {
    return {
        version: 8,
        sources: {
            elevationSource: {
                type: 'raster-dem',
                url: 'https://demotiles.maplibre.org/terrain-tiles/tiles.json',
                tileSize: 256
            }
        },
        layers: [
            {
                id: 'background',
                type: 'background',
                paint: {
                    'background-color': '#1b263b'
                }
            },
            {
                id: 'color-relief',
                type: 'color-relief',
                source: 'elevationSource',
                paint: {
                    'color-relief-opacity': 0.85,
                    'color-relief-color': [
                        'interpolate',
                        ['linear'],
                        ['elevation'],
                        0, '#2b5c8f',
                        500, '#3b8c5a',
                        1000, '#8fbc6b',
                        1500, '#d9c589',
                        2000, '#b88b5a',
                        2500, '#8c6239',
                        3200, '#ffffff'
                    ]
                }
            },
            {
                id: 'hillshade',
                type: 'hillshade',
                source: 'elevationSource',
                paint: {
                    'hillshade-method': 'standard',
                    'hillshade-exaggeration': 0.35
                }
            }
        ],
        projection: {
            type: 'globe'
        },
        center: [11.39085, 47.27574],
        zoom: 10.5,
        pitch: 0
    };
}
