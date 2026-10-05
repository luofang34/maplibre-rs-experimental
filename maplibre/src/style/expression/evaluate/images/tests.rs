#![allow(clippy::expect_used, clippy::panic)]

use std::collections::HashMap;

use crate::style::expression::{
    EvaluationContext, Expression, FeatureProperties, ResolvedImage, Type, Value,
};

/// Evaluates `json` for a road carrying an autobahn and a European route, with `available`
/// the images the map holds.
fn evaluate(json: serde_json::Value, available: &[&str]) -> Value {
    let images: HashMap<String, ()> = available
        .iter()
        .map(|name| ((*name).to_owned(), ()))
        .collect();
    let properties = FeatureProperties::from([
        ("route_1_network".to_owned(), Value::from("BAB")),
        ("route_1_ref".to_owned(), Value::from("A 115")),
        ("route_2_network".to_owned(), Value::from("e-road")),
        ("route_2_ref".to_owned(), Value::from("E 51")),
    ]);
    let expression = Expression::parse(&json).expect("parses");
    expression
        .evaluate(&EvaluationContext {
            available_images: Some(&images),
            ..EvaluationContext::for_feature(14.0, &properties)
        })
        .expect("evaluates")
}

fn route(n: u8) -> serde_json::Value {
    serde_json::json!([
        "case",
        ["has", format!("route_{n}_network")],
        [
            "image",
            [
                "concat",
                "shield\n",
                ["get", format!("route_{n}_network")],
                "\n",
                ["get", format!("route_{n}_ref")]
            ]
        ],
        ["literal", ""]
    ])
}

#[test]
fn an_image_a_case_chooses_inside_format_stays_an_image() {
    let Value::Formatted(formatted) = evaluate(
        serde_json::json!(["format", route(1), route(2), route(3)]),
        &["shield\nBAB\nA 115"],
    ) else {
        panic!("formatted text");
    };
    let images: Vec<_> = formatted
        .sections
        .iter()
        .map(|section| section.image.clone())
        .collect();
    assert_eq!(
        images,
        [
            Some(ResolvedImage {
                name: "shield\nBAB\nA 115".to_owned(),
                available: true
            }),
            Some(ResolvedImage {
                name: "shield\ne-road\nE 51".to_owned(),
                available: false
            }),
            None,
        ]
    );
    assert_eq!(formatted.text(), "", "no request name is shown as text");
}

#[test]
fn coalesce_skips_an_image_the_map_lacks_and_names_the_first_when_all_are_missing() {
    let coalesce =
        || serde_json::json!(["coalesce", ["image", "missing"], ["image", "also-missing"]]);
    assert_eq!(
        evaluate(
            serde_json::json!(["coalesce", ["image", "missing"], ["image", "here"]]),
            &["here"]
        ),
        Value::Image(ResolvedImage {
            name: "here".to_owned(),
            available: true
        })
    );
    assert_eq!(
        evaluate(coalesce(), &[]),
        Value::String("missing".to_owned())
    );
    assert_eq!(
        evaluate(
            serde_json::json!(["coalesce", ["image", "missing"], "fallback"]),
            &[]
        ),
        Value::String("fallback".to_owned())
    );
    let Value::Formatted(formatted) = evaluate(
        serde_json::json!([
            "format",
            ["coalesce", ["image", "missing"], ["image", "here"]]
        ]),
        &["here"],
    ) else {
        panic!("formatted text");
    };
    assert_eq!(
        formatted.sections[0]
            .image
            .as_ref()
            .map(|image| image.name.as_str()),
        Some("here")
    );
}

#[test]
fn let_and_match_carry_an_image_into_format() {
    let Value::Formatted(formatted) = evaluate(
        serde_json::json!(["let", "shield", ["image", "shield\nBAB\nA 115"],
            ["format",
                ["match", ["get", "route_1_network"], "BAB", ["var", "shield"], ""],
                {"font-scale": 2}]]),
        &[],
    ) else {
        panic!("formatted text");
    };
    let section = &formatted.sections[0];
    assert_eq!(
        section.image.as_ref().map(|image| image.name.as_str()),
        Some("shield\nBAB\nA 115")
    );
    assert_eq!(
        section.scale, None,
        "an image section takes no text options"
    );
}

#[test]
fn images_and_formatted_text_read_as_their_names_and_text() {
    let image = evaluate(serde_json::json!(["to-string", ["image", "marker"]]), &[]);
    assert_eq!(image, Value::String("marker".to_owned()));
    let text = evaluate(
        serde_json::json!(["to-string", ["format", "a", {}, ["image", "x"], "b"]]),
        &[],
    );
    assert_eq!(text, Value::String("ab".to_owned()));
    assert_eq!(
        evaluate(serde_json::json!(["typeof", ["image", "marker"]]), &[]),
        Value::String("resolvedImage".to_owned())
    );
    assert_eq!(
        Expression::parse(&serde_json::json!(["format", "a"]))
            .expect("parses")
            .output_type(),
        Type::Formatted
    );
    assert!(
        Expression::parse(&serde_json::json!(["format", 1])).is_err(),
        "a number is no section"
    );
}
