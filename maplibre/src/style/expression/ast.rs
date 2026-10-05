//! The parsed form of an expression, with the classification the style engine needs.

use super::{
    interpolation::{ColorSpace, Interpolation},
    value::{Type, Value},
};

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
    /// `to-number`, `to-string`, `to-boolean` and `to-color`.
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
}

impl Expression {
    /// The type the expression evaluates to, as far as it is known before evaluation.
    pub fn output_type(&self) -> Type {
        match self {
            Self::Literal(value) => value.type_of(),
            Self::Folded { output, .. } => output.clone(),
            Self::Global(_) => Type::Number,
            Self::Feature(FeatureProperty::Id) => Type::Value,
            Self::Feature(FeatureProperty::GeometryType) => Type::String,
            Self::Feature(FeatureProperty::Properties) => Type::Object,
            Self::Get { .. } | Self::GlobalState(_) => Type::Value,
            Self::Has { .. }
            | Self::Compare { .. }
            | Self::All(_)
            | Self::Any(_)
            | Self::Not(_)
            | Self::IsSupportedScript(_)
            | Self::Within(_)
            | Self::In { .. } => Type::Boolean,
            Self::Distance(_) => Type::Number,
            Self::Collator { .. } => Type::Collator,
            Self::Var { bound, .. } => bound.output_type(),
            Self::Let { body, .. } => body.output_type(),
            Self::Case { output, .. }
            | Self::Match { output, .. }
            | Self::Coalesce { output, .. }
            | Self::Slice { output, .. }
            | Self::Interpolate { output, .. }
            | Self::Step { output, .. } => output.clone(),
            Self::IndexOf { .. }
            | Self::Length(_)
            | Self::Arithmetic { .. }
            | Self::Math { .. }
            | Self::MinMax { .. } => Type::Number,
            Self::TypeOf(_)
            | Self::Concat(_)
            | Self::StringCase { .. }
            | Self::ResolvedLocale(_)
            | Self::NumberFormat { .. } => Type::String,
            Self::Image(_) => Type::ResolvedImage,
            Self::Format(_) => Type::Formatted,
            Self::Assert { required, .. } => required.clone(),
            Self::Coerce { coercion, .. } => match coercion {
                Coercion::Number => Type::Number,
                Coercion::String => Type::String,
                Coercion::Boolean => Type::Boolean,
                Coercion::Color => Type::Color,
                Coercion::Formatted => Type::Formatted,
                Coercion::ResolvedImage => Type::ResolvedImage,
            },
            Self::ToRgba(_) => Type::array(Type::Number, Some(4)),
            Self::Rgba(_) => Type::Color,
        }
    }

    /// Calls `visit` on every direct child.
    pub fn for_each_child<'a>(&'a self, visit: &mut dyn FnMut(&'a Expression)) {
        match self {
            Self::Literal(_)
            | Self::Folded { .. }
            | Self::Global(_)
            | Self::Feature(_)
            | Self::Within(_)
            | Self::Distance(_)
            | Self::GlobalState(_) => {}
            Self::NumberFormat { input, options } => {
                visit(input);
                options.iter().for_each(|(_, option)| visit(option));
            }
            Self::Get { key, object } | Self::Has { key, object } => {
                visit(key);
                if let Some(object) = object {
                    visit(object);
                }
            }
            Self::Var { bound, .. } => visit(bound),
            Self::Let { bindings, body } => {
                for (_, bound) in bindings {
                    visit(bound);
                }
                visit(body);
            }
            Self::Case {
                branches, fallback, ..
            } => {
                for (condition, output) in branches {
                    visit(condition);
                    visit(output);
                }
                visit(fallback);
            }
            Self::Match {
                input,
                cases,
                fallback,
                ..
            } => {
                visit(input);
                for (_, output) in cases {
                    visit(output);
                }
                visit(fallback);
            }
            Self::Coalesce { operands, .. }
            | Self::All(operands)
            | Self::Any(operands)
            | Self::Arithmetic { operands, .. }
            | Self::MinMax { operands, .. }
            | Self::Assert { operands, .. }
            | Self::Coerce { operands, .. }
            | Self::Rgba(operands)
            | Self::Concat(operands) => operands.iter().for_each(visit),
            Self::Compare {
                left,
                right,
                collator,
                ..
            } => {
                visit(left);
                visit(right);
                if let Some(collator) = collator {
                    visit(collator);
                }
            }
            Self::Collator {
                case_sensitive,
                diacritic_sensitive,
                locale,
            } => {
                visit(case_sensitive);
                visit(diacritic_sensitive);
                if let Some(locale) = locale {
                    visit(locale);
                }
            }
            Self::Not(operand)
            | Self::ResolvedLocale(operand)
            | Self::IsSupportedScript(operand)
            | Self::Length(operand)
            | Self::Math { operand, .. }
            | Self::TypeOf(operand)
            | Self::Image(operand)
            | Self::ToRgba(operand)
            | Self::StringCase { operand, .. } => visit(operand),
            Self::In { needle, haystack } => {
                visit(needle);
                visit(haystack);
            }
            Self::IndexOf {
                needle,
                haystack,
                from,
            } => {
                visit(needle);
                visit(haystack);
                if let Some(from) = from {
                    visit(from);
                }
            }
            Self::Slice {
                input, from, to, ..
            } => {
                visit(input);
                visit(from);
                if let Some(to) = to {
                    visit(to);
                }
            }
            Self::Interpolate { input, stops, .. } | Self::Step { input, stops, .. } => {
                visit(input);
                for (_, output) in stops {
                    visit(output);
                }
            }
            Self::Format(sections) => sections.iter().for_each(|section| section.for_each(visit)),
        }
    }

    /// Whether the result is the same for every feature.
    pub fn is_feature_constant(&self) -> bool {
        match self {
            Self::Feature(_) | Self::Within(_) | Self::Distance(_) => false,
            Self::Get { object: None, .. } | Self::Has { object: None, .. } => false,
            _ => self.children_all(Self::is_feature_constant),
        }
    }

    /// Whether the result is the same at every zoom.
    pub fn is_zoom_constant(&self) -> bool {
        match self {
            Self::Global(Global::Zoom) => false,
            _ => self.children_all(Self::is_zoom_constant),
        }
    }

    /// Whether the result depends on neither the feature nor the map.
    pub fn is_constant(&self) -> bool {
        match self {
            // Whether an image exists is known only where it is drawn.
            Self::Global(_) | Self::GlobalState(_) | Self::Image(_) => false,
            _ => self.is_feature_constant() && self.children_all(Self::is_constant),
        }
    }

    fn children_all(&self, predicate: fn(&Expression) -> bool) -> bool {
        let mut all = true;
        self.for_each_child(&mut |child| {
            if all && !predicate(child) {
                all = false;
            }
        });
        all
    }
}
