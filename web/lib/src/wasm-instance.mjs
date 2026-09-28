import init from "./wasm/maplibre.js";

// The generated bindings own the module's memory limits; workers reuse the main instance's
// memory only when it is shared.
export const initializeWasm = (source, memory) =>
    init({module_or_path: source, memory});
