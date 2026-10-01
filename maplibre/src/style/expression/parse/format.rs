//! `["format", ...]`: text in sections that differ in size, colour and font, lowered to a
//! string the text field reads back into its sections.
//!
//! Every section becomes its options and its text joined by control characters, and the whole
//! starts with one more, so ordinary expressions (`concat`, `to-string`) carry the sections
//! through an evaluation that only knows strings.

use serde_json::{json, Value as Json};

/// Starts the lowered form of a formatted text.
pub const FORMATTED_START: char = '\u{1}';
/// Starts each section.
pub const SECTION: char = '\u{2}';
/// Separates the fields of a section: font scale, colour, font stack, text, image name.
pub const FIELD: char = '\u{3}';
/// The character that stands for an image in the text of a section.
pub const IMAGE_PLACEHOLDER: char = '\u{E000}';

/// The expression a `format` with these arguments stands for, or what is wrong with them.
pub(super) fn lower(args: &[Json]) -> Result<Json, String> {
    if args.is_empty() {
        return Err("Expected at least one argument, but found none.".to_owned());
    }
    let mut parts = vec![json!("concat"), json!(FORMATTED_START.to_string())];
    let mut rest = args;
    while let Some((content, after)) = rest.split_first() {
        let image = content
            .is_array_with_operator("image")
            .then(|| content.get(1).cloned())
            .flatten();
        let (options, after) = match after.split_first() {
            Some((Json::Object(options), after)) => (Some(options), after),
            _ => (None, after),
        };
        let field = |name: &str| {
            options
                .and_then(|options| options.get(name))
                .map_or_else(|| json!(""), |value| json!(["to-string", value]))
        };
        parts.push(json!([
            "concat",
            SECTION.to_string(),
            field("font-scale"),
            FIELD.to_string(),
            field("text-color"),
            FIELD.to_string(),
            field("text-font"),
            FIELD.to_string(),
            if image.is_some() {
                json!(IMAGE_PLACEHOLDER.to_string())
            } else {
                json!(["to-string", content])
            },
            FIELD.to_string(),
            image.map_or_else(|| json!(""), |name| json!(["to-string", name])),
        ]));
        rest = after;
    }
    Ok(Json::Array(parts))
}

trait OperatorCheck {
    fn is_array_with_operator(&self, operator: &str) -> bool;
}

impl OperatorCheck for Json {
    fn is_array_with_operator(&self, operator: &str) -> bool {
        self.as_array()
            .and_then(|items| items.first())
            .and_then(Json::as_str)
            == Some(operator)
    }
}
