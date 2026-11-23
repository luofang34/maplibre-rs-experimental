import {ensureWebGpuCompatibility} from "./webgpu-compat.mjs";

export {ensureWebGpuCompatibility};

export interface GraphicsBackendInput {
    webgl: boolean;
    hasWebGpu: boolean;
    hasWebGl: boolean;
    hasWebglFallbackModule?: boolean;
    multithreaded?: boolean;
    crossOriginIsolated?: boolean;
    isSecureContext?: boolean;
}

export type GraphicsBackendDecision =
    | { backend: "webgpu" }
    | { backend: "webgl2" }
    | { backend: "fallback-webgl2" }
    | { backend: "error"; message: string };

/** Selects WebGPU, native WebGL2, or runtime WebGL2 fallback before engine startup. */
export const selectGraphicsBackend = ({
    webgl,
    hasWebGpu,
    hasWebGl,
    hasWebglFallbackModule = true,
    multithreaded = false,
    crossOriginIsolated = true,
    isSecureContext = true,
}: GraphicsBackendInput): GraphicsBackendDecision => {
    if (multithreaded && (!isSecureContext || !crossOriginIsolated)) {
        if (!webgl && hasWebGl && hasWebglFallbackModule) {
            return {backend: "fallback-webgl2"};
        }
        return {
            backend: "error",
            message: !isSecureContext
                ? "isSecureContext is false!"
                : "crossOriginIsolated is false! " +
                  "The Cross-Origin-Opener-Policy and Cross-Origin-Embedder-Policy HTTP headers are required.",
        };
    }
    if (webgl) {
        return hasWebGl
            ? {backend: "webgl2"}
            : {backend: "error", message: "WebGL is not supported in this Browser!"};
    }
    if (hasWebGpu) {
        return {backend: "webgpu"};
    }
    if (hasWebGl && hasWebglFallbackModule) {
        return {backend: "fallback-webgl2"};
    }
    return {
        backend: "error",
        message: "WebGPU is not supported in this Browser and WebGL2 fallback is unavailable!",
    };
};

let webGpuProbe: Promise<boolean> | null = null;

/** Resets cached WebGPU adapter detection (intended for Node unit tests). */
export const resetWebGpuProbeForTest = (): void => {
    webGpuProbe = null;
};

/** Probes `navigator.gpu.requestAdapter()`, caching the result for default `globalThis.navigator`. */
export const probeWebGpuAvailable = (
    nav: { gpu?: { requestAdapter?: () => Promise<unknown> } } | undefined =
        (globalThis as { navigator?: { gpu?: { requestAdapter?: () => Promise<unknown> } } }).navigator,
): Promise<boolean> => {
    const defaultNav = (globalThis as { navigator?: unknown }).navigator;
    if (nav !== defaultNav) {
        return (async () => {
            try {
                if (!nav?.gpu || typeof nav.gpu.requestAdapter !== "function") {
                    return false;
                }
                return Boolean(await nav.gpu.requestAdapter());
            } catch {
                return false;
            }
        })();
    }
    if (!webGpuProbe) {
        webGpuProbe = (async () => {
            try {
                const g = (defaultNav as { gpu?: { requestAdapter?: () => Promise<unknown> } } | undefined)?.gpu;
                if (!g || typeof g.requestAdapter !== "function") {
                    return false;
                }
                return Boolean(await g.requestAdapter());
            } catch {
                return false;
            }
        })();
    }
    return webGpuProbe;
};

export const checkRequirements = () => {
    if (MULTITHREADED) {
        if (!isSecureContext) {
            return "isSecureContext is false!"
        }

        if (!crossOriginIsolated) {
            return "crossOriginIsolated is false! " +
                "The Cross-Origin-Opener-Policy and Cross-Origin-Embedder-Policy HTTP headers are required."
        }
    }

    if (WEBGL) {
        if (!isWebGLSupported()) {
            return "WebGL is not supported in this Browser!"
        }
    }

    return null
}

export const isWebGLSupported = () => {
    try {
        const canvas = document.createElement('canvas')
        return Boolean(canvas.getContext("webgl2") || canvas.getContext("webgl"));
    } catch (x) {
        return false
    }
}
