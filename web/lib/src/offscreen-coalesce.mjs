const utf8Encoder = typeof TextEncoder !== "undefined" ? new TextEncoder() : null;

const isPlainCameraObject = val =>
    typeof val === "object" && val !== null && !Array.isArray(val);

export const mergeCameraOptions = (base, next) => {
    if (!isPlainCameraObject(base) || !isPlainCameraObject(next)) {
        return next;
    }
    const merged = {...base};
    for (const [k, v] of Object.entries(next)) {
        if (v !== undefined && v !== null) {
            merged[k] = v;
        }
    }
    return merged;
};

/**
 * Coalesces high-frequency pointer-move, wheel, touch-move, `jumpTo`, and `setGeoJsonData`
 * messages before posting to the dedicated OffscreenCanvas render worker, matching
 * `web/src/controller/coalesce.rs` latest-wins semantics.
 */
export const createOffscreenCoalescer = (post, rawRpc, isClosed) => {
    let pendingPointer = null;
    let pendingWheel = null;
    const pendingTouchMoves = new Map();
    let inputRafId = null;
    let inputMicrotaskQueued = false;

    let queuedJump = null;
    let jumpInFlight = false;
    let jumpFlushScheduled = false;
    const queuedGeoJson = new Map();
    const geoJsonInFlight = new Set();
    let geoJsonFlushScheduled = false;
    let geoJsonChannel = null;

    const flushInput = () => {
        if (inputRafId !== null && typeof cancelAnimationFrame === "function") {
            cancelAnimationFrame(inputRafId);
        }
        inputRafId = null;
        inputMicrotaskQueued = false;
        if (isClosed()) {
            pendingPointer = null;
            pendingWheel = null;
            pendingTouchMoves.clear();
            return;
        }
        if (pendingPointer) {
            const p = pendingPointer;
            pendingPointer = null;
            post({type: "pmove", x: p.x, y: p.y});
        }
        if (pendingWheel) {
            const w = pendingWheel;
            pendingWheel = null;
            post({type: "pmove", x: w.x, y: w.y});
            post({
                type: "wheel",
                dx: w.dx,
                dy: w.dy,
                mode: w.mode,
                ctrl: w.ctrl,
                shift: w.shift,
                alt: w.alt,
            });
        }
        if (pendingTouchMoves.size > 0) {
            for (const [id, pt] of pendingTouchMoves) {
                post({type: "touch", id, phase: 1, x: pt.x, y: pt.y});
            }
            pendingTouchMoves.clear();
        }
    };

    const scheduleInputFlush = () => {
        if (inputRafId !== null || inputMicrotaskQueued) return;
        const hidden = typeof document !== "undefined" && document.hidden;
        if (!hidden && typeof requestAnimationFrame === "function") {
            inputRafId = requestAnimationFrame(() => {
                inputRafId = null;
                flushInput();
            });
        } else {
            inputMicrotaskQueued = true;
            queueMicrotask(flushInput);
        }
    };

    const queuePointerMove = (x, y) => {
        if (isClosed()) return;
        pendingPointer = {x, y};
        scheduleInputFlush();
    };

    const queueWheel = w => {
        if (isClosed()) return;
        if (
            pendingWheel &&
            pendingWheel.mode === w.mode &&
            pendingWheel.ctrl === w.ctrl &&
            pendingWheel.shift === w.shift &&
            pendingWheel.alt === w.alt
        ) {
            pendingWheel.x = w.x;
            pendingWheel.y = w.y;
            pendingWheel.dx += w.dx;
            pendingWheel.dy += w.dy;
        } else {
            if (pendingWheel) flushInput();
            pendingWheel = {...w};
        }
        scheduleInputFlush();
    };

    const queueTouchMove = (id, x, y) => {
        if (isClosed()) return;
        pendingTouchMoves.set(id, {x, y});
        scheduleInputFlush();
    };

    const dispatchQueuedJump = () => {
        jumpFlushScheduled = false;
        if (isClosed() || jumpInFlight || !queuedJump) return;
        const batch = queuedJump;
        queuedJump = null;
        jumpInFlight = true;
        rawRpc("jumpTo", [batch.options]).then(
            res => {
                jumpInFlight = false;
                for (const w of batch.waiters) w.resolve(res);
                if (queuedJump) dispatchQueuedJump();
            },
            err => {
                jumpInFlight = false;
                for (const w of batch.waiters) w.reject(err);
                if (queuedJump) dispatchQueuedJump();
            },
        );
    };

    const jumpTo = options => {
        if (isClosed()) return Promise.resolve(undefined);
        return new Promise((resolve, reject) => {
            if (queuedJump) {
                queuedJump.options = mergeCameraOptions(queuedJump.options, options);
                queuedJump.waiters.push({resolve, reject});
            } else {
                queuedJump = {options, waiters: [{resolve, reject}]};
            }
            if (!jumpInFlight && !jumpFlushScheduled) {
                jumpFlushScheduled = true;
                queueMicrotask(dispatchQueuedJump);
            }
        });
    };

    const dispatchQueuedGeoJson = source => {
        if (isClosed() || geoJsonInFlight.has(source)) return;
        const batch = queuedGeoJson.get(source);
        if (!batch) return;
        queuedGeoJson.delete(source);
        geoJsonInFlight.add(source);
        let rpcPromise;
        if (utf8Encoder) {
            try {
                const wasString = typeof batch.data === "string";
                const text = wasString ? batch.data : JSON.stringify(batch.data);
                if (typeof text !== "string") {
                    throw new TypeError("GeoJSON data is not JSON-serializable");
                }
                const bytes = utf8Encoder.encode(text);
                rpcPromise = rawRpc("setGeoJsonBytes", [source, bytes, wasString], [bytes.buffer]);
            } catch (err) {
                geoJsonInFlight.delete(source);
                for (const w of batch.waiters) w.reject(err);
                if (queuedGeoJson.has(source)) dispatchQueuedGeoJson(source);
                return;
            }
        } else {
            rpcPromise = rawRpc("setGeoJsonData", [source, batch.data]);
        }
        rpcPromise.then(
            res => {
                geoJsonInFlight.delete(source);
                for (const w of batch.waiters) w.resolve(res);
                if (queuedGeoJson.has(source)) dispatchQueuedGeoJson(source);
            },
            err => {
                geoJsonInFlight.delete(source);
                for (const w of batch.waiters) w.reject(err);
                if (queuedGeoJson.has(source)) dispatchQueuedGeoJson(source);
            },
        );
    };

    const flushQueuedGeoJson = () => {
        geoJsonFlushScheduled = false;
        if (isClosed() || queuedGeoJson.size === 0) return;
        for (const source of Array.from(queuedGeoJson.keys())) {
            dispatchQueuedGeoJson(source);
        }
    };

    const scheduleGeoJsonFlush = () => {
        if (geoJsonFlushScheduled) return;
        geoJsonFlushScheduled = true;
        if (typeof MessageChannel === "function") {
            if (!geoJsonChannel) {
                geoJsonChannel = new MessageChannel();
                geoJsonChannel.port1.onmessage = flushQueuedGeoJson;
                geoJsonChannel.port1.unref?.();
            }
            geoJsonChannel.port2.postMessage(0);
        } else {
            setTimeout(flushQueuedGeoJson, 0);
        }
    };

    const setGeoJsonData = (source, data) => {
        if (isClosed()) return Promise.resolve(undefined);
        return new Promise((resolve, reject) => {
            const existing = queuedGeoJson.get(source);
            if (existing) {
                existing.data = data;
                existing.waiters.push({resolve, reject});
            } else {
                queuedGeoJson.set(source, {data, waiters: [{resolve, reject}]});
            }
            scheduleGeoJsonFlush();
        });
    };

    const flushBeforeOrderedRpc = () => {
        flushInput();
        if (queuedJump && !jumpInFlight) {
            dispatchQueuedJump();
        }
        if (queuedGeoJson.size > 0) {
            flushQueuedGeoJson();
        }
    };

    const rejectQueued = reason => {
        if (inputRafId !== null && typeof cancelAnimationFrame === "function") {
            cancelAnimationFrame(inputRafId);
        }
        inputRafId = null;
        inputMicrotaskQueued = false;
        geoJsonFlushScheduled = false;
        if (geoJsonChannel) {
            geoJsonChannel.port1.onmessage = null;
            geoJsonChannel.port1.close?.();
            geoJsonChannel.port2.close?.();
            geoJsonChannel = null;
        }
        pendingPointer = null;
        pendingWheel = null;
        pendingTouchMoves.clear();
        if (queuedJump) {
            const batch = queuedJump;
            queuedJump = null;
            for (const w of batch.waiters) w.reject(reason);
        }
        for (const batch of queuedGeoJson.values()) {
            for (const w of batch.waiters) w.reject(reason);
        }
        queuedGeoJson.clear();
    };

    return {
        flushInput,
        queuePointerMove,
        queueWheel,
        queueTouchMove,
        jumpTo,
        setGeoJsonData,
        flushBeforeOrderedRpc,
        rejectQueued,
    };
};
