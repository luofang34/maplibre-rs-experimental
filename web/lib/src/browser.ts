import {
    bigInt,
    bulkMemory,
    exceptions,
    multiValue,
    mutableGlobals,
    referenceTypes,
    saturatedFloatToInt,
    signExtensions,
    simd,
    tailCall,
    threads
} from "wasm-feature-detect"


import {ensureWebGpuCompatibility} from "./webgpu-compat.mjs";

export {ensureWebGpuCompatibility};

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
    } else {
        if (!("gpu" in navigator)) {
            return "WebGPU is not supported in this Browser!"
        }
    }

    return null
}

export const isWebGLSupported = () => {
    try {
        const canvas = document.createElement('canvas')
        canvas.getContext("webgl")
        return true
    } catch (x) {
        return false
    }
}

let checkedFeatures: Promise<void> | null = null;

export const checkWasmFeatures = (): Promise<void> => {
    if (!checkedFeatures) {
        checkedFeatures = (async () => {
            const entries: Array<[string, () => Promise<boolean>]> = [
                ["bulkMemory", bulkMemory],
                ["exceptions", exceptions],
                ["multiValue", multiValue],
                ["mutableGlobals", mutableGlobals],
                ["referenceTypes", referenceTypes],
                ["saturatedFloatToInt", saturatedFloatToInt],
                ["signExtensions", signExtensions],
                ["simd", simd],
                ["tailCall", tailCall],
                ["threads", threads],
                ["bigInt", bigInt],
            ];
            await Promise.all(entries.map(async ([name, detect]) => {
                if (!(await detect())) {
                    console.warn(`WASM feature ${name} is unsupported in this browser.`);
                }
            }));
        })();
    }
    return checkedFeatures;
}
