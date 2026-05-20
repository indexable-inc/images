//! Property tests for the JSON->NBT decoder.
//!
//! These feed an arbitrary `serde_json::Value` tree (recursively constructed
//! by Hegel) into [`minecraft_nbt::decode_document`] and assert that the
//! decoder never panics. It is allowed to return `Err`; what must not happen
//! is a panic, integer overflow, or stack overflow on a hostile input.

use hegel::TestCase;
use hegel::generators::{self as gs, Generator};
use minecraft_nbt::decode_document;
use serde_json::{Map, Number, Value};

fn json_value() -> impl Generator<Value> {
    let value = gs::deferred::<Value>();
    let handle = value.generator();

    let leaf = hegel::one_of!(
        gs::just(Value::Null),
        gs::booleans().map(Value::Bool),
        gs::integers::<i64>().map(|n| Value::Number(n.into())),
        gs::floats::<f64>().map(|n| Number::from_f64(n).map_or(Value::Null, Value::Number)),
        gs::text().map(Value::String),
    );

    let array = gs::vecs(value.generator())
        .max_size(8)
        .map(Value::Array);

    let object = gs::vecs(hegel::tuples!(gs::text(), value.generator()))
        .max_size(8)
        .map(|entries| {
            let mut map = Map::new();
            for (key, child) in entries {
                map.insert(key, child);
            }
            Value::Object(map)
        });

    value.set(hegel::one_of!(leaf, array, object));
    handle
}

#[hegel::test]
fn decode_document_never_panics_on_arbitrary_json(tc: TestCase) {
    let value = tc.draw(json_value());
    let _ = decode_document(&value, "ix");
}

#[hegel::test]
fn explicit_byte_tag_round_trips_in_range(tc: TestCase) {
    let byte: i64 = tc.draw(gs::integers::<i64>());

    let value = serde_json::json!({
        "value": { "__minecraftNbt": "byte", "value": byte }
    });

    match decode_document(&value, "ix") {
        Ok(document) => {
            let stored: i8 = document.compound.get("value").expect("byte tag present");
            assert!(
                (i64::from(i8::MIN)..=i64::from(i8::MAX)).contains(&byte),
                "decoder accepted out-of-range byte {byte}"
            );
            assert_eq!(i64::from(stored), byte);
        }
        Err(_) => assert!(
            !(i64::from(i8::MIN)..=i64::from(i8::MAX)).contains(&byte),
            "decoder rejected in-range byte {byte}"
        ),
    }
}
