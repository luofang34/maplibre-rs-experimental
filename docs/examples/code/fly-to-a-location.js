export default async function style() {
    const style = await (await fetch('https://tiles.openfreemap.org/styles/bright')).json();
    style.center = [-74.5, 40];
    style.zoom = 9;
    return style;
}

export function run(map) {
    const button = document.createElement('button');
    button.textContent = 'Fly';
    button.style.cssText = 'position:absolute;top:10px;left:50%;transform:translateX(-50%);' +
        'padding:10px 20px;font:bold 14px sans-serif;border:none;border-radius:4px;' +
        'background:#ee8a65;color:#fff;cursor:pointer;';
    document.body.append(button);

    button.addEventListener('click', async () => {
        // Somewhere within five degrees of the starting point.
        const arrived = await map.flyTo({
            center: [-74.5 + (Math.random() - 0.5) * 10, 40 + (Math.random() - 0.5) * 10],
            essential: true
        });
        console.log(arrived ? 'arrived' : 'stopped on the way');
    });
}
