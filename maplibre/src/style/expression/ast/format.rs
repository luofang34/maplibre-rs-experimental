//! A section of a `format` expression.

use super::Expression;

/// One section of a `format` expression: its content and the options that style it.
#[derive(Clone, Debug, PartialEq)]
pub struct FormatSection {
    /// Text, or an image when it evaluates to one.
    pub content: Expression,
    /// `font-scale`.
    pub scale: Option<Expression>,
    /// `text-font`.
    pub font: Option<Expression>,
    /// `text-color`.
    pub color: Option<Expression>,
    /// `vertical-align`.
    pub vertical_align: Option<Expression>,
}

impl FormatSection {
    /// Calls `visit` on the content and every option.
    pub(super) fn for_each<'a>(&'a self, visit: &mut dyn FnMut(&'a Expression)) {
        visit(&self.content);
        for option in [&self.scale, &self.font, &self.color, &self.vertical_align]
            .into_iter()
            .flatten()
        {
            visit(option);
        }
    }
}
