export const computeMtTargetWorkerCount = (
    cores = Number(globalThis.navigator?.hardwareConcurrency) || 4,
) => Math.min(4, Math.max(1, cores - 1));

export const computeMtEagerWorkerCount = (target = computeMtTargetWorkerCount()) =>
    Math.min(4, target);

/**
 * Manages lazy activation and cleanup of dormant secondary worker MessagePorts so multithreaded
 * boot only spawns up to `eagerCount` workers immediately while keeping remaining ports ready.
 */
export const createDormantPortSlotController = ({
    totalCount,
    eagerCount,
    spawnWorkerForSlot,
}) => {
    const dormantPorts = new Array(totalCount).fill(null);
    return {
        setDormant(slot, port) {
            if (slot >= eagerCount && slot < totalCount) {
                dormantPorts[slot] = port;
            }
        },
        activate(slot) {
            const port = dormantPorts[slot] ?? null;
            if (!port) return false;
            dormantPorts[slot] = null;
            spawnWorkerForSlot(slot, port);
            return true;
        },
        closeAll() {
            for (let i = 0; i < dormantPorts.length; i++) {
                dormantPorts[i]?.close?.();
                dormantPorts[i] = null;
            }
        },
        remainingCount() {
            return dormantPorts.filter(Boolean).length;
        },
    };
};
