export default async function style() {
    return {
        'version': 8,
        'sources': {
            'raster-tiles': {
                'type': 'raster',
                'tiles': ['https://tile.openstreetmap.org/{z}/{x}/{y}.png'],
                'tileSize': 256,
                'minzoom': 0,
                'maxzoom': 19,
                'attribution': '© OpenStreetMap contributors',
            }
        },
        'layers': [
            {
                'id': 'simple-tiles',
                'type': 'raster',
                'source': 'raster-tiles',
            }
        ],
        'id': 'blank',
        'center': [0, 0], // starting position
        'zoom': 0 // starting zoom
    };
}
