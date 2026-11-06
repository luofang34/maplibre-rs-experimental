export default async function style() {
    const style = await (await fetch('https://demotiles.maplibre.org/style.json')).json();
    style.center = [0, 0]; // starting position [lng, lat]
    style.zoom = 1; // starting zoom
    return style;
}
