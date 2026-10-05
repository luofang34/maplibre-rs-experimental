//! `["format", ...]`: text in sections that differ in size, colour and font, any of which may
//! be an image.
//!
//! The control characters below are the form a [`crate::style::property::TextField`] with
//! sections is written in, so a layer serialized and parsed again keeps its sections.

use serde_json::Value as Json;

use super::{Parser, Result};
use crate::style::expression::{
    ast::{Expression, FormatSection},
    value::Type,
};

/// Starts the written form of a formatted text.
pub const FORMATTED_START: char = '\u{1}';
/// Starts each section.
pub const SECTION: char = '\u{2}';
/// Separates the fields of a section: font scale, colour, font stack, text, image name.
pub const FIELD: char = '\u{3}';
/// The character that stands for an image in the text of a section.
pub const IMAGE_PLACEHOLDER: char = '\u{E000}';

const VERTICAL_ALIGNMENTS: [&str; 3] = ["bottom", "center", "top"];

impl Parser {
    /// Parses the arguments of a `format` expression as GL JS does: each content, typed as any
    /// value, may be followed by an object of options for its section.
    pub(super) fn parse_format(&mut self, args: &[Json]) -> Result<Expression> {
        match args.first() {
            None => return Err(self.error("Expected at least one argument.")),
            Some(Json::Object(_)) => {
                return Err(self.error("First argument must be an image or text section."))
            }
            Some(_) => {}
        }
        let mut sections: Vec<FormatSection> = Vec::new();
        let mut options_may_follow = false;
        for (offset, arg) in args.iter().enumerate() {
            let index = offset + 1;
            match (arg, sections.last_mut()) {
                (Json::Object(options), Some(_)) if options_may_follow => {
                    options_may_follow = false;
                    let styled = self.parse_section_options(options, index)?;
                    if let Some(section) = sections.last_mut() {
                        section.scale = styled.scale;
                        section.font = styled.font;
                        section.color = styled.color;
                        section.vertical_align = styled.vertical_align;
                    }
                }
                _ => {
                    let content = self.parse_at(arg, index, Some(&Type::Value))?;
                    let kind = content.output_type();
                    if !matches!(
                        kind,
                        Type::String | Type::Value | Type::Null | Type::ResolvedImage
                    ) {
                        return Err(self.error(
                            "Formatted text type must be 'string', 'value', 'image' or 'null'.",
                        ));
                    }
                    options_may_follow = true;
                    sections.push(FormatSection {
                        content,
                        scale: None,
                        font: None,
                        color: None,
                        vertical_align: None,
                    });
                }
            }
        }
        Ok(Expression::Format(sections))
    }

    /// The options object of a section: what each styles, parsed into an otherwise empty
    /// section.
    fn parse_section_options(
        &mut self,
        options: &serde_json::Map<String, Json>,
        index: usize,
    ) -> Result<FormatSection> {
        let option = |parser: &mut Self, name: &str, expected: Type| {
            options
                .get(name)
                .map(|value| parser.parse_at(value, index, Some(&expected)))
                .transpose()
        };
        if let Some(Json::String(align)) = options.get("vertical-align") {
            if !VERTICAL_ALIGNMENTS.contains(&align.as_str()) {
                return Err(self.error(format!(
                    "'vertical-align' must be one of: 'bottom', 'center', 'top' but found \
                     '{align}' instead."
                )));
            }
        }
        Ok(FormatSection {
            content: Expression::Literal(crate::style::expression::Value::Null),
            scale: option(self, "font-scale", Type::Number)?,
            font: option(self, "text-font", Type::array(Type::String, None))?,
            color: option(self, "text-color", Type::Color)?,
            vertical_align: option(self, "vertical-align", Type::String)?,
        })
    }
}
