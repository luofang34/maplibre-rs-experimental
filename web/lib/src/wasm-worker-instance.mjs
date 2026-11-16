import init from "./wasm/maplibre_tile_worker.js";

export const initializeWorkerWasm = (source, memory) =>
    init({module_or_path: source, memory});
