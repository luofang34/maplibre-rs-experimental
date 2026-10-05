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

/// An `icon-image` for the first route of a feature of the OpenMapTiles `transportation_name`
/// layer; see [`openmaptiles_route_shield_image`].
pub fn openmaptiles_shield_image() -> Value {
    openmaptiles_route_shield_image(1)
}

/// An `icon-image` for route `n` (1 to 6) of a feature of the OpenMapTiles
/// `transportation_name` layer, so that a road carrying several routes shows each in a layer
/// of its own, offset from the others.
///
/// A full OpenStreetMap route relation names the route: `route_n` as `network=ref`, or
/// `route_n_network` with `route_n_ref`, as newer schemas split it. Without one, the first
/// route falls back to the `network` class and `ref`: `us-interstate` is `US:I` and
/// `us-highway` is `US:US`, while `us-state` and every other class name no state or county,
/// so the route gets roadshield's generic shield. A feature with no such route draws no image.
pub fn openmaptiles_route_shield_image(n: u8) -> Value {
    let joined = format!("route_{n}");
    let (network, reference) = (format!("route_{n}_network"), format!("route_{n}_ref"));
    let mut cases = vec![
        json!("case"),
        json!(["has", joined]),
        json!(["to-string", ["get", joined]]),
        json!(["all", ["has", network], ["has", reference]]),
        json!([
            "concat",
            ["get", network],
            "=",
            ["to-string", ["get", reference]]
        ]),
    ];
    if n == 1 {
        let class = json!([
            "match",
            ["get", "network"],
            "us-interstate",
            "US:I",
            "us-highway",
            "US:US",
            ""
        ]);
        cases.extend([
            json!(["has", "ref"]),
            json!(["concat", class, "=", ["to-string", ["get", "ref"]]]),
        ]);
    }
    cases.push(json!(""));
    let route = Value::Array(cases);
    json!([
        "case",
        ["==", route.clone(), ""],
        "",
        route_shield_image(route, json!(""))
    ])
}

#[cfg(test)]
mod tests;
