//! The parsed form of an expression, with the classification the style engine needs.

use super::{
    interpolation::{ColorSpace, Interpolation},
    value::{Type, Value},
};

mod children;
mod classify;
mod format;
pub use format::FormatSection;

/// A property of the map as a whole, the same for every feature.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Global {
    /// The `zoom` operator.
    Zoom,
    /// The `elevation` operator.
    Elevation,
    /// The `heatmap-density` operator.
    HeatmapDensity,
    /// The `line-progress` operator.
    LineProgress,
}

/// A property of the feature being evaluated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeatureProperty {
    /// The `id` operator.
    Id,
    /// The `geometry-type` operator.
    GeometryType,
    /// The `properties` operator.
    Properties,
}

/// Comparison operators.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Comparison {
    /// `==`
    Equal,
    /// `!=`
    NotEqual,
    /// `<`
    Less,
    /// `<=`
    LessEqual,
    /// `>`
    Greater,
    /// `>=`
    GreaterEqual,
}

impl Comparison {
    /// The operator as the style writes it.
    pub fn symbol(self) -> &'static str {
        match self {
            Self::Equal => "==",
            Self::NotEqual => "!=",
            Self::Less => "<",
            Self::LessEqual => "<=",
            Self::Greater => ">",
            Self::GreaterEqual => ">=",
        }
    }

    /// Whether the operator orders its operands rather than testing equality.
    pub fn is_ordering(self) -> bool {
        !matches!(self, Self::Equal | Self::NotEqual)
    }
}

/// Arithmetic operators over numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arithmetic {
    /// `+`, any number of operands.
    Add,
    /// `-`, one operand negates, two subtract.
    Subtract,
    /// `*`, any number of operands.
    Multiply,
    /// `/`
    Divide,
    /// `%`
    Remainder,
    /// `^`
    Power,
}

/// One-operand mathematical functions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MathFunction {
    /// `sqrt`
    Sqrt,
    /// `ln`
    Ln,
    /// `log10`
    Log10,
    /// `log2`
    Log2,
    /// `sin`
    Sin,
    /// `cos`
    Cos,
    /// `tan`
    Tan,
    /// `asin`
    Asin,
    /// `acos`
    Acos,
    /// `atan`
    Atan,
    /// `abs`
    Abs,
    /// `round`, away from zero at the midpoint as JavaScript rounds.
    Round,
    /// `floor`
    Floor,
    /// `ceil`
    Ceil,
}

/// Conversions between types.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Coercion {
    /// `to-number`
    Number,
    /// `to-string`
    String,
    /// `to-boolean`
    Boolean,
    /// `to-color`
    Color,
    /// Text into one section of formatted text, as a `text-field` takes it.
    Formatted,
    /// A name into an image, as an image property takes it.
    ResolvedImage,
    /// A number or 1..=4 numeric array into directional `[top, right, bottom, left]` padding.
    Padding,
    /// A number or numeric array into a `numberArray`.
    NumberArray,
    /// A colour, colour string, or colour array into a `colorArray`.
    ColorArray,
    /// A projection name, `[from, to, transition]` tuple, or transition object into `projectionDefinition`.
    ProjectionDefinition,
    /// Alternating anchor strings and `[x, y]` offset arrays into `variableAnchorOffsetCollection`.
    VariableAnchorOffsetCollection,
}

/// One-operand string functions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StringFunction {
    /// `upcase`
    Upcase,
    /// `downcase`
    Downcase,
}

/// A parsed expression.
///
/// Every node knows its output type, so a parsed expression can be classified and checked
/// without evaluating it.
#[derive(Clone, Debug, PartialEq)]
pub enum Expression {
    /// A constant.
    Literal(Value),
    /// A constant computed while parsing, keeping the type the source expression declared.
    Folded {
        /// The computed value.
        value: Value,
        /// Type of the source expression.
        output: Type,
    },
    /// A property of the map.
    Global(Global),
    /// A property of the feature.
    Feature(FeatureProperty),
    /// `accumulated`: the cluster property value accumulated so far, or `null` outside clustering.
    Accumulated,
    /// `global-state`: a map-level value, `null` unless the evaluation context carries one.
    GlobalState(String),
    /// `get`: a property of the feature, or of `object` when given.
    Get {
        /// Name of the property.
        key: Box<Expression>,
        /// Object to read from instead of the feature.
        object: Option<Box<Expression>>,
    },
    /// `has`: whether the feature, or `object`, has the property.
    Has {
        /// Name of the property.
        key: Box<Expression>,
        /// Object to look in instead of the feature.
        object: Option<Box<Expression>>,
    },
    /// `var`: a variable bound by an enclosing `let`, resolved when parsed.
    Var {
        /// Name of the variable.
        name: String,
        /// The expression bound to it.
        bound: Box<Expression>,
    },
    /// `let`: bindings for the variables `body` may use.
    Let {
        /// Bound expressions, kept so classification sees them.
        bindings: Vec<(String, Expression)>,
        /// The result.
        body: Box<Expression>,
    },
    /// `case`: the output of the first branch whose condition holds.
    Case {
        /// Condition and output pairs.
        branches: Vec<(Expression, Expression)>,
        /// Output when no condition holds.
        fallback: Box<Expression>,
        /// Type of every output.
        output: Type,
    },
    /// `match`: the output whose labels contain the input.
    Match {
        /// The value to look up.
        input: Box<Expression>,
        /// Type the labels share; an input of another type takes the fallback.
        input_type: Type,
        /// Labels and output pairs.
        cases: Vec<(Vec<Value>, Expression)>,
        /// Output when no label matches.
        fallback: Box<Expression>,
        /// Type of every output.
        output: Type,
    },
    /// `coalesce`: the first output that is not `null`.
    Coalesce {
        /// Candidates in order.
        operands: Vec<Expression>,
        /// Type of the outputs.
        output: Type,
    },
    /// A comparison of two values.
    Compare {
        /// The operator.
        operator: Comparison,
        /// Left operand.
        left: Box<Expression>,
        /// Right operand.
        right: Box<Expression>,
        /// Whether an operand's type is only known at run time.
        untyped: bool,
        /// How strings compare, when the expression names a collator.
        collator: Option<Box<Expression>>,
    },
    /// `collator`: a rule for comparing strings.
    Collator {
        /// Whether letters differing only in case differ.
        case_sensitive: Box<Expression>,
        /// Whether letters differing only in their accents differ.
        diacritic_sensitive: Box<Expression>,
        /// The locale, if given.
        locale: Option<Box<Expression>>,
    },
    /// `within`: whether the feature's geometry lies inside the area.
    Within(super::geometry::Geometry),
    /// `distance`: metres from the feature's geometry to the geometry.
    Distance(super::geometry::Geometry),
    /// `number-format`: a number as text, in the style of a locale.
    NumberFormat {
        /// The number.
        input: Box<Expression>,
        /// Options: `locale`, `currency`, `min-fraction-digits`, `max-fraction-digits`.
        options: Vec<(String, Expression)>,
    },
    /// `resolved-locale`: the locale a collator compares in.
    ResolvedLocale(Box<Expression>),
    /// `is-supported-script`: whether a string is in scripts the renderer can lay out.
    IsSupportedScript(Box<Expression>),
    /// `all`
    All(Vec<Expression>),
    /// `any`
    Any(Vec<Expression>),
    /// `!`
    Not(Box<Expression>),
    /// `in`: whether a string or array contains the needle.
    In {
        /// What to look for.
        needle: Box<Expression>,
        /// Where to look.
        haystack: Box<Expression>,
    },
    /// `index-of`: where a string or array holds the needle, or minus one.
    IndexOf {
        /// What to look for.
        needle: Box<Expression>,
        /// Where to look.
        haystack: Box<Expression>,
        /// Index to start from.
        from: Option<Box<Expression>>,
    },
    /// `slice` of a string or array.
    Slice {
        /// The string or array.
        input: Box<Expression>,
        /// First index kept.
        from: Box<Expression>,
        /// First index dropped.
        to: Option<Box<Expression>>,
        /// Type of the input, which is also the output's.
        output: Type,
    },
    /// `length` of a string or array.
    Length(Box<Expression>),
    /// Arithmetic over numbers.
    Arithmetic {
        /// The operator.
        operator: Arithmetic,
        /// Operands, all numbers.
        operands: Vec<Expression>,
    },
    /// A one-operand mathematical function.
    Math {
        /// The function.
        function: MathFunction,
        /// Its operand.
        operand: Box<Expression>,
    },
    /// `min` or `max` over numbers.
    MinMax {
        /// `true` for `max`.
        max: bool,
        /// Operands, all numbers.
        operands: Vec<Expression>,
    },
    /// `typeof`
    TypeOf(Box<Expression>),
    /// `number`, `string`, `boolean`, `object` and `array`: the first operand of the type.
    Assert {
        /// Required type.
        required: Type,
        /// Candidates in order.
        operands: Vec<Expression>,
    },
    /// An image by name, with whether the map holds it.
    Image(Box<Expression>),
    /// `format`: text in sections, each of which may be an image.
    Format(Vec<FormatSection>),
    /// Type coercions (`to-number`, `to-string`, `to-boolean`, `to-color`, and composite property coercions).
    Coerce {
        /// Conversion.
        coercion: Coercion,
        /// Candidates in order.
        operands: Vec<Expression>,
    },
    /// `to-rgba`
    ToRgba(Box<Expression>),
    /// `rgb` and `rgba`.
    Rgba(Vec<Expression>),
    /// `interpolate` and its colour-space variants.
    Interpolate {
        /// The curve.
        interpolation: Interpolation,
        /// Colour space for colour outputs.
        space: ColorSpace,
        /// The number to look up.
        input: Box<Expression>,
        /// Ascending stop inputs with their outputs.
        stops: Vec<(f64, Expression)>,
        /// Type of every output.
        output: Type,
    },
    /// `step`: the output of the last stop at or below the input.
    Step {
        /// The number to look up.
        input: Box<Expression>,
        /// Ascending stop inputs with their outputs; the first is minus infinity.
        stops: Vec<(f64, Expression)>,
        /// Type of every output.
        output: Type,
    },
    /// `concat`
    Concat(Vec<Expression>),
    /// `upcase` and `downcase`.
    StringCase {
        /// Which case.
        function: StringFunction,
        /// The string.
        operand: Box<Expression>,
    },
    /// `error`: raises a user-specified evaluation error when reached.
    Error(Box<Expression>),
    /// `at`: element lookup at an integer index in an array.
    At {
        /// Zero-based integer index.
        index: Box<Expression>,
        /// Array operand.
        array: Box<Expression>,
        /// Static item type of the array.
        output: Type,
    },
    /// `split`: splits a string by a delimiter into `array<string>`.
    Split {
        /// Source string.
        input: Box<Expression>,
        /// Separator string.
        delimiter: Box<Expression>,
    },
    /// `join`: joins array elements into a string with a separator.
    Join {
        /// Array operand.
        input: Box<Expression>,
        /// Separator string.
        separator: Box<Expression>,
    },
}
