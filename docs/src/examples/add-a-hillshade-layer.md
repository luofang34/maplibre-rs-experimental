# Add a Hillshade Layer

Add a simple hillshade layer.

<div class="example-map">
  <iframe src="./demo/index.html?example=../code/add-a-hillshade-layer.js" title="Add a Hillshade Layer" loading="lazy" allow="fullscreen"></iframe>
</div>

<div class="example-code-tabs">

```rust,no_run
{{#include code/add-a-hillshade-layer.rs}}
```

```js
{{#include code/add-a-hillshade-layer.js}}
```

```html
{{#include code/add-a-hillshade-layer.html}}
```

</div>

> **Note:** The style has no background layer. GL JS leaves its canvas transparent, so the white page shows through the unshaded slopes; maplibre-rs draws them on black.
