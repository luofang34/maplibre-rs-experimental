// Safari / WebKit omits or leaves undefined WebGPU Subgroups and fallback fields on
// GPUAdapterInfo, whereas wgpu's wasm-bindgen bindings read them as non-optional primitives.
export const ensureWebGpuCompatibility = (scope = globalThis) => {
    const adapterInfoCtor = scope?.GPUAdapterInfo;
    const proto = adapterInfoCtor?.prototype;
    if (!proto) {
        return;
    }

    const defaults = {
        vendor: "",
        architecture: "",
        device: "",
        description: "",
        subgroupMinSize: 0,
        subgroupMaxSize: 0,
        isFallbackAdapter: false,
    };

    for (const [key, fallback] of Object.entries(defaults)) {
        const descriptor = Object.getOwnPropertyDescriptor(proto, key);
        if (!descriptor) {
            try {
                Object.defineProperty(proto, key, {
                    get: () => fallback,
                    configurable: true,
                });
            } catch {
                // Ignore non-extensible host prototypes.
            }
        } else if (descriptor.configurable && typeof descriptor.get === "function") {
            const origGet = descriptor.get;
            Object.defineProperty(proto, key, {
                ...descriptor,
                get() {
                    const value = origGet.call(this);
                    return value === undefined || value === null ? fallback : value;
                },
            });
        }
    }
};
