use onda_daemon::RunEventValue;
use serde_json::{Number, Value};

const NAN: &str = "NaN";
const POSITIVE_INFINITY: &str = "Infinity";
const NEGATIVE_INFINITY: &str = "-Infinity";
const NEGATIVE_ZERO: &str = "-0";

pub fn event_value_to_json(value: &RunEventValue) -> Value {
    match value {
        RunEventValue::Bool(value) => Value::Bool(*value),
        RunEventValue::Number(value) => float_to_json(*value),
        RunEventValue::I64(value) => Value::String(value.to_string()),
        RunEventValue::Array(values) => {
            Value::Array(values.iter().map(event_value_to_json).collect())
        }
        RunEventValue::Struct(fields) => Value::Object(
            fields
                .iter()
                .map(|(name, value)| (name.clone(), event_value_to_json(value)))
                .collect(),
        ),
    }
}

pub fn event_value_from_json(value: Value) -> Result<RunEventValue, String> {
    match value {
        Value::Bool(value) => Ok(RunEventValue::Bool(value)),
        Value::Number(value) => value
            .as_f64()
            .map(RunEventValue::Number)
            .ok_or_else(|| "event values must be representable as f64".to_owned()),
        Value::String(value) => match value.as_str() {
            NAN => Ok(RunEventValue::Number(f64::NAN)),
            POSITIVE_INFINITY => Ok(RunEventValue::Number(f64::INFINITY)),
            NEGATIVE_INFINITY => Ok(RunEventValue::Number(f64::NEG_INFINITY)),
            NEGATIVE_ZERO => Ok(RunEventValue::Number(-0.0)),
            _ => value
                .parse::<i64>()
                .map(RunEventValue::I64)
                .map_err(|_| {
                    "event string values must be decimal i64 integers, 'NaN', 'Infinity', '-Infinity', or '-0'"
                        .to_owned()
                }),
        },
        Value::Array(values) => values
            .into_iter()
            .map(event_value_from_json)
            .collect::<Result<Vec<_>, _>>()
            .map(RunEventValue::Array),
        Value::Object(values) => values
            .into_iter()
            .map(|(name, value)| event_value_from_json(value).map(|value| (name, value)))
            .collect::<Result<_, _>>()
            .map(RunEventValue::Struct),
        Value::Null => Err(
            "event values must be numbers, decimal i64 or special float strings, booleans, arrays, or objects"
                .to_owned(),
        ),
    }
}

fn float_to_json(value: f64) -> Value {
    if value.is_nan() {
        Value::String(NAN.to_owned())
    } else if value == f64::INFINITY {
        Value::String(POSITIVE_INFINITY.to_owned())
    } else if value == f64::NEG_INFINITY {
        Value::String(NEGATIVE_INFINITY.to_owned())
    } else if value == 0.0 && value.is_sign_negative() {
        Value::String(NEGATIVE_ZERO.to_owned())
    } else {
        Value::Number(Number::from_f64(value).expect("finite f64 must be a JSON number"))
    }
}

#[cfg(test)]
mod tests {
    use super::{event_value_from_json, event_value_to_json};
    use onda_daemon::RunEventValue;
    use serde_json::{json, Value};

    #[test]
    fn negative_zero_string_decodes_as_a_float() {
        let RunEventValue::Number(value) = event_value_from_json(json!("-0")).unwrap() else {
            panic!("negative zero must retain its floating-point encoding");
        };
        assert_eq!(value.to_bits(), (-0.0_f64).to_bits());
        for value in [0, i64::MIN, i64::MAX, 9_007_199_254_740_993] {
            assert_eq!(
                event_value_from_json(json!(value.to_string())).unwrap(),
                RunEventValue::I64(value)
            );
        }
    }

    #[test]
    fn negative_zero_serializes_without_losing_its_sign_in_javascript() {
        assert_eq!(
            event_value_to_json(&RunEventValue::Number(-0.0)),
            json!("-0")
        );
        assert_eq!(event_value_to_json(&RunEventValue::Number(0.0)), json!(0.0));
    }

    #[test]
    fn event_values_round_trip_losslessly_and_preserve_field_order() {
        let value = RunEventValue::Struct(vec![
            (
                "zeta".to_owned(),
                RunEventValue::Array(vec![
                    RunEventValue::Number(f64::NAN),
                    RunEventValue::Number(f64::INFINITY),
                    RunEventValue::Number(f64::NEG_INFINITY),
                    RunEventValue::Number(-0.0),
                    RunEventValue::Number(0.25),
                ]),
            ),
            (
                "alpha".to_owned(),
                RunEventValue::I64(9_007_199_254_740_993),
            ),
        ]);

        let encoded = event_value_to_json(&value);
        assert_eq!(
            encoded,
            json!({
                "zeta": ["NaN", "Infinity", "-Infinity", "-0", 0.25],
                "alpha": "9007199254740993",
            })
        );
        assert_eq!(
            encoded.to_string(),
            r#"{"zeta":["NaN","Infinity","-Infinity","-0",0.25],"alpha":"9007199254740993"}"#
        );

        let RunEventValue::Struct(decoded) =
            event_value_from_json(encoded).expect("encoded value should decode")
        else {
            panic!("expected a struct value");
        };
        assert_eq!(decoded[0].0, "zeta");
        assert_eq!(decoded[1].0, "alpha");
        let RunEventValue::Array(floats) = &decoded[0].1 else {
            panic!("expected a float array");
        };
        let numbers = floats
            .iter()
            .map(|value| match value {
                RunEventValue::Number(value) => *value,
                _ => panic!("expected a numeric value"),
            })
            .collect::<Vec<_>>();
        assert!(numbers[0].is_nan());
        assert_eq!(numbers[1], f64::INFINITY);
        assert_eq!(numbers[2], f64::NEG_INFINITY);
        assert_eq!(numbers[3].to_bits(), (-0.0_f64).to_bits());
        assert_eq!(numbers[4], 0.25);
        assert_eq!(decoded[1].1, RunEventValue::I64(9_007_199_254_740_993));
    }

    #[test]
    fn event_value_json_rejects_unknown_strings_and_null() {
        assert!(event_value_from_json(Value::String("1.5".to_owned())).is_err());
        assert!(event_value_from_json(Value::Null).is_err());
    }
}
