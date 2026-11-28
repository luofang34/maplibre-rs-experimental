import assert from "node:assert/strict";
import test from "node:test";
import {
    computeMtEagerWorkerCount,
    computeMtTargetWorkerCount,
    createDormantPortSlotController,
} from "../src/mt-pool-slots.mjs";

test("MT target and eager worker counts scale cleanly within caps", () => {
    assert.equal(computeMtTargetWorkerCount(1), 1);
    assert.equal(computeMtTargetWorkerCount(2), 1);
    assert.equal(computeMtTargetWorkerCount(4), 3);
    assert.equal(computeMtTargetWorkerCount(8), 4);
    assert.equal(computeMtTargetWorkerCount(16), 4);

    assert.equal(computeMtEagerWorkerCount(1), 1);
    assert.equal(computeMtEagerWorkerCount(2), 2);
    assert.equal(computeMtEagerWorkerCount(3), 3);
    assert.equal(computeMtEagerWorkerCount(4), 4);
});

test("dormant port controller activates each slot once and closes remaining ports on teardown", () => {
    const activated = [];
    const closedPorts = [];
    const makePort = id => ({id, close: () => closedPorts.push(id)});

    const ctrl = createDormantPortSlotController({
        totalCount: 4,
        eagerCount: 2,
        spawnWorkerForSlot: (slot, port) => activated.push([slot, port.id]),
    });

    ctrl.setDormant(0, makePort("p0"));
    ctrl.setDormant(1, makePort("p1"));
    ctrl.setDormant(2, makePort("p2"));
    ctrl.setDormant(3, makePort("p3"));
    assert.equal(ctrl.remainingCount(), 2);

    assert.equal(ctrl.activate(0), false);
    assert.equal(ctrl.activate(2), true);
    assert.equal(ctrl.activate(2), false);
    assert.deepEqual(activated, [[2, "p2"]]);
    assert.equal(ctrl.remainingCount(), 1);

    ctrl.closeAll();
    assert.equal(ctrl.remainingCount(), 0);
    assert.deepEqual(closedPorts, ["p3"]);
});
