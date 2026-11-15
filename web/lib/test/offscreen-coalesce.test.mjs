import test from "node:test";
import assert from "node:assert/strict";
import {createOffscreenCoalescer, mergeCameraOptions} from "../src/offscreen-coalesce.mjs";

test("mergeCameraOptions preserves earlier defined fields and overwrites with latest", () => {
    const merged = mergeCameraOptions(
        {center: [10, 20], zoom: 12, pitch: 45},
        {zoom: 13.5, bearing: 90, pitch: undefined},
    );
    assert.deepEqual(merged, {center: [10, 20], zoom: 13.5, pitch: 45, bearing: 90});
});

test("high-rate pointermove and wheel events coalesce into a single flush", async () => {
    const posted = [];
    const coalescer = createOffscreenCoalescer(
        msg => posted.push(msg),
        async () => undefined,
        () => false,
    );

    coalescer.queuePointerMove(10, 20);
    coalescer.queuePointerMove(30, 40);
    coalescer.queuePointerMove(50, 60);
    coalescer.queueWheel({x: 50, y: 60, dx: 1, dy: -10, mode: 0, ctrl: false, shift: false, alt: false});
    coalescer.queueWheel({x: 52, y: 62, dx: 2, dy: -15, mode: 0, ctrl: false, shift: false, alt: false});

    assert.equal(posted.length, 0);
    await new Promise(resolve => queueMicrotask(resolve));

    assert.deepEqual(posted, [
        {type: "pmove", x: 50, y: 60},
        {type: "pmove", x: 52, y: 62},
        {type: "wheel", dx: 3, dy: -25, mode: 0, ctrl: false, shift: false, alt: false},
    ]);
});

test("jumpTo coalesces same-turn and in-flight bursts while settling all callers", async () => {
    const rpcCalls = [];
    const pendingResolvers = [];
    const coalescer = createOffscreenCoalescer(
        () => {},
        (method, args) =>
            new Promise(resolve => {
                rpcCalls.push({method, args});
                pendingResolvers.push(resolve);
            }),
        () => false,
    );

    const p1 = coalescer.jumpTo({center: [1, 2], zoom: 10});
    const p2 = coalescer.jumpTo({zoom: 11});
    await new Promise(resolve => queueMicrotask(resolve));

    assert.equal(rpcCalls.length, 1);
    assert.deepEqual(rpcCalls[0], {
        method: "jumpTo",
        args: [{center: [1, 2], zoom: 11}],
    });

    // While the first jumpTo is still in flight on a >16ms worker frame, queue 3 more:
    const p3 = coalescer.jumpTo({bearing: 10});
    const p4 = coalescer.jumpTo({bearing: 20, pitch: 35});
    const p5 = coalescer.jumpTo({bearing: 30});
    await new Promise(resolve => queueMicrotask(resolve));
    assert.equal(rpcCalls.length, 1, "does not flood worker while jumpTo is in flight");

    // Complete first RPC -> immediately dispatches the single merged follow-up RPC.
    pendingResolvers.shift()(undefined);
    await Promise.all([p1, p2]);
    assert.equal(rpcCalls.length, 2);
    assert.deepEqual(rpcCalls[1], {
        method: "jumpTo",
        args: [{bearing: 30, pitch: 35}],
    });

    pendingResolvers.shift()(undefined);
    await Promise.all([p3, p4, p5]);
});

test("flushBeforeOrderedRpc dispatches pending jumpTo before getCamera", () => {
    const rpcCalls = [];
    const coalescer = createOffscreenCoalescer(
        () => {},
        (method, args) => {
            rpcCalls.push({method, args});
            return Promise.resolve(undefined);
        },
        () => false,
    );

    void coalescer.jumpTo({zoom: 14});
    coalescer.flushBeforeOrderedRpc();
    rpcCalls.push({method: "getCamera", args: []});

    assert.equal(rpcCalls[0].method, "jumpTo");
    assert.equal(rpcCalls[1].method, "getCamera");
});
