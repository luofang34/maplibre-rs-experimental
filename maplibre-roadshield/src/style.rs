//! Style expressions that name a route's shield.

use serde_json::{json, Value};

use crate::route::NAMESPACE;

/// An `icon-image` naming the shield of the route `route` evaluates to, as `network=ref`, and
/// falling back to `fallback` when no shield is drawn for it.
pub fn route_shield_image(route: Value, fallback: Value) -> Value {
    json!([
        "coalesce",
        ["image", ["concat", format!("{NAMESPACE}:"), route]],
        fallback
    ])
}

/// An `icon-image` for the OpenMapTiles `transportation_name` layer.
///
/// A feature with `route_1` (a full OpenStreetMap route relation, `network=ref`) names that
/// route. Otherwise its `network` class is read: `us-interstate` is `US:I` and `us-highway` is
/// `US:US`. Every other class, `us-state` included, names neither state nor county, so the
/// route is drawn with roadshield's generic shield. A feature with neither draws no image.
pub fn openmaptiles_shield_image() -> Value {
    let network = json!([
        "match",
        ["get", "network"],
        "us-interstate",
        "US:I",
        "us-highway",
        "US:US",
        ""
    ]);
    let route = json!([
        "case",
        ["has", "route_1"],
        ["to-string", ["get", "route_1"]],
        ["concat", network, "=", ["to-string", ["get", "ref"]]]
    ]);
    json!([
        "case",
        ["any", ["has", "route_1"], ["has", "ref"]],
        route_shield_image(route, json!("")),
        ""
    ])
}
