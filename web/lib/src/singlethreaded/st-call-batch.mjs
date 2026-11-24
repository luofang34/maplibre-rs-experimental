export const WORDS_PER_CALL = 8;

/**
 * Decodes the unsigned 64-bit `attempt` from two little-endian `u32` words.
 * @param {Uint32Array} words
 * @param {number} wordOffset
 * @returns {bigint}
 */
export function decodeBatchAttempt(words, wordOffset) {
    return BigInt(words[wordOffset] >>> 0) | (BigInt(words[wordOffset + 1] >>> 0) << 32n);
}

/**
 * Registers all batched attempts in `waiting` synchronously before any `await`, preserving FIFO
 * cancellation semantics with scalar `cancel` and `cancel_batch` messages.
 * @param {Uint32Array} words
 * @param {Map<bigint, { cancelled: boolean }>} waiting
 * @returns {Array<{ idx: number, attempt: bigint, entry: { cancelled: boolean } }>}
 */
export function registerBatchCalls(words, waiting) {
    /** @type {Array<{ idx: number, attempt: bigint, entry: { cancelled: boolean } }>} */
    const registered = [];
    for (let i = 0; i + WORDS_PER_CALL <= words.length; i += WORDS_PER_CALL) {
        const attempt = decodeBatchAttempt(words, i);
        const entry = {cancelled: false};
        waiting.set(attempt, entry);
        registered.push({idx: i, attempt, entry});
    }
    return registered;
}

/**
 * Deletes settled entries from `waiting`, separates cancelled attempt IDs, and compacts live calls.
 * @param {Uint32Array} words
 * @param {Array<{ idx: number, attempt: bigint, entry: { cancelled: boolean } }>} registered
 * @param {Map<bigint, { cancelled: boolean }>} waiting
 * @returns {{ words: Uint32Array | null, cancelledAttempts: bigint[] }}
 */
export function partitionBatchWords(words, registered, waiting) {
    /** @type {bigint[]} */
    const cancelledAttempts = [];
    for (const item of registered) {
        waiting.delete(item.attempt);
        if (item.entry.cancelled) {
            cancelledAttempts.push(item.attempt);
        }
    }
    if (cancelledAttempts.length === 0) {
        return {words, cancelledAttempts};
    }
    const keptCount = registered.length - cancelledAttempts.length;
    if (keptCount === 0) {
        return {words: null, cancelledAttempts};
    }
    const compact = new Uint32Array(keptCount * WORDS_PER_CALL);
    let dst = 0;
    for (const item of registered) {
        if (item.entry.cancelled) continue;
        for (let k = 0; k < WORDS_PER_CALL; k++) {
            compact[dst + k] = words[item.idx + k];
        }
        dst += WORDS_PER_CALL;
    }
    return {words: compact, cancelledAttempts};
}

/**
 * Deletes settled entries from `waiting` and compacts out any calls cancelled while queued.
 * @param {Uint32Array} words
 * @param {Array<{ idx: number, attempt: bigint, entry: { cancelled: boolean } }>} registered
 * @param {Map<bigint, { cancelled: boolean }>} waiting
 * @returns {Uint32Array | null}
 */
export function filterBatchWords(words, registered, waiting) {
    return partitionBatchWords(words, registered, waiting).words;
}
