//! Direct JS `postMessage` bindings for single-threaded tile worker control messages.

#![deny(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

#[cfg(target_arch = "wasm32")]
mod imp {
    use wasm_bindgen::{prelude::wasm_bindgen, JsValue};

    #[wasm_bindgen(inline_js = r#"
export function post_st_tile_call(
    target,
    attempt,
    style_revision,
    x,
    y,
    meta,
    pixel_ratio
) {
    target.postMessage({
        type: "call",
        attempt,
        style_revision,
        x,
        y,
        meta,
        pixel_ratio
    });
}

export function post_st_tile_call_batch(target, words) {
    const copy = words.slice();
    target.postMessage({ type: "call_batch", words: copy }, [copy.buffer]);
}

export function post_st_cancel(target, attempt) {
    target.postMessage({ type: "cancel", attempt });
}

export function post_st_cancel_batch(target, attempts) {
    const copy = attempts.slice();
    target.postMessage({ type: "cancel_batch", attempts: copy }, [copy.buffer]);
}

export function post_st_style(target, revision, style, prefetch) {
    target.postMessage({ type: "style", revision, style, prefetch });
}
"#)]
    extern "C" {
        #[wasm_bindgen(catch)]
        pub(crate) fn post_st_tile_call(
            target: &JsValue,
            attempt: u64,
            style_revision: u64,
            x: i32,
            y: i32,
            meta: u32,
            pixel_ratio: f32,
        ) -> Result<(), JsValue>;

        #[wasm_bindgen(catch)]
        pub(crate) fn post_st_tile_call_batch(
            target: &JsValue,
            words: &[u32],
        ) -> Result<(), JsValue>;

        #[wasm_bindgen(catch)]
        pub(crate) fn post_st_cancel(target: &JsValue, attempt: u64) -> Result<(), JsValue>;

        #[wasm_bindgen(catch)]
        pub(crate) fn post_st_cancel_batch(
            target: &JsValue,
            attempts: &[u64],
        ) -> Result<(), JsValue>;

        #[wasm_bindgen(catch)]
        pub(crate) fn post_st_style(
            target: &JsValue,
            revision: u64,
            style: &str,
            prefetch: bool,
        ) -> Result<(), JsValue>;
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    use wasm_bindgen::JsValue;

    #[expect(
        clippy::too_many_arguments,
        reason = "host test stub matches wasm32 post_st_tile_call ABI"
    )]
    pub(crate) fn post_st_tile_call(
        _target: &JsValue,
        _attempt: u64,
        _style_revision: u64,
        _x: i32,
        _y: i32,
        _meta: u32,
        _pixel_ratio: f32,
    ) -> Result<(), JsValue> {
        Ok(())
    }

    pub(crate) fn post_st_tile_call_batch(
        _target: &JsValue,
        _words: &[u32],
    ) -> Result<(), JsValue> {
        Ok(())
    }

    pub(crate) fn post_st_cancel(_target: &JsValue, _attempt: u64) -> Result<(), JsValue> {
        Ok(())
    }

    pub(crate) fn post_st_cancel_batch(
        _target: &JsValue,
        _attempts: &[u64],
    ) -> Result<(), JsValue> {
        Ok(())
    }

    pub(crate) fn post_st_style(
        _target: &JsValue,
        _revision: u64,
        _style: &str,
        _prefetch: bool,
    ) -> Result<(), JsValue> {
        Ok(())
    }
}

pub(super) use imp::{
    post_st_cancel, post_st_cancel_batch, post_st_style, post_st_tile_call, post_st_tile_call_batch,
};
