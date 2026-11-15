// Safari / WebKit omits or leaves undefined WebGPU Subgroups and fallback fields on
// GPUAdapterInfo, whereas wgpu's wasm-bindgen bindings read them as non-optional primitives.
// In multithreaded builds (+atomics), WASM linear memory is backed by SharedArrayBuffer, while
// Blink/WebKit WebGPU IDL methods (setBindGroup dynamicOffsets, writeBuffer, writeTexture)
// omit [AllowShared] and throw TypeError unless views are copied into unshared buffers.

const COMPAT_APPLIED = Symbol.for("maplibre.webgpuCompatApplied");

const isSharedBuffer = (buf, sabCtor) =>
    Boolean(sabCtor && buf instanceof sabCtor) ||
    Object.prototype.toString.call(buf) === "[object SharedArrayBuffer]";

const copySharedSlice = (srcBuffer, startByte, byteLen, getScratch) => {
    const safeLen = Math.max(0, byteLen);
    const scratch = getScratch(safeLen);
    if (safeLen > 0) {
        scratch.set(new Uint8Array(srcBuffer, startByte, safeLen), 0);
    }
    return { scratch, byteLength: safeLen };
};

const unshareByteSource = (data, dataOffset, size, sabCtor, getScratch) => {
    if (!data) return null;
    if (ArrayBuffer.isView(data)) {
        if (!isSharedBuffer(data.buffer, sabCtor)) return null;
        const bpe = data.BYTES_PER_ELEMENT || 1;
        const elemOffset = dataOffset ?? 0;
        const elemLength = size ?? (data.byteLength / bpe - elemOffset);
        const startByte = data.byteOffset + elemOffset * bpe;
        const byteLen = Math.max(0, elemLength * bpe);
        return copySharedSlice(data.buffer, startByte, byteLen, getScratch);
    }
    if (isSharedBuffer(data, sabCtor)) {
        const startByte = dataOffset ?? 0;
        const byteLen = size ?? (data.byteLength - startByte);
        return copySharedSlice(data, startByte, byteLen, getScratch);
    }
    return null;
};

export const ensureWebGpuCompatibility = (scope = globalThis) => {
    if (!scope || scope[COMPAT_APPLIED]) {
        return;
    }
    try {
        Object.defineProperty(scope, COMPAT_APPLIED, {
            value: true,
            configurable: true,
        });
    } catch {
        // Proceed even if scope is non-extensible.
    }

    const adapterInfoCtor = scope?.GPUAdapterInfo;
    const proto = adapterInfoCtor?.prototype;
    if (proto) {
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
    }

    const sabCtor = scope?.SharedArrayBuffer;
    let byteScratch = new Uint8Array(65536);
    const getByteScratch = (minBytes) => {
        if (byteScratch.byteLength < minBytes) {
            let nextCap = byteScratch.byteLength || 65536;
            while (nextCap < minBytes) {
                nextCap *= 2;
            }
            byteScratch = new Uint8Array(nextCap);
        }
        return byteScratch;
    };
    const queueProto = scope?.GPUQueue?.prototype;
    if (queueProto) {
        if (typeof queueProto.writeBuffer === "function") {
            const origWriteBuffer = queueProto.writeBuffer;
            queueProto.writeBuffer = function (buffer, bufferOffset, data, dataOffset, size) {
                const copy = unshareByteSource(data, dataOffset, size, sabCtor, getByteScratch);
                if (copy) {
                    return origWriteBuffer.call(
                        this,
                        buffer,
                        bufferOffset,
                        copy.scratch,
                        0,
                        copy.byteLength,
                    );
                }
                return size === undefined
                    ? dataOffset === undefined
                        ? origWriteBuffer.call(this, buffer, bufferOffset, data)
                        : origWriteBuffer.call(this, buffer, bufferOffset, data, dataOffset)
                    : origWriteBuffer.call(this, buffer, bufferOffset, data, dataOffset, size);
            };
        }

        if (typeof queueProto.writeTexture === "function") {
            const origWriteTexture = queueProto.writeTexture;
            queueProto.writeTexture = function (destination, data, dataLayout, size) {
                if (data && (ArrayBuffer.isView(data) ? isSharedBuffer(data.buffer, sabCtor) : isSharedBuffer(data, sabCtor))) {
                    const srcBuffer = ArrayBuffer.isView(data) ? data.buffer : data;
                    const baseByteOffset = ArrayBuffer.isView(data) ? data.byteOffset : 0;
                    const totalBytes = data.byteLength;
                    const rawOffset = dataLayout?.offset ?? 0;
                    const w = Array.isArray(size) ? (size[0] ?? 0) : (size?.width ?? 0);
                    const h = Array.isArray(size) ? (size[1] ?? 1) : (size?.height ?? 1);
                    const d = Array.isArray(size) ? (size[2] ?? 1) : (size?.depthOrArrayLayers ?? 1);
                    const avail = Math.max(0, totalBytes - rawOffset);
                    let byteLen = avail;
                    if (w === 0 || h === 0 || d === 0) {
                        byteLen = 0;
                    } else if (dataLayout?.bytesPerRow) {
                        const rowsPerImage = dataLayout.rowsPerImage ?? h;
                        const totalRows = (d - 1) * rowsPerImage + h;
                        byteLen = Math.min(avail, totalRows * dataLayout.bytesPerRow);
                    } else if (h === 1 && d === 1) {
                        byteLen = Math.min(avail, w * 16);
                    }
                    const { scratch, byteLength } = copySharedSlice(
                        srcBuffer,
                        baseByteOffset + rawOffset,
                        byteLen,
                        getByteScratch,
                    );
                    const view = scratch.subarray(0, byteLength);
                    const patchedLayout = rawOffset ? { ...dataLayout, offset: 0 } : dataLayout;
                    return origWriteTexture.call(this, destination, view, patchedLayout, size);
                }
                return origWriteTexture.call(this, destination, data, dataLayout, size);
            };
        }
    }

    // Reusable scratch Uint32Array for dynamic bind group offsets so zero heap allocations occur per draw.
    let offsetScratch = new Uint32Array(32);
    const patchSetBindGroup = (encoderProto) => {
        if (!encoderProto || typeof encoderProto.setBindGroup !== "function") {
            return;
        }
        const origSetBindGroup = encoderProto.setBindGroup;
        encoderProto.setBindGroup = function (index, bindGroup, dynamicOffsets, start, length) {
            if (
                dynamicOffsets &&
                ArrayBuffer.isView(dynamicOffsets) &&
                isSharedBuffer(dynamicOffsets.buffer, sabCtor)
            ) {
                const srcStart = start ?? 0;
                const srcLen = length ?? (dynamicOffsets.length - srcStart);
                if (srcLen <= 0) {
                    return origSetBindGroup.call(this, index, bindGroup);
                }
                if (srcLen > offsetScratch.length) {
                    offsetScratch = new Uint32Array(Math.max(srcLen, offsetScratch.length * 2));
                }
                for (let i = 0; i < srcLen; i++) {
                    offsetScratch[i] = dynamicOffsets[srcStart + i];
                }
                return origSetBindGroup.call(this, index, bindGroup, offsetScratch, 0, srcLen);
            }
            if (arguments.length <= 2 || (arguments.length === 3 && dynamicOffsets === undefined)) {
                return origSetBindGroup.call(this, index, bindGroup);
            }
            if (arguments.length === 3) {
                return origSetBindGroup.call(this, index, bindGroup, dynamicOffsets);
            }
            return origSetBindGroup.call(this, index, bindGroup, dynamicOffsets, start, length);
        };
    };

    patchSetBindGroup(scope?.GPURenderPassEncoder?.prototype);
    patchSetBindGroup(scope?.GPUComputePassEncoder?.prototype);
    patchSetBindGroup(scope?.GPURenderBundleEncoder?.prototype);
};
