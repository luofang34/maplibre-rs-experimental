//! The values of the `image` and `format` expressions: an image name with whether the map
//! holds it, and text in sections that may each be an image.

use super::Color;

/// An image an `image` expression names, and whether the map holds it now. A name the map
/// is still loading or making counts as unavailable, so `coalesce` moves on to its fallback
/// until it arrives and the label is laid out again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedImage {
    /// The image name.
    pub name: String,
    /// Whether the map holds the image.
    pub available: bool,
}

/// One run of a formatted text: text, or an image drawn in the line of text.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FormattedSection {
    /// The text; empty for an image.
    pub text: String,
    /// The image the section is, if it is one.
    pub image: Option<ResolvedImage>,
    /// Factor on the layout size.
    pub scale: Option<f64>,
    /// Font stack that replaces the layer's.
    pub font: Option<Vec<String>>,
    /// Text colour that replaces the layer's.
    pub color: Option<Color>,
    /// Vertical alignment within the line: `bottom`, `center` or `top`.
    pub vertical_align: Option<String>,
}

/// Text in sections, what `format` produces.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Formatted {
    /// The sections, in order.
    pub sections: Vec<FormattedSection>,
}

impl Formatted {
    /// One plain section of `text`.
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            sections: vec![FormattedSection {
                text: text.into(),
                ..FormattedSection::default()
            }],
        }
    }

    /// One section that is `image`.
    pub fn image(image: ResolvedImage) -> Self {
        Self {
            sections: vec![FormattedSection {
                image: Some(image),
                ..FormattedSection::default()
            }],
        }
    }

    /// The text of every section, images contributing none, as GL JS `toString` gives it.
    pub fn text(&self) -> String {
        self.sections
            .iter()
            .map(|section| section.text.as_str())
            .collect()
    }

    /// The form the style specification's conformance suite writes.
    pub(super) fn to_json(&self) -> serde_json::Value {
        let sections = self
            .sections
            .iter()
            .map(|section| {
                serde_json::json!({
                    "text": section.text,
                    "image": section.image.as_ref().map(ResolvedImage::to_json),
                    "scale": section.scale,
                    "fontStack": section.font.as_ref().map(|font| font.join(",")),
                    "textColor": section.color.map(|color| {
                        let [r, g, b, a] = color.premultiplied();
                        serde_json::json!({ "r": r, "g": g, "b": b, "a": a })
                    }),
                    "verticalAlign": section.vertical_align,
                })
            })
            .collect::<Vec<_>>();
        serde_json::json!({ "sections": sections })
    }
}

impl ResolvedImage {
    /// The form the style specification's conformance suite writes.
    pub(super) fn to_json(&self) -> serde_json::Value {
        serde_json::json!({ "name": self.name, "available": self.available })
    }
}
