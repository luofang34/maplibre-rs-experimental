import init from "./wasm/maplibre.js";

// Pass thread_stack_size whenever SharedArrayBuffer memory is supplied so any worker
// initializing after another thread does not fall back to the 2 MiB default stack.
export const initializeWasm = (source, memory, _primary = !memory) =>
    memory
        ? init({module_or_path: source, memory, thread_stack_size: 196608})
        : init({module_or_path: source, memory});
