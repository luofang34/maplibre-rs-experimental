use super::*;

/// The typed value of a feature property, as an expression sees it.
pub(crate) fn property_value(value: &ColumnValue) -> Option<Value> {
    Some(match value {
        ColumnValue::Bool(flag) => Value::Bool(*flag),
        ColumnValue::Byte(number) => Value::Number(f64::from(*number)),
        ColumnValue::UByte(number) => Value::Number(f64::from(*number)),
        ColumnValue::Short(number) => Value::Number(f64::from(*number)),
        ColumnValue::UShort(number) => Value::Number(f64::from(*number)),
        ColumnValue::Int(number) => Value::Number(f64::from(*number)),
        ColumnValue::UInt(number) => Value::Number(f64::from(*number)),
        ColumnValue::Long(number) => Value::Number(*number as f64),
        ColumnValue::ULong(number) => Value::Number(*number as f64),
        ColumnValue::Float(number) => Value::Number(f64::from(*number)),
        ColumnValue::Double(number) => Value::Number(*number),
        ColumnValue::String(text) | ColumnValue::DateTime(text) => {
            Value::String((*text).to_string())
        }
        ColumnValue::Json(text) => serde_json::from_str::<serde_json::Value>(text)
            .map(|json| Value::from_json(&json))
            .unwrap_or_else(|_| Value::String((*text).to_string())),
        ColumnValue::Binary(_) => return None,
    })
}
