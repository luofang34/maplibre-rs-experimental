export default async function style() {
    const style = await (await fetch('https://demotiles.maplibre.org/style.json')).json();
    style.projection = {type: 'globe'};
    style.center = [0, 0];
    style.zoom = 2;
    return style;
}
