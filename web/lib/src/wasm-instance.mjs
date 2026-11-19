import init from "./wasm/maplibre.js";

// The generated bindings own the module's memory limits; workers reuse the main instance's
// memory only when it is shared.
export const initializeWasm = (source, memory) =>
    memory
        ? init({module_or_path: source, memory, thread_stack_size: 262144})
        : init({module_or_path: source, memory});
