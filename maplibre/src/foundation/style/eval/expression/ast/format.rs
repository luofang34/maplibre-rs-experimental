//! A section of a `format` expression.

/// One section of a `format` expression: its content and the options that style it.
#[derive(Clone, Debug, PartialEq)]
pub struct FormatSectionNode<E> {
    /// Text, or an image when it evaluates to one.
    pub content: E,
    /// `font-scale`.
    pub scale: Option<E>,
    /// `text-font`.
    pub font: Option<E>,
    /// `text-color`.
    pub color: Option<E>,
    /// `vertical-align`.
    pub vertical_align: Option<E>,
}

impl<E> FormatSectionNode<E> {
    /// Calls `visit` on the content and every option.
    pub(super) fn for_each<'a>(&'a self, visit: &mut dyn FnMut(&'a E)) {
        visit(&self.content);
        for option in [&self.scale, &self.font, &self.color, &self.vertical_align]
            .into_iter()
            .flatten()
        {
            visit(option);
        }
    }
}
