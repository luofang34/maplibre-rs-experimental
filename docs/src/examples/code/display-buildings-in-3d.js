export default async function style() {
    const style = await (await fetch('https://tiles.openfreemap.org/styles/bright')).json();
    style.projection = { type: 'globe' };
    style.center = [-74.0066, 40.7135];
    style.zoom = 15.5;
    style.pitch = 45;
    style.bearing = -17.6;

    const buildingSource = style.sources.openmaptiles ? 'openmaptiles' : 'openfreemap';
    if (!style.sources[buildingSource]) {
        style.sources[buildingSource] = {
            url: 'https://tiles.openfreemap.org/planet',
            type: 'vector'
        };
    }
    style.layers = style.layers.filter((layer) => layer.id !== 'building-top');
    // Insert the layer beneath the first symbol layer with labels.
    const labelLayer = style.layers.findIndex(
        (layer) => layer.type === 'symbol' && layer.layout?.['text-field']
    );
    style.layers.splice(labelLayer < 0 ? style.layers.length : labelLayer, 0, {
        'id': '3d-buildings',
        'source': buildingSource,
        'source-layer': 'building',
        'type': 'fill-extrusion',
        'minzoom': 14,
        'filter': ['!=', ['get', 'hide_3d'], true],
        'paint': {
            'fill-extrusion-color': [
                'interpolate',
                ['linear'],
                ['get', 'render_height'], 0, 'lightgray', 200, 'royalblue', 400, 'lightblue'
            ],
            'fill-extrusion-height': ['get', 'render_height'],
            'fill-extrusion-base': ['get', 'render_min_height']
        }
    });
    return style;
}
