use super::*;

#[test]
fn a_route_reads_back_from_its_id() {
    for (id, network, reference, name) in [
        ("US:I=287", "US:I", "287", None),
        ("US:NJ:CR=609", "US:NJ:CR", "609", None),
        ("=609", "", "609", None),
        ("US:US:Truck:Bypass=1", "US:US:Truck:Bypass", "1", None),
        ("AM=Մ4", "AM", "Մ4", None),
        (
            "US:KY:Parkway=\u{1f}Audubon Parkway",
            "US:KY:Parkway",
            "",
            Some("Audubon Parkway"),
        ),
        ("CA:ON=401=A", "CA:ON", "401=A", None),
    ] {
        let route = RouteRequest::parse(id).expect(id);
        assert_eq!(route.network, network, "{id}");
        assert_eq!(route.reference, reference, "{id}");
        assert_eq!(route.name.as_deref(), name, "{id}");
        assert_eq!(route.id(), id);
        assert_eq!(route.image_name(), format!("roadshield:{id}"));
    }
}

#[test]
fn an_id_without_a_route_is_refused() {
    for id in ["", "US:I", "US:I=", "US:I=\u{1f}"] {
        assert!(RouteRequest::parse(id).is_err(), "{id:?}");
    }
}

#[test]
fn a_route_of_unknown_network_is_drawn_generic_not_guessed() {
    let route = RouteRequest::parse("=609").expect("route");
    let descriptor = route.descriptor();
    assert_eq!(descriptor.network.as_deref(), Some(UNKNOWN_NETWORK));
    assert_eq!(descriptor.ref_.as_deref(), Some("609"));
    assert_eq!(descriptor.source.as_deref(), Some("roadshield:=609"));
}
