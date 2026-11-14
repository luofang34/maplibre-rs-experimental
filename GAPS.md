# Gaps

What `maplibre-rs` lacks or only partly has, measured against the MapLibre Style Specification and
against [mln-rs](https://github.com/louwers/mln-rs), the Rust port of MapLibre Native. Delete an
entry when it is closed.

Sizes: **S** days, **M** weeks, **L** a month or more. Paths are relative to `maplibre/src` unless
they start with another directory.

## Architecture and engineering

| Gap | State | Size |
| --- | --- | --- |
| Crate layering | One crate holds the engine, with 34 `pub mod` | L |
