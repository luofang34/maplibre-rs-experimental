import init from "./wasm/maplibre.js";

// The generated bindings own the module's memory limits; workers reuse the main instance's
// memory only when it is shared. Pre-created shared memory passes primary = true on the render worker.
export const initializeWasm = (source, memory, primary = !memory) =>
    memory && !primary
        ? init({module_or_path: source, memory, thread_stack_size: 262144})
        : init({module_or_path: source, memory});
