/**
 * Normalizes a style argument into `{ resolved, value, promise }`, kicking off `fetch()`
 * immediately for style URLs so HTTP I/O overlaps WASM compilation and WebGPU initialization.
 *
 * @param {unknown} style
 * @param {((url: string, resourceType: string) => unknown) | undefined} transformRequest
 * @param {{ fetchImpl?: typeof globalThis.fetch, signal?: AbortSignal }} [opts]
 */
export const normalizeStyleInput = (
    style,
    transformRequest = undefined,
    { fetchImpl = globalThis.fetch, signal = undefined } = {},
) => {
    if (style === undefined || style === null) {
        return { resolved: true, value: undefined, promise: Promise.resolve(undefined) };
    }
    if (typeof style === "string") {
        const trimmed = style.trimStart();
        if (!trimmed || trimmed.startsWith("{") || trimmed.startsWith("[")) {
            return { resolved: true, value: style, promise: Promise.resolve(style) };
        }
        let targetUrl = style;
        let init;
        if (typeof transformRequest === "function") {
            const transformed = transformRequest(style, "Style");
            if (typeof transformed === "string") {
                targetUrl = transformed;
            } else if (transformed && typeof transformed === "object" && typeof transformed.url === "string") {
                targetUrl = transformed.url;
                const reqInit = {};
                if (transformed.headers !== undefined) reqInit.headers = transformed.headers;
                if (transformed.credentials !== undefined) reqInit.credentials = transformed.credentials;
                if (transformed.mode !== undefined) reqInit.mode = transformed.mode;
                if (Object.keys(reqInit).length > 0) init = reqInit;
            }
        }
        if (signal) {
            init = init ? { ...init, signal } : { signal };
        }
        if (typeof fetchImpl !== "function") {
            return {
                resolved: false,
                value: undefined,
                promise: Promise.reject(new Error(`fetch is unavailable for style URL: ${targetUrl}`)),
            };
        }
        const promise = Promise.resolve(fetchImpl(targetUrl, init)).then(async (response) => {
            if (!response.ok) {
                throw new Error(`HTTP ${response.status} fetching style ${targetUrl}`);
            }
            return response.text();
        });
        return { resolved: false, value: undefined, promise };
    }
    if (typeof style === "object") {
        if (typeof style.then === "function") {
            const promise = Promise.resolve(style).then(
                (inner) => normalizeStyleInput(inner, transformRequest, { fetchImpl, signal }).promise,
            );
            return { resolved: false, value: undefined, promise };
        }
        const json = JSON.stringify(style);
        return { resolved: true, value: json, promise: Promise.resolve(json) };
    }
    return { resolved: true, value: style, promise: Promise.resolve(style) };
};
