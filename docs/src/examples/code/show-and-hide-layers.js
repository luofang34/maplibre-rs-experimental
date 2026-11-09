export default async function style() {
    const style = await (await fetch('https://tiles.openfreemap.org/styles/bright')).json();
    style.center = [-71.977221, -13.517379];
    style.zoom = 14;
    return style;
}

const TOGGLE_LAYERS = [
    { id: 'water', label: 'Water' },
    { id: 'building', label: 'Buildings' },
    { id: 'park', label: 'Parks' }
];

export function run(map) {
    const menu = document.createElement('div');
    menu.style.cssText = 'position:absolute;top:10px;left:10px;display:flex;flex-direction:column;' +
        'gap:4px;background:#fff;padding:8px;border-radius:4px;font:12px sans-serif;';
    document.body.append(menu);

    for (const { id, label } of TOGGLE_LAYERS) {
        let visible = true;
        const button = document.createElement('button');
        button.textContent = `Hide ${label}`;
        button.style.cssText = 'padding:6px 10px;border:1px solid #cbd5e1;border-radius:3px;' +
            'background:#3887be;color:#fff;cursor:pointer;text-align:left;';
        button.addEventListener('click', async () => {
            visible = !visible;
            await map.setLayoutProperty(id, 'visibility', visible ? 'visible' : 'none');
            button.textContent = `${visible ? 'Hide' : 'Show'} ${label}`;
            button.style.background = visible ? '#3887be' : '#f1f5f9';
            button.style.color = visible ? '#fff' : '#334155';
        });
        menu.append(button);
    }
}
