//! Reports the parts of a style this renderer cannot honour, so a caller learns about an
//! unsupported filter when the style loads rather than from a layer that renders nothing.

use thiserror::Error;

use crate::style::{filter::Filter, filter::FilterError, Style};

/// One thing in a style the renderer would otherwise skip silently.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum StyleValidationError {
    /// A layer's filter uses syntax outside the supported subset; the layer renders nothing.
    #[error("layer `{layer}` has an unsupported filter: {source}")]
    Filter {
        /// The id of the layer carrying the filter.
        layer: String,
        /// Why the filter cannot be evaluated.
        #[source]
        source: FilterError,
    },
}

impl Style {
    /// Lists every unsupported construct in the style. An empty list means every layer can be
    /// evaluated as written.
    pub fn validate(&self) -> Result<(), Vec<StyleValidationError>> {
        let errors: Vec<StyleValidationError> = self
            .layers
            .iter()
            .filter_map(|layer| {
                let filter = layer.filter.as_ref()?;
                Filter::parse(filter)
                    .err()
                    .map(|source| StyleValidationError::Filter {
                        layer: layer.id.clone(),
                        source,
                    })
            })
            .collect();
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Logs each validation error; the map still loads and renders what it can.
    pub fn log_validation_errors(&self) {
        if let Err(errors) = self.validate() {
            for error in errors {
                tracing::error!(%error, "style uses a construct this renderer does not support");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::panic)]

    use super::StyleValidationError;
    use crate::style::{filter::FilterError, Style};

    #[test]
    fn unsupported_filters_are_reported_per_layer() {
        let style: Style = serde_json::from_str(
            r#"{
                "version": 8,
                "sources": {},
                "layers": [
                    {"id": "ok", "type": "line", "filter": ["==", ["get", "level"], "low"]},
                    {"id": "bad", "type": "line", "filter": ["within", {"type": "Polygon", "coordinates": []}]}
                ]
            }"#,
        )
        .expect("style parses");

        assert_eq!(
            style.validate(),
            Err(vec![StyleValidationError::Filter {
                layer: "bad".to_string(),
                source: FilterError::UnsupportedOperator {
                    operator: "within".to_string()
                }
            }])
        );
    }

    #[test]
    fn the_default_style_validates() {
        assert_eq!(Style::default().validate(), Ok(()));
    }
}
