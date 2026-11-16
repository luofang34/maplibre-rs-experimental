// Safari / WebKit omits or leaves undefined WebGPU Subgroups and fallback fields on
// GPUAdapterInfo, whereas wgpu's wasm-bindgen bindings read them as non-optional primitives.
// In multithreaded builds (+atomics), WASM linear memory is backed by SharedArrayBuffer, while
// Blink/WebKit WebGPU IDL methods (setBindGroup dynamicOffsets, writeBuffer, writeTexture)
// omit [AllowShared] and throw TypeError unless views are copied into unshared buffers.

const COMPAT_APPLIED = Symbol.for("maplibre.webgpuCompatApplied");

const isSharedBuffer = (buf, sabCtor) =>
    Boolean(sabCtor && buf instanceof sabCtor) ||
    Object.prototype.toString.call(buf) === "[object SharedArrayBuffer]";

const copySharedBytes = (srcBuffer, startByte, byteLen, getSabView, getScratch) => {
    const safeLen = Math.max(0, byteLen);
    const scratch = getScratch(safeLen);
    if (safeLen > 0) {
        const srcView = getSabView(srcBuffer);
        scratch.set(srcView.subarray(startByte, startByte + safeLen), 0);
    }
    return safeLen;
};

const unshareByteSource = (data, dataOffset, size, sabCtor, getSabView, getScratch) => {
    if (!data) return -1;
    if (ArrayBuffer.isView(data)) {
        if (!isSharedBuffer(data.buffer, sabCtor)) return -1;
        const bpe = data.BYTES_PER_ELEMENT || 1;
        const elemOffset = dataOffset ?? 0;
        const elemLength = size ?? (data.byteLength / bpe - elemOffset);
        const startByte = data.byteOffset + elemOffset * bpe;
        const byteLen = Math.max(0, elemLength * bpe);
        return copySharedBytes(data.buffer, startByte, byteLen, getSabView, getScratch);
    }
    if (isSharedBuffer(data, sabCtor)) {
        const startByte = dataOffset ?? 0;
        const byteLen = size ?? (data.byteLength - startByte);
        return copySharedBytes(data, startByte, byteLen, getSabView, getScratch);
    }
    return -1;
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
    let cachedSab = null;
    let cachedSabLen = -1;
    let cachedSabView = null;
    const getSabView = (srcBuffer) => {
        if (srcBuffer !== cachedSab || srcBuffer.byteLength !== cachedSabLen || !cachedSabView) {
            cachedSab = srcBuffer;
            cachedSabLen = srcBuffer.byteLength;
            cachedSabView = new Uint8Array(srcBuffer);
        }
        return cachedSabView;
    };
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
    const texLayoutScratch = { offset: 0, bytesPerRow: undefined, rowsPerImage: undefined };
    let curPasses = 0;
    let curSetVertexBuffers = 0;
    let curWriteBufferCalls = 0;
    let curWriteBufferBytes = 0;
    const lastGpuApiStats = new Float64Array(4);
    try {
        Object.defineProperty(scope, "__maplibreLastGpuApiStats", {
            get: () => lastGpuApiStats,
            configurable: true,
        });
    } catch {
        // Ignore non-extensible global objects.
    }

    const queueProto = scope?.GPUQueue?.prototype;
    if (queueProto) {
        if (typeof queueProto.writeBuffer === "function") {
            const origWriteBuffer = queueProto.writeBuffer;
            queueProto.writeBuffer = function (buffer, bufferOffset, data, dataOffset, size) {
                curWriteBufferCalls++;
                const copiedLen = unshareByteSource(
                    data,
                    dataOffset,
                    size,
                    sabCtor,
                    getSabView,
                    getByteScratch,
                );
                if (copiedLen >= 0) {
                    curWriteBufferBytes += copiedLen;
                    return origWriteBuffer.call(
                        this,
                        buffer,
                        bufferOffset,
                        byteScratch,
                        0,
                        copiedLen,
                    );
                }
                if (data) {
                    const bpe = ArrayBuffer.isView(data) ? (data.BYTES_PER_ELEMENT || 1) : 1;
                    const elemOff = dataOffset ?? 0;
                    const elemLen = size ?? (data.byteLength / bpe - elemOff);
                    curWriteBufferBytes += Math.max(0, elemLen * bpe);
                }
                return size === undefined
                    ? dataOffset === undefined
                        ? origWriteBuffer.call(this, buffer, bufferOffset, data)
                        : origWriteBuffer.call(this, buffer, bufferOffset, data, dataOffset)
                    : origWriteBuffer.call(this, buffer, bufferOffset, data, dataOffset, size);
            };
        }

        if (typeof queueProto.submit === "function") {
            const origSubmit = queueProto.submit;
            queueProto.submit = function (commandBuffers) {
                if (curPasses > 0) {
                    lastGpuApiStats[0] = curPasses;
                    lastGpuApiStats[1] = curSetVertexBuffers;
                    lastGpuApiStats[2] = curWriteBufferCalls;
                    lastGpuApiStats[3] = curWriteBufferBytes;
                    curPasses = 0;
                    curSetVertexBuffers = 0;
                    curWriteBufferCalls = 0;
                    curWriteBufferBytes = 0;
                }
                return origSubmit.call(this, commandBuffers);
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
                    const byteLength = copySharedBytes(
                        srcBuffer,
                        baseByteOffset + rawOffset,
                        byteLen,
                        getSabView,
                        getByteScratch,
                    );
                    const view =
                        byteScratch.byteLength === byteLength
                            ? byteScratch
                            : byteScratch.subarray(0, byteLength);
                    let patchedLayout = dataLayout;
                    if (rawOffset) {
                        texLayoutScratch.offset = 0;
                        texLayoutScratch.bytesPerRow = dataLayout.bytesPerRow;
                        texLayoutScratch.rowsPerImage = dataLayout.rowsPerImage;
                        patchedLayout = texLayoutScratch;
                    }
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

    const encProto = scope?.GPUCommandEncoder?.prototype;
    if (encProto && typeof encProto.beginRenderPass === "function") {
        const origBeginRenderPass = encProto.beginRenderPass;
        encProto.beginRenderPass = function (descriptor) {
            curPasses++;
            return origBeginRenderPass.call(this, descriptor);
        };
    }
    const rpeProto = scope?.GPURenderPassEncoder?.prototype;
    if (rpeProto && typeof rpeProto.setVertexBuffer === "function") {
        const origSetVertexBuffer = rpeProto.setVertexBuffer;
        rpeProto.setVertexBuffer = function (slot, buffer, offset, size) {
            curSetVertexBuffers++;
            if (arguments.length <= 2) {
                return origSetVertexBuffer.call(this, slot, buffer);
            }
            if (arguments.length === 3) {
                return origSetVertexBuffer.call(this, slot, buffer, offset);
            }
            return origSetVertexBuffer.call(this, slot, buffer, offset, size);
        };
    }

    let liveGpuBufferBytes = 0;
    const deviceReleasers = typeof WeakMap === "function" ? new WeakMap() : null;
    try {
        Object.defineProperty(scope, "__maplibreLiveGpuBufferBytes", {
            get: () => liveGpuBufferBytes,
            configurable: true,
        });
    } catch {
        // Ignore non-extensible global objects.
    }
    const devProto = scope?.GPUDevice?.prototype;
    if (devProto && typeof devProto.createBuffer === "function") {
        const origCreateBuffer = devProto.createBuffer;
        devProto.createBuffer = function (descriptor) {
            const bytes = Math.max(0, Number(descriptor?.size ?? 0));
            const buf = origCreateBuffer.call(this, descriptor);
            if (buf && bytes > 0) {
                liveGpuBufferBytes += bytes;
                let released = false;
                const release = () => {
                    if (!released) {
                        released = true;
                        liveGpuBufferBytes = Math.max(0, liveGpuBufferBytes - bytes);
                    }
                };
                if (deviceReleasers && this && typeof this === "object") {
                    let set = deviceReleasers.get(this);
                    if (!set) {
                        set = new Set();
                        deviceReleasers.set(this, set);
                    }
                    set.add(release);
                }
                if (typeof buf.destroy === "function") {
                    const origDestroy = buf.destroy;
                    const dev = this;
                    buf.destroy = function () {
                        release();
                        if (deviceReleasers && dev && typeof dev === "object") {
                            deviceReleasers.get(dev)?.delete(release);
                        }
                        return origDestroy.call(this);
                    };
                }
            }
            return buf;
        };
        if (deviceReleasers && typeof devProto.destroy === "function") {
            const origDevDestroy = devProto.destroy;
            devProto.destroy = function () {
                const set = deviceReleasers.get(this);
                if (set) {
                    for (const release of set) {
                        release();
                    }
                    set.clear();
                }
                return origDevDestroy.call(this);
            };
        }
    }
};

export const getLiveGpuBufferBytes = (scope = globalThis) =>
    Math.max(0, Number(scope?.__maplibreLiveGpuBufferBytes ?? 0));
