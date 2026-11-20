/**
 * Constructs Rustflags for wasm32 builds.
 *
 * WebGL2 release builds use `-Zfmt-debug=shallow` for the main crate because Naga's GLSL writer
 * (`naga::back::glsl::writer`) formats float literals and `ShaderStage` via `Debug::fmt` (`{:?}`).
 * `-Zfmt-debug=none` strips primitive/enum `Debug` formatting, emitting invalid GLSL declarations
 * such as `const float PROJECTION_PI = ;` that trap during WebGL2 initialization.
 */
export function releaseRustFlags({release, webgl = false, worker = false}) {
    if (!release) {
        return [];
    }
    const fmtDebug = webgl && !worker ? 'shallow' : 'none';
    return [
        '-Zunstable-options',
        '-Cpanic=immediate-abort',
        '-Zlocation-detail=none',
        `-Zfmt-debug=${fmtDebug}`,
    ];
}

export function wasmTargetConfig({release, webgl = false, multithreaded = false, worker = false}) {
    const flags = releaseRustFlags({release, webgl, worker})
        .map((flag) => `"${flag}",`)
        .join('\n        ');
    const flagBlock = flags ? `${flags}\n        ` : '';
    if (multithreaded) {
        return `target.wasm32-unknown-unknown.rustflags = [
        "--cfg=web_sys_unstable_apis",
        ${flagBlock}"-C", "target-feature=+atomics,+bulk-memory,+mutable-globals,+nontrapping-fptoint,+sign-ext",
        "-C", "link-args=--shared-memory --import-memory --max-memory=1073741824 -zstack-size=131072",
        "-C", "link-args=--export=__heap_base --export=__wasm_init_tls --export=__tls_size --export=__tls_align --export=__tls_base"
    ]`;
    }
    return `target.wasm32-unknown-unknown.rustflags = [
        "--cfg=web_sys_unstable_apis",
        ${flagBlock}"-C", "target-feature=+bulk-memory,+mutable-globals,+nontrapping-fptoint,+sign-ext",
        "-C", "link-arg=-zstack-size=262144",
    ]`;
}
