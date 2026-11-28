//! Evaluation error diagnostics produced at runtime (or during parse-time constant folding).

use thiserror::Error;

use crate::style::expression::value::Type;

/// Why an expression could not produce a value.
#[derive(Error, Debug, Clone, PartialEq)]
pub enum EvaluationError {
    /// A value had the wrong type for the operator that received it.
    #[error("expected value to be of type {expected}, but found {found} instead")]
    Expected {
        /// Type the operator needed.
        expected: Type,
        /// Type of the value it received.
        found: Type,
    },
    /// A value could not be read as a colour.
    #[error("could not parse color from value '{value}'")]
    InvalidColor {
        /// The value, as `to-string` would print it.
        value: String,
    },
    /// A value could not be parsed or interpolated as a composite style-spec type.
    #[error("could not parse {kind} from value '{value}'")]
    InvalidComposite {
        /// Target composite type name (`padding`, `numberArray`, `colorArray`, etc.).
        kind: &'static str,
        /// String or JSON representation of the offending value.
        value: String,
    },
    /// Two composite stop values could not be interpolated together (e.g. mismatched lengths).
    #[error("{reason}")]
    IncompatibleInterpolation {
        /// Diagnostic reason why interpolation failed.
        reason: String,
    },
    /// An `rgba` component was out of range.
    #[error("invalid rgba value {value}: {reason}")]
    InvalidRgba {
        /// The components.
        value: String,
        /// Which component and range failed.
        reason: String,
    },
    /// A value could not be read as a number.
    #[error("could not convert {value} to number")]
    NotANumber {
        /// The value, as JSON.
        value: String,
    },
    /// An ordering comparison received operands of different or unordered types.
    #[error("expected arguments for \"{operator}\" to be (string, string) or (number, number), but found ({left}, {right}) instead")]
    Incomparable {
        /// The operator.
        operator: &'static str,
        /// Type of the left operand.
        left: Type,
        /// Type of the right operand.
        right: Type,
    },
    /// `in` or `index-of` received a needle that cannot be searched for.
    #[error("expected first argument to be of type boolean, string, number or null, but found {found} instead")]
    NotSearchable {
        /// Type of the needle.
        found: Type,
    },
    /// A string or array operation received something else.
    #[error("expected value to be of type string or array, but found {found} instead")]
    NotIndexable {
        /// Type of the value.
        found: Type,
    },
    /// A `step` or `interpolate` input was not a number, such as the result of dividing by zero.
    #[error("input is not a number")]
    InputNotANumber,
    /// `["error", ...]` was reached during evaluation.
    #[error("{message}")]
    UserError {
        /// User-provided error message.
        message: String,
    },
    /// `at` received a non-integer or out-of-bounds array index.
    #[error("{reason}")]
    InvalidArrayIndex {
        /// Diagnostic message describing the invalid index.
        reason: String,
    },
}
