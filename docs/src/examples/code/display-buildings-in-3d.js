export default async function style() {
    const style = await (await fetch('https://tiles.openfreemap.org/styles/bright')).json();
    style.center = [-74.0066, 40.7135];
    style.zoom = 15.5;
    style.pitch = 45;
    style.bearing = -17.6;

    style.sources.openfreemap = {
        url: 'https://tiles.openfreemap.org/planet',
        type: 'vector'
    };
    // Insert the layer beneath the first symbol layer with labels.
    const labelLayer = style.layers.findIndex(
        (layer) => layer.type === 'symbol' && layer.layout?.['text-field']
    );
    style.layers.splice(labelLayer < 0 ? style.layers.length : labelLayer, 0, {
        'id': '3d-buildings',
        'source': 'openfreemap',
        'source-layer': 'building',
        'type': 'fill-extrusion',
        'minzoom': 15,
        'filter': ['!=', ['get', 'hide_3d'], true],
        'paint': {
            'fill-extrusion-color': [
                'interpolate',
                ['linear'],
                ['get', 'render_height'], 0, 'lightgray', 200, 'royalblue', 400, 'lightblue'
            ],
            'fill-extrusion-height': [
                'interpolate',
                ['linear'],
                ['zoom'],
                15,
                0,
                16,
                ['get', 'render_height']
            ],
            'fill-extrusion-base': [
                'interpolate',
                ['linear'],
                ['zoom'],
                15,
                0,
                16,
                ['get', 'render_min_height']
            ]
        }
    });
    return style;
}
