import { startMapLibre } from 'maplibre-rs'

// `?style=<url>` loads any style; `?example=<path>` instead imports a same-origin module whose
// default export returns a style document. `?projection=globe|mercator|vertical-perspective`
// overrides the projection, and `?lat=`, `?lon=`, `?zoom=`, `?pitch=`, `?bearing=` override the
// initial camera. Without a style URL or example the MapLibre demotiles world style is shown on
// the globe.
const params = new URLSearchParams(location.search)
const customStyleUrl = params.get('style')
const examplePath = params.get('example')
const styleUrl = customStyleUrl ?? 'https://demotiles.maplibre.org/style.json'
const projection =
  params.get('projection') ?? (customStyleUrl || examplePath ? null : 'globe')

const numberParam = (name: string): number | null => {
  const raw = params.get(name)
  if (raw === null || raw.trim() === '') return null
  const parsed = Number(raw)
  return Number.isFinite(parsed) ? parsed : null
}

const fetchStyle = async (): Promise<any> => {
  const response = await fetch(styleUrl)
  if (!response.ok) {
    throw new Error(`${response.status} ${response.statusText}`)
  }
  return response.json()
}

// Only modules of the page's own origin run, so a link cannot make the page execute foreign code.
const importExample = async (path: string): Promise<any> => {
  const url = new URL(path, location.href)
  if (url.origin !== location.origin) {
    throw new Error(`example ${path} is not on ${location.origin}`)
  }
  const module = await import(/* @vite-ignore */ url.href)
  return typeof module.default === 'function' ? module.default() : module.default
}

const loadStyle = async (): Promise<string | undefined> => {
  const source = examplePath ?? styleUrl
  try {
    const style = examplePath ? await importExample(examplePath) : await fetchStyle()
    if (projection) {
      style.projection = { type: projection }
    }
    const lat = numberParam('lat')
    const lon = numberParam('lon')
    if (lat !== null || lon !== null) {
      const base = Array.isArray(style.center) ? style.center : [0, 0]
      style.center = [lon ?? base[0], lat ?? base[1]]
    }
    const zoom = numberParam('zoom')
    if (zoom !== null) style.zoom = zoom
    const pitch = numberParam('pitch')
    if (pitch !== null) style.pitch = pitch
    const bearing = numberParam('bearing')
    if (bearing !== null) style.bearing = bearing
    return JSON.stringify(style)
  } catch (error) {
    console.error(`Cannot load style ${source}, falling back to the built-in style`, error)
    return undefined
  }
}

loadStyle().then((styleJson) => startMapLibre(undefined, undefined, styleJson))
