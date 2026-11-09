export default async function style() {
    const style = await (await fetch('https://tiles.openfreemap.org/styles/bright')).json();
    style.center = [12.338, 45.4385];
    style.zoom = 14;

    // The background follows a theme a page can switch without editing the layer.
    style.state = {theme: {default: 'day'}};
    const background = style.layers.find((layer) => layer.id === 'background');
    background.paint['background-color'] =
        ['match', ['global-state', 'theme'], 'night', '#1d2330', '#f8f4f0'];
    return style;
}

const SWATCHES = ['#ffffcc', '#a1dab4', '#41b6c4', '#2c7fb8', '#253494', '#fed976', '#feb24c', '#fd8d3c', '#f03b20', '#bd0026'];

export function run(map) {
    const menu = document.createElement('div');
    menu.style.cssText = 'position:absolute;top:10px;left:10px;padding:10px;width:150px;' +
        'background:#fff;border-radius:4px;font:12px sans-serif;';
    document.body.append(menu);

    const layer = document.createElement('select');
    layer.innerHTML = '<option value="water">Water</option><option value="building">Buildings</option>';
    menu.append(layer);

    const swatches = document.createElement('div');
    swatches.style.cssText = 'display:grid;grid-template-columns:repeat(5,1fr);gap:4px;margin:8px 0;';
    for (const color of SWATCHES) {
        const swatch = document.createElement('button');
        swatch.style.cssText = `height:20px;border:none;border-radius:2px;background:${color};cursor:pointer;`;
        swatch.addEventListener('click', () => map.setPaintProperty(layer.value, 'fill-color', color));
        swatches.append(swatch);
    }
    menu.append(swatches);

    const theme = document.createElement('button');
    theme.textContent = 'Night';
    theme.addEventListener('click', async () => {
        const night = theme.textContent === 'Night';
        await map.setGlobalStateProperty('theme', night ? 'night' : 'day');
        theme.textContent = night ? 'Day' : 'Night';
    });
    menu.append(theme);
}
