# Get Features Under the Mouse Pointer

Show the features under the pointer with `queryRenderedFeatures`. The page's single-threaded build reports labels and icons, since its tile workers send no index of fill and line geometry.

<div class="example-map">
  <iframe src="./demo/index.html?example=../code/get-features-under-the-mouse-pointer.js" title="Get Features Under the Mouse Pointer" loading="lazy" allow="fullscreen"></iframe>
</div>

<div class="example-code-tabs">

```rust,no_run
{{#include code/get-features-under-the-mouse-pointer.rs}}
```

```js
{{#include code/get-features-under-the-mouse-pointer.js}}
```

```html
{{#include code/get-features-under-the-mouse-pointer.html}}
```

</div>
