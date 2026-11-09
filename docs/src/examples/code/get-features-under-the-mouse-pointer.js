export default async function style() {
    const style = await (await fetch('https://tiles.openfreemap.org/styles/bright')).json();
    style.center = [-74.006, 40.7128];
    style.zoom = 13;
    return style;
}

export function run(map) {
    const panel = document.createElement('pre');
    panel.style.cssText = 'position:absolute;top:0;left:0;margin:10px;padding:10px;' +
        'width:40%;max-height:60%;overflow:auto;font:12px monospace;' +
        'background:rgba(255,255,255,0.85);border-radius:4px;';
    panel.textContent = 'Move the pointer over a label.';
    document.body.append(panel);

    // The latest move wins: an answer to an earlier one that arrives late is dropped.
    let latest = 0;
    map.on('mousemove', async (event) => {
        const move = ++latest;
        const features = await map.queryRenderedFeatures(event.point);
        if (move !== latest) return;
        panel.textContent = JSON.stringify(
            features.map(({layer, sourceLayer, properties}) => ({layer: layer.id, sourceLayer, properties})),
            null,
            2
        );
    });
}
