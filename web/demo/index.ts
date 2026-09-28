import { startMapLibre } from 'maplibre-rs'

// `?style=<url>` loads any style; `?projection=globe|mercator|vertical-perspective` overrides its
// projection. Without a style URL the MapLibre demotiles world style is shown on the globe.
const params = new URLSearchParams(location.search)
const styleUrl = params.get('style') ?? 'https://demotiles.maplibre.org/style.json'
const projection = params.get('projection') ?? 'globe'

const loadStyle = async (): Promise<string | undefined> => {
  try {
    const response = await fetch(styleUrl)
    if (!response.ok) {
      throw new Error(`${response.status} ${response.statusText}`)
    }
    const style = await response.json()
    style.projection = { type: projection }
    return JSON.stringify(style)
  } catch (error) {
    console.error(`Cannot load style ${styleUrl}, falling back to the built-in style`, error)
    return undefined
  }
}

loadStyle().then((styleJson) => startMapLibre(undefined, undefined, styleJson))
