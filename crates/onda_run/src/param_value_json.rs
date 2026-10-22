use onda_daemon::RunParamValue;
use serde_json::Value;

pub fn parse_run_param_value(text: &str) -> Result<RunParamValue, String> {
    let text = text.trim();
    if let Ok(value) = text.parse::<i64>() {
        return Ok(if value == 0 && text.starts_with('-') {
            RunParamValue::F64(-0.0)
        } else {
            RunParamValue::I64(value)
        });
    }
    text.parse()
        .map(RunParamValue::F64)
        .map_err(|_| format!("invalid numeric parameter value '{text}'"))
}

pub fn param_value_from_json(value: &Value) -> Result<RunParamValue, String> {
    match value {
        Value::String(text) => parse_run_param_value(text),
        Value::Bool(value) => Ok(RunParamValue::Bool(*value)),
        Value::Number(value) => {
            if let Some(integer) = value.as_i64() {
                Ok(RunParamValue::I64(integer))
            } else {
                value
                    .as_f64()
                    .map(RunParamValue::F64)
                    .ok_or_else(|| "parameter value must be numeric".to_owned())
            }
        }
        _ => Err("parameter value must be a number, numeric string, or boolean".to_owned()),
    }
}

pub fn param_value_to_json(value: RunParamValue) -> Value {
    match value {
        RunParamValue::I64(value) => Value::String(value.to_string()),
        RunParamValue::I32(value) => Value::from(value),
        RunParamValue::Bool(value) => Value::Bool(value),
        RunParamValue::F32(value) => {
            Value::from(value.to_string().parse::<f64>().expect("f32 decimal"))
        }
        RunParamValue::F64(value) => Value::from(value),
    }
}

pub(super) fn param_value_repr(value: RunParamValue) -> String {
    match value {
        RunParamValue::I64(value) => value.to_string(),
        RunParamValue::I32(value) => value.to_string(),
        RunParamValue::Bool(value) => value.to_string(),
        RunParamValue::F32(value) => value.to_string(),
        RunParamValue::F64(value) => value.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_controls_round_trip_without_floating_point() {
        for value in [
            i64::MIN,
            -9_007_199_254_740_993,
            9_007_199_254_740_993,
            i64::MAX,
        ] {
            let scalar = RunParamValue::I64(value);
            assert_eq!(
                param_value_from_json(&param_value_to_json(scalar)).unwrap(),
                scalar
            );
            assert_eq!(param_value_from_json(&Value::from(value)).unwrap(), scalar);
            assert_eq!(parse_run_param_value(&value.to_string()).unwrap(), scalar);
        }
    }

    #[test]
    fn floating_controls_accept_decimal_values_beyond_the_i64_range() {
        let decimal = "10000000000000000000";
        let scalar = RunParamValue::F64(1e19);
        assert_eq!(parse_run_param_value(decimal).unwrap(), scalar);
        assert_eq!(
            param_value_from_json(&serde_json::from_str::<Value>(decimal).unwrap()).unwrap(),
            scalar
        );
        assert_eq!(
            param_value_from_json(&Value::from(decimal)).unwrap(),
            scalar
        );
        assert!(parse_run_param_value("-0")
            .unwrap()
            .as_f64()
            .is_sign_negative());
    }
}
