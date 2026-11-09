const PLACES = {
    type: 'FeatureCollection',
    features: [
        {
            type: 'Feature',
            properties: { category: 'theatre' },
            geometry: { type: 'Point', coordinates: [-77.038659, 38.931567] }
        },
        {
            type: 'Feature',
            properties: { category: 'theatre' },
            geometry: { type: 'Point', coordinates: [-77.003168, 38.894651] }
        },
        {
            type: 'Feature',
            properties: { category: 'museum' },
            geometry: { type: 'Point', coordinates: [-77.031706, 38.914581] }
        },
        {
            type: 'Feature',
            properties: { category: 'museum' },
            geometry: { type: 'Point', coordinates: [-77.020945, 38.878241] }
        },
        {
            type: 'Feature',
            properties: { category: 'music' },
            geometry: { type: 'Point', coordinates: [-77.007481, 38.876516] }
        },
        {
            type: 'Feature',
            properties: { category: 'music' },
            geometry: { type: 'Point', coordinates: [-77.090372, 38.881189] }
        }
    ]
};

export default async function style() {
    const style = await (await fetch('https://tiles.openfreemap.org/styles/bright')).json();
    style.center = [-77.04, 38.907];
    style.zoom = 11.5;

    style.sources['places'] = {
        type: 'geojson',
        data: PLACES
    };

    style.layers.push({
        id: 'places',
        type: 'circle',
        source: 'places',
        paint: {
            'circle-radius': 8,
            'circle-stroke-width': 2,
            'circle-color': [
                'match',
                ['get', 'category'],
                'theatre', '#e11d48',
                'museum', '#2563eb',
                '#16a34a'
            ],
            'circle-stroke-color': '#ffffff'
        }
    });

    return style;
}

const CATEGORIES = [
    { value: 'all', label: 'All' },
    { value: 'theatre', label: 'Theatre' },
    { value: 'museum', label: 'Museum' },
    { value: 'music', label: 'Music' }
];

export function run(map) {
    const menu = document.createElement('div');
    menu.style.cssText = 'position:absolute;top:10px;left:10px;display:flex;gap:4px;' +
        'background:#fff;padding:6px;border-radius:4px;font:12px sans-serif;';
    document.body.append(menu);

    for (const { value, label } of CATEGORIES) {
        const button = document.createElement('button');
        button.textContent = label;
        button.style.cssText = 'padding:5px 10px;border:1px solid #cbd5e1;border-radius:3px;' +
            'background:#f8fafc;cursor:pointer;';
        button.addEventListener('click', () => {
            const filter = value === 'all' ? null : ['==', ['get', 'category'], value];
            map.setFilter('places', filter);
        });
        menu.append(button);
    }
}
