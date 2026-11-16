//! Pre-compiled style property types for evaluation without JSON AST reparsing.

use std::sync::Arc;

use crate::style::{
    expression::{EvaluationContext, Expression, FeatureProperties},
    property::{PropertyValue, StyleProperty},
};

/// The zoom curve of a camera function: an expression evaluated across zoom levels,
/// invariant across features.
#[derive(Debug, Clone)]
pub struct ZoomCurve<T> {
    expression: Arc<Expression>,
    _marker: std::marker::PhantomData<T>,
}

impl<T: PropertyValue> ZoomCurve<T> {
    /// Creates a zoom curve from an expression.
    pub fn new(expression: Arc<Expression>) -> Self {
        Self {
            expression,
            _marker: std::marker::PhantomData,
        }
    }

    /// Evaluates the curve at a specific zoom level.
    pub fn evaluate(&self, zoom: f64) -> Option<T> {
        let context = EvaluationContext::at_zoom(zoom);
        let value = self.expression.evaluate(&context).ok()?;
        T::from_value(&value)
    }

    /// The underlying expression.
    pub fn expression(&self) -> &Expression {
        &self.expression
    }
}

/// A pre-compiled style property categorised for efficient evaluation:
/// - `Constant`: invariant across zoom and features, evaluated once at load time.
/// - `CameraFunction`: depends only on zoom, evaluated once per frame per layer.
/// - `DataDriven`: depends only on feature properties, evaluated per feature without re-parsing JSON.
/// - `Composite`: depends on both zoom and feature properties.
#[derive(Debug, Clone)]
pub enum CompiledProperty<T> {
    /// Constant value invariant across zoom and features.
    Constant(T),
    /// Function of zoom only, invariant across features.
    CameraFunction(ZoomCurve<T>),
    /// Data-driven property depending on feature properties, invariant across zoom.
    DataDriven(Arc<Expression>),
    /// Composite property depending on both zoom and feature properties.
    Composite(Arc<Expression>),
}

impl<T: PropertyValue> CompiledProperty<T> {
    /// Compiles a [`StyleProperty<T>`] into a [`CompiledProperty<T>`].
    pub fn compile(property: &StyleProperty<T>) -> Option<Self> {
        match property {
            StyleProperty::Constant(value) => Some(Self::Constant(value.clone())),
            StyleProperty::Expression(prop) => {
                let expr = prop.expression();
                let is_feature_const = expr.is_feature_constant();
                let is_zoom_const = expr.is_zoom_constant();
                if is_feature_const && is_zoom_const && expr.is_constant() {
                    let ctx = EvaluationContext::at_zoom(0.0);
                    if let Ok(val) = expr.evaluate(&ctx) {
                        if let Some(t) = T::from_value(&val) {
                            return Some(Self::Constant(t));
                        }
                    }
                    Some(Self::CameraFunction(ZoomCurve::new(Arc::new(expr.clone()))))
                } else if is_feature_const {
                    Some(Self::CameraFunction(ZoomCurve::new(Arc::new(expr.clone()))))
                } else if is_zoom_const {
                    Some(Self::DataDriven(Arc::new(expr.clone())))
                } else {
                    Some(Self::Composite(Arc::new(expr.clone())))
                }
            }
            StyleProperty::Unsupported(_) => None,
        }
    }

    /// Parses a JSON value into a [`CompiledProperty<T>`].
    pub fn parse(json: &serde_json::Value) -> Option<Self> {
        let property = StyleProperty::<T>::parse(json);
        Self::compile(&property)
    }

    /// Evaluates the property for a feature at a given zoom level.
    pub fn evaluate_for(&self, properties: &FeatureProperties, zoom: f64) -> Option<T> {
        match self {
            Self::Constant(val) => Some(val.clone()),
            Self::CameraFunction(curve) => curve.evaluate(zoom),
            Self::DataDriven(expr) | Self::Composite(expr) => {
                let ctx = EvaluationContext::for_feature(zoom, properties);
                let val = expr.evaluate(&ctx).ok()?;
                T::from_value(&val)
            }
        }
    }

    /// Evaluates at zoom only without feature properties.
    pub fn evaluate_at_zoom(&self, zoom: f64) -> Option<T> {
        match self {
            Self::Constant(val) => Some(val.clone()),
            Self::CameraFunction(curve) => curve.evaluate(zoom),
            Self::DataDriven(expr) | Self::Composite(expr) => {
                let ctx = EvaluationContext::at_zoom(zoom);
                let val = expr.evaluate(&ctx).ok()?;
                T::from_value(&val)
            }
        }
    }

    /// Whether the property is invariant across features.
    pub fn is_feature_constant(&self) -> bool {
        match self {
            Self::Constant(_) | Self::CameraFunction(_) => true,
            Self::DataDriven(expr) | Self::Composite(expr) => expr.is_feature_constant(),
        }
    }

    /// Whether the property is invariant across zoom levels.
    pub fn is_zoom_constant(&self) -> bool {
        match self {
            Self::Constant(_) | Self::DataDriven(_) => true,
            Self::CameraFunction(curve) => curve.expression.is_zoom_constant(),
            Self::Composite(expr) => expr.is_zoom_constant(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::expression::Value;

    #[test]
    fn compiled_property_differentiates_constant_camera_data_and_composite() {
        // Constant
        let constant_json = serde_json::json!(42.0);
        let prop = CompiledProperty::<f32>::parse(&constant_json).expect("parses");
        assert!(matches!(prop, CompiledProperty::Constant(42.0)));
        assert!(prop.is_feature_constant() && prop.is_zoom_constant());

        // Camera Function (zoom curve)
        let camera_json = serde_json::json!(["interpolate", ["linear"], ["zoom"], 0, 10, 10, 20]);
        let prop = CompiledProperty::<f32>::parse(&camera_json).expect("parses");
        assert!(matches!(prop, CompiledProperty::CameraFunction(_)));
        assert!(prop.is_feature_constant() && !prop.is_zoom_constant());
        assert_eq!(prop.evaluate_at_zoom(5.0), Some(15.0));

        // Data-driven (feature-dependent only)
        let data_json = serde_json::json!(["get", "population"]);
        let prop = CompiledProperty::<f32>::parse(&data_json).expect("parses");
        assert!(matches!(prop, CompiledProperty::DataDriven(_)));
        assert!(!prop.is_feature_constant() && prop.is_zoom_constant());
        let feat: FeatureProperties = [("population".to_string(), Value::Number(100.0))].into();
        assert_eq!(prop.evaluate_for(&feat, 5.0), Some(100.0));

        // Composite (both zoom and feature dependent)
        let comp_json = serde_json::json!([
            "interpolate",
            ["linear"],
            ["zoom"],
            0,
            ["get", "val"],
            10,
            30
        ]);
        let prop = CompiledProperty::<f32>::parse(&comp_json).expect("parses");
        assert!(matches!(prop, CompiledProperty::Composite(_)));
        assert!(!prop.is_feature_constant() && !prop.is_zoom_constant());
        let feat: FeatureProperties = [("val".to_string(), Value::Number(10.0))].into();
        assert_eq!(prop.evaluate_for(&feat, 5.0), Some(20.0));
    }

    #[test]
    fn resolved_image_coalesce_is_not_folded_before_draw_time() {
        let expr_json = serde_json::json!(["coalesce", ["image", "missing"], ["image", "present"]]);
        let prop = CompiledProperty::<crate::style::layer::PatternName>::parse(&expr_json)
            .expect("parses");
        let CompiledProperty::CameraFunction(curve) = prop else {
            panic!("image coalesce must not fold at parse time without available_images");
        };
        let available = std::collections::HashMap::from([("present".to_string(), ())]);
        let mut ctx = EvaluationContext::at_zoom(0.0);
        ctx.available_images = Some(&available);
        let evaluated = curve
            .expression()
            .evaluate(&ctx)
            .ok()
            .and_then(|val| crate::style::layer::PatternName::from_value(&val));
        assert_eq!(
            evaluated,
            Some(crate::style::layer::PatternName("present".to_string()))
        );
    }
}
