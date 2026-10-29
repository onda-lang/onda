//! Typed event values and their editor paths. JSON transport remains lossless
//! for i64 and non-finite floats; shape validation happens before dispatch.
use serde_json::{json, Value};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct EventField {
    pub event: usize,
    pub arg: usize,
    pub path: String,
    pub scalar: String,
}

#[derive(Clone)]
pub(crate) struct Argument {
    pub name: String,
    pub shape: Value,
    pub value: Value,
    pub default: Value,
}
#[derive(Clone)]
pub(crate) struct Event {
    pub name: String,
    pub args: Vec<Argument>,
}

pub(crate) fn shape(arg: &Value) -> Value {
    if arg["shape"].is_object() {
        return arg["shape"].clone();
    }
    let ty = arg["type"].as_str().unwrap_or("f64");
    if let Some((scalar, length)) = ty.strip_suffix(']').and_then(|s| s.split_once('[')) {
        let element = json!({"kind":"scalar","encoding":scalar});
        return if length.is_empty() || arg["isSlice"] == true {
            json!({"kind":"slice","element":element})
        } else {
            json!({"kind":"array","element":element,"len":length.parse::<usize>().unwrap_or(0)})
        };
    }
    json!({"kind":"scalar","encoding":ty})
}

pub(crate) fn default_value(shape: &Value) -> Value {
    decode_default(shape, None)
}

fn decode_default(shape: &Value, value: Option<&Value>) -> Value {
    match shape["kind"].as_str() {
        Some("struct") => shape["fields"]
            .as_array()
            .map(|fields| {
                fields
                    .iter()
                    .enumerate()
                    .filter_map(|(index, f)| {
                        Some((
                            f["name"].as_str()?.to_owned(),
                            decode_default(
                                &f["ty"],
                                value
                                    .and_then(|v| v.get(index))
                                    .or_else(|| f.get("default")),
                            ),
                        ))
                    })
                    .collect::<serde_json::Map<_, _>>()
                    .into()
            })
            .unwrap_or_else(|| json!({})),
        Some("tuple") => shape["elements"]
            .as_array()
            .map(|v| {
                Value::Array(
                    v.iter()
                        .enumerate()
                        .map(|(i, shape)| decode_default(shape, value.and_then(|v| v.get(i))))
                        .collect(),
                )
            })
            .unwrap_or_else(|| json!([])),
        Some("array") => Value::Array(
            (0..shape["len"].as_u64().unwrap_or(0))
                .map(|i| decode_default(&shape["element"], value.and_then(|v| v.get(i as usize))))
                .collect(),
        ),
        Some("slice") => Value::Array(
            value
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|v| decode_default(&shape["element"], Some(v)))
                .collect(),
        ),
        _ => {
            let ty = shape["encoding"].as_str().unwrap_or("");
            let decoded = value.and_then(Value::as_str).and_then(|text| {
                if let Some(bits) = text.strip_prefix("0x") {
                    return match ty {
                        "f32" if bits.len() == 8 => scalar_value(
                            ty,
                            &f32::from_bits(u32::from_str_radix(bits, 16).ok()?).to_string(),
                        ),
                        "f64" if bits.len() == 16 => scalar_value(
                            ty,
                            &f64::from_bits(u64::from_str_radix(bits, 16).ok()?).to_string(),
                        ),
                        _ => None,
                    };
                }
                scalar_value(ty, text)
            });
            decoded.unwrap_or_else(|| match ty {
                "bool" => json!(false),
                "i64" => json!("0"),
                _ => json!(0),
            })
        }
    }
}

pub(crate) fn scalar_value(ty: &str, text: &str) -> Option<Value> {
    let text = text.trim();
    match ty {
        "bool" => text.parse::<bool>().ok().map(Value::Bool),
        "i64" => text.parse::<i64>().ok().map(|n| json!(n.to_string())),
        "i32" => text.parse::<i32>().ok().map(|n| json!(n)),
        "f32" | "f64" => {
            let mut n = text.parse::<f64>().ok()?;
            if ty == "f32" {
                n = f64::from(n as f32);
            }
            Some(if n.is_nan() {
                json!("NaN")
            } else if n == f64::INFINITY {
                json!("Infinity")
            } else if n == f64::NEG_INFINITY {
                json!("-Infinity")
            } else {
                json!(n)
            })
        }
        _ => None,
    }
}

pub(crate) fn valid(shape: &Value, value: &Value) -> bool {
    match shape["kind"].as_str() {
        Some("scalar") => {
            let ty = shape["encoding"].as_str().unwrap_or("");
            if ty == "bool" {
                return value.is_boolean();
            }
            let text = value
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| value.to_string());
            scalar_value(ty, &text).is_some()
        }
        Some("struct") => shape["fields"].as_array().is_some_and(|fields| {
            value.as_object().is_some_and(|object| {
                object.len() == fields.len()
                    && fields.iter().all(|f| {
                        f["name"]
                            .as_str()
                            .and_then(|n| object.get(n))
                            .is_some_and(|v| valid(&f["ty"], v))
                    })
            })
        }),
        Some("tuple") => shape["elements"]
            .as_array()
            .zip(value.as_array())
            .is_some_and(|(shapes, values)| {
                shapes.len() == values.len() && shapes.iter().zip(values).all(|(s, v)| valid(s, v))
            }),
        Some("array" | "slice") => value.as_array().is_some_and(|values| {
            (shape["kind"] == "slice" || shape["len"].as_u64() == Some(values.len() as u64))
                && values.iter().all(|v| valid(&shape["element"], v))
        }),
        _ => false,
    }
}

impl Event {
    pub fn reset_arguments(&mut self) {
        for arg in &mut self.args {
            arg.value = arg.default.clone();
        }
    }
    pub fn from_metadata(metadata: &Value) -> Option<Self> {
        Some(Self {
            name: metadata["name"].as_str()?.into(),
            args: metadata["args"]
                .as_array()
                .map(|args| {
                    args.iter()
                        .filter_map(|arg| {
                            let shape = shape(arg);
                            let default = arg
                                .get("default")
                                .cloned()
                                .filter(|v| valid(&shape, v))
                                .unwrap_or_else(|| default_value(&shape));
                            let value = arg
                                .get("value")
                                .cloned()
                                .filter(|v| valid(&shape, v))
                                .unwrap_or_else(|| default.clone());
                            Some(Argument {
                                name: arg["name"].as_str()?.into(),
                                shape,
                                value,
                                default,
                            })
                        })
                        .collect()
                })
                .unwrap_or_default(),
        })
    }
    pub fn same_schema(&self, other: &Self) -> bool {
        self.name == other.name
            && self.args.len() == other.args.len()
            && self
                .args
                .iter()
                .zip(&other.args)
                .all(|(a, b)| a.name == b.name && a.shape == b.shape && a.default == b.default)
    }
    pub fn valid(&self) -> bool {
        self.args.iter().all(|a| valid(&a.shape, &a.value))
    }
}

pub(crate) fn child_path(parent: &str, field: &str) -> String {
    format!("{parent}/{}", field.replace('~', "~0").replace('/', "~1"))
}

// Canonical wire values keep wide integers out of JavaScript's number domain.
pub(crate) fn canonical(shape: &Value, value: &Value) -> Option<Value> {
    if !valid(shape, value) {
        return None;
    }
    Some(match shape["kind"].as_str()? {
        "scalar" => scalar_value(
            shape["encoding"].as_str()?,
            &value
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| value.to_string()),
        )?,
        "struct" => Value::Object(
            shape["fields"]
                .as_array()?
                .iter()
                .map(|f| {
                    let name = f["name"].as_str()?;
                    Some((name.to_owned(), canonical(&f["ty"], &value[name])?))
                })
                .collect::<Option<serde_json::Map<_, _>>>()?,
        ),
        "tuple" => Value::Array(
            shape["elements"]
                .as_array()?
                .iter()
                .zip(value.as_array()?)
                .map(|(s, v)| canonical(s, v))
                .collect::<Option<Vec<_>>>()?,
        ),
        "array" | "slice" => Value::Array(
            value
                .as_array()?
                .iter()
                .map(|v| canonical(&shape["element"], v))
                .collect::<Option<Vec<_>>>()?,
        ),
        _ => return None,
    })
}
