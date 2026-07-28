use anyhow::{Context, Result, anyhow, bail, ensure};
use quartz_nbt::{NbtCompound, NbtList, NbtTag};
use serde_json::{Map, Number, Value};

pub const TAG_KEY: &str = "__minecraftNbt";
pub const VALUE_KEY: &str = "value";

#[derive(Debug)]
pub struct Document {
    pub root_name: String,
    pub compound: NbtCompound,
}

/// Decodes a JSON representation of an NBT document.
///
/// # Errors
///
/// Returns an error when the input uses an invalid explicit tag wrapper, contains
/// a value that cannot be represented as the requested NBT tag, or decodes to a
/// non-compound document root.
pub fn decode_document(value: &Value, default_root_name: &str) -> Result<Document> {
    let (root_name, root_value) = match value {
        Value::Object(object) if tag_name(object) == Some("root") => {
            let root_name = object
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("NBT root wrapper must include a string name"))?;
            let root_value = tagged_value(object).context("NBT root wrapper")?;

            (root_name.to_owned(), root_value)
        }
        _ => (default_root_name.to_owned(), value),
    };

    let tag = decode_tag("$", root_value)?;
    let compound = match tag {
        NbtTag::Compound(compound) => compound,
        other => bail!(
            "NBT document root must be a compound, found {}",
            tag_kind(&other)
        ),
    };

    Ok(Document {
        root_name,
        compound,
    })
}

fn decode_tag(path: &str, value: &Value) -> Result<NbtTag> {
    match value {
        Value::Object(object) => tag_name(object).map_or_else(
            || decode_compound(path, object).map(NbtTag::Compound),
            |tag| decode_explicit_tag(path, tag, object),
        ),
        Value::Array(values) => decode_list(path, values).map(NbtTag::List),
        Value::String(value) => Ok(NbtTag::String(value.to_owned())),
        Value::Bool(value) => Ok(NbtTag::Byte(i8::from(*value))),
        Value::Number(value) => decode_implicit_number(path, value),
        Value::Null => bail!("{path}: NBT has no null tag"),
    }
}

fn decode_explicit_tag(path: &str, tag: &str, object: &Map<String, Value>) -> Result<NbtTag> {
    ensure!(
        tag != "root",
        "{path}: NBT root wrapper is only valid at the document root"
    );

    let value = tagged_value(object).with_context(|| format!("{path}: explicit {tag} tag"))?;

    match tag {
        "byte" => integer_tag(path, tag, value, i8::MIN.into(), i8::MAX.into(), |value| {
            Ok(NbtTag::Byte(i8::try_from(value)?))
        }),
        "short" => integer_tag(
            path,
            tag,
            value,
            i16::MIN.into(),
            i16::MAX.into(),
            |value| Ok(NbtTag::Short(i16::try_from(value)?)),
        ),
        "int" => integer_tag(
            path,
            tag,
            value,
            i32::MIN.into(),
            i32::MAX.into(),
            |value| Ok(NbtTag::Int(i32::try_from(value)?)),
        ),
        "long" => integer_tag(path, tag, value, i64::MIN, i64::MAX, |value| {
            Ok(NbtTag::Long(value))
        }),
        "float" => float_tag(path, tag, value, |value| nbt_float(path, tag, value)),
        "double" => float_tag(path, tag, value, |value| Ok(NbtTag::Double(value))),
        "string" => value
            .as_str()
            .map(|value| NbtTag::String(value.to_owned()))
            .ok_or_else(|| anyhow!("{path}: string tag requires a string value")),
        "byteArray" => {
            integer_array_tag(path, tag, value, i8::MIN.into(), i8::MAX.into(), |values| {
                let values: Result<Vec<_>, _> = values.into_iter().map(i8::try_from).collect();

                Ok(NbtTag::ByteArray(values?))
            })
        }
        "intArray" => integer_array_tag(
            path,
            tag,
            value,
            i32::MIN.into(),
            i32::MAX.into(),
            |values| {
                let values: Result<Vec<_>, _> = values.into_iter().map(i32::try_from).collect();

                Ok(NbtTag::IntArray(values?))
            },
        ),
        "longArray" => integer_array_tag(path, tag, value, i64::MIN, i64::MAX, |values| {
            Ok(NbtTag::LongArray(values))
        }),
        "list" => value
            .as_array()
            .ok_or_else(|| anyhow!("{path}: list tag requires an array value"))
            .and_then(|values| decode_list(path, values).map(NbtTag::List)),
        "compound" => value
            .as_object()
            .ok_or_else(|| anyhow!("{path}: compound tag requires an object value"))
            .and_then(|object| decode_compound(path, object).map(NbtTag::Compound)),
        _ => bail!("{path}: unsupported NBT tag type {tag:?}"),
    }
}

fn decode_compound(path: &str, object: &Map<String, Value>) -> Result<NbtCompound> {
    let mut compound = NbtCompound::new();

    for (key, value) in object {
        let child_path = format!("{path}.{key}");
        compound.insert(key.to_owned(), decode_tag(&child_path, value)?);
    }

    Ok(compound)
}

fn decode_list(path: &str, values: &[Value]) -> Result<NbtList> {
    let mut list = NbtList::with_capacity(values.len());
    let mut element_kind = None;

    for (index, value) in values.iter().enumerate() {
        let child_path = format!("{path}[{index}]");
        let tag = decode_tag(&child_path, value)?;
        let current_kind = tag_kind(&tag);

        if let Some(expected_kind) = element_kind {
            ensure!(
                current_kind == expected_kind,
                "{child_path}: NBT lists must be homogeneous; expected {expected_kind}, found {current_kind}"
            );
        } else {
            element_kind = Some(current_kind);
        }

        list.push(tag);
    }

    Ok(list)
}

fn decode_implicit_number(path: &str, value: &Number) -> Result<NbtTag> {
    if value.is_f64() {
        return value
            .as_f64()
            .filter(|value| value.is_finite())
            .map(NbtTag::Double)
            .ok_or_else(|| anyhow!("{path}: floating-point number must be finite"));
    }

    let value = as_i64(path, "number", &Value::Number(value.clone()))?;
    let tag = i32::try_from(value).map_or_else(|_| NbtTag::Long(value), NbtTag::Int);

    Ok(tag)
}

fn integer_tag(
    path: &str,
    tag: &str,
    value: &Value,
    min: i64,
    max: i64,
    build: impl FnOnce(i64) -> Result<NbtTag>,
) -> Result<NbtTag> {
    let value = ranged_i64(path, tag, value, min, max)?;

    build(value)
}

fn integer_array_tag(
    path: &str,
    tag: &str,
    value: &Value,
    min: i64,
    max: i64,
    build: impl FnOnce(Vec<i64>) -> Result<NbtTag>,
) -> Result<NbtTag> {
    let values = value
        .as_array()
        .ok_or_else(|| anyhow!("{path}: {tag} tag requires an array value"))?;
    let mut out = Vec::with_capacity(values.len());

    for (index, value) in values.iter().enumerate() {
        out.push(ranged_i64(
            &format!("{path}[{index}]"),
            tag,
            value,
            min,
            max,
        )?);
    }

    build(out)
}

fn float_tag(
    path: &str,
    tag: &str,
    value: &Value,
    build: impl FnOnce(f64) -> Result<NbtTag>,
) -> Result<NbtTag> {
    let value = value
        .as_f64()
        .filter(|value| value.is_finite())
        .ok_or_else(|| anyhow!("{path}: {tag} tag requires a finite numeric value"))?;

    build(value)
}

#[allow(clippy::cast_possible_truncation)]
fn nbt_float(path: &str, tag: &str, value: f64) -> Result<NbtTag> {
    ensure!(
        (f64::from(f32::MIN)..=f64::from(f32::MAX)).contains(&value),
        "{path}: {tag} value {value} is outside the finite 32-bit float range"
    );

    Ok(NbtTag::Float(value as f32))
}

fn ranged_i64(path: &str, tag: &str, value: &Value, min: i64, max: i64) -> Result<i64> {
    let value = as_i64(path, tag, value)?;
    ensure!(
        (min..=max).contains(&value),
        "{path}: {tag} value {value} is outside {min}..={max}"
    );

    Ok(value)
}

fn as_i64(path: &str, tag: &str, value: &Value) -> Result<i64> {
    match value {
        Value::Number(number) => {
            if let Some(value) = number.as_i64() {
                Ok(value)
            } else if let Some(value) = number.as_u64() {
                i64::try_from(value).with_context(|| {
                    format!("{path}: {tag} value {value} is too large for signed NBT")
                })
            } else {
                bail!("{path}: {tag} tag requires an integer value")
            }
        }
        _ => bail!("{path}: {tag} tag requires an integer value"),
    }
}

fn tag_name(object: &Map<String, Value>) -> Option<&str> {
    object.get(TAG_KEY).and_then(Value::as_str)
}

fn tagged_value(object: &Map<String, Value>) -> Result<&Value> {
    object
        .get(VALUE_KEY)
        .ok_or_else(|| anyhow!("missing {VALUE_KEY:?}"))
}

const fn tag_kind(tag: &NbtTag) -> &'static str {
    match tag {
        NbtTag::Byte(_) => "byte",
        NbtTag::Short(_) => "short",
        NbtTag::Int(_) => "int",
        NbtTag::Long(_) => "long",
        NbtTag::Float(_) => "float",
        NbtTag::Double(_) => "double",
        NbtTag::ByteArray(_) => "byteArray",
        NbtTag::String(_) => "string",
        NbtTag::List(_) => "list",
        NbtTag::Compound(_) => "compound",
        NbtTag::IntArray(_) => "intArray",
        NbtTag::LongArray(_) => "longArray",
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn decodes_explicit_minecraft_tag_types() {
        let document = decode_document(
            &json!({
                "__minecraftNbt": "root",
                "name": "ix",
                "value": {
                    "name": { "__minecraftNbt": "string", "value": "spawn" },
                    "health": { "__minecraftNbt": "short", "value": 20 },
                    "position": {
                        "__minecraftNbt": "list",
                        "value": [
                            { "__minecraftNbt": "double", "value": 1.5 },
                            { "__minecraftNbt": "double", "value": 65.0 },
                            { "__minecraftNbt": "double", "value": -30.25 }
                        ]
                    },
                    "flags": { "__minecraftNbt": "byteArray", "value": [1, 0, -1] }
                }
            }),
            "",
        )
        .unwrap();

        assert_eq!(document.root_name, "ix");
        assert!(matches!(document.compound.get::<_, i16>("health"), Ok(20)));
        assert!(matches!(
            document.compound.get::<_, &str>("name"),
            Ok("spawn")
        ));
    }

    #[test]
    fn rejects_non_compound_roots() {
        let error = decode_document(&json!({ "__minecraftNbt": "int", "value": 1 }), "")
            .unwrap_err()
            .to_string();

        assert!(error.contains("document root must be a compound"));
    }

    #[test]
    fn rejects_mixed_lists() {
        let error = decode_document(&json!({ "mixed": [1, "two"] }), "")
            .unwrap_err()
            .to_string();

        assert!(error.contains("NBT lists must be homogeneous"));
    }

    #[test]
    fn rejects_out_of_range_integer_tags() {
        let error = decode_document(
            &json!({
                "tooLarge": { "__minecraftNbt": "byte", "value": 256 }
            }),
            "",
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("outside -128..=127"));
    }
}
