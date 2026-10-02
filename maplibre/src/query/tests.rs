use super::*;
use crate::style::expression::{FeatureProperties, FEATURE_STATE_PREFIX};

#[test]
fn feature_state_is_not_reported_as_a_property() {
    let properties: FeatureProperties = [
        ("name".to_owned(), Value::String("Bucks".into())),
        (format!("{FEATURE_STATE_PREFIX}big"), Value::Bool(true)),
    ]
    .into_iter()
    .collect();

    let reported = source_properties(&properties);

    assert_eq!(
        reported.into_iter().collect::<Vec<_>>(),
        [("name".to_owned(), serde_json::json!("Bucks"))]
    );
}
