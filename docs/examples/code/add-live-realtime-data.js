export default async function style() {
    const style = await (await fetch('https://tiles.openfreemap.org/styles/bright')).json();
    style.center = [0, 0];
    style.zoom = 2;

    style.sources['iss'] = {
        'type': 'geojson',
        'data': {'type': 'FeatureCollection', 'features': []}
    };
    style.layers.push({
        'id': 'iss',
        'type': 'circle',
        'source': 'iss',
        'paint': {
            'circle-radius': 8,
            'circle-color': '#e55e5e'
        }
    });
    return style;
}

export function run(map) {
    const update = async () => {
        // Where the International Space Station is now.
        const response = await fetch('https://api.wheretheiss.at/v1/satellites/25544');
        const {longitude, latitude} = await response.json();
        await map.setGeoJsonData('iss', {
            'type': 'Feature',
            'properties': {},
            'geometry': {'type': 'Point', 'coordinates': [longitude, latitude]}
        });
        await map.easeTo({center: [longitude, latitude], duration: 1000});
    };
    const tick = () => update().catch((error) => console.error(error)).finally(() => setTimeout(tick, 2000));
    tick();
}
