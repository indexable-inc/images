//! The introspection half of the public boundary: the type description a
//! module yields, and the schema decisions the golden files show without
//! explaining.

use ix2nix::ty::{Field, Parameter, Ty};
use ix2nix::{convert, schema, types};

fn parameters(source: &str) -> Vec<Parameter> {
    types(source).expect("source should convert").parameters
}

#[test]
fn default_export_parameters_are_recorded_in_call_order() {
    let module = types("type P = int;\nexport default (a: P, b) => a;\n").expect("converts");
    assert_eq!(module.aliases.len(), 1, "{module:?}");
    assert_eq!(module.aliases[0].name, "P");
    assert_eq!(module.aliases[0].ty, Ty::Int);
    assert_eq!(
        module.parameters,
        vec![
            Parameter {
                name: Some("a".to_owned()),
                ty: Some(Ty::Alias("P".to_owned())),
            },
            Parameter {
                name: Some("b".to_owned()),
                ty: None,
            },
        ]
    );
}

#[test]
fn a_destructured_parameter_has_no_name_but_keeps_its_fields() {
    let parameters = parameters("export default ({ a }: { a: int }) => a;\n");
    assert_eq!(parameters.len(), 1, "{parameters:?}");
    assert_eq!(parameters[0].name, None);
    assert_eq!(
        parameters[0].ty,
        Some(Ty::Object(vec![Field {
            name: "a".to_owned(),
            ty: Ty::Int,
            optional: false,
        }]))
    );
}

#[test]
fn an_inner_helpers_parameters_are_not_the_modules() {
    // Only the default export describes what a caller of the module can pass.
    let parameters = parameters("const f = (n: int) => n;\nexport default f(1);\n");
    assert!(parameters.is_empty(), "{parameters:?}");
}

#[test]
fn a_module_with_no_annotated_parameter_gets_an_unconstrained_root() {
    // Neither `false` nor "takes nothing": an empty schema says only that
    // nothing is known about the argument, which is the truth here.
    assert_eq!(
        schema("export default 1;\n").expect("converts"),
        "{\n  \"$schema\": \"https://json-schema.org/draft/2020-12/schema\"\n}\n"
    );
}

#[test]
fn drv_and_function_fields_validate_against_nothing() {
    let document = schema("export default (p: { d: drv; f: (a: int) => int }) => p;\n")
        .expect("converts");
    assert_eq!(document.matches("\"not\": {}").count(), 2, "{document}");
    assert!(
        document.contains("a derivation exists only during evaluation"),
        "{document}"
    );
}

#[test]
fn an_object_schema_permits_extra_fields_because_the_checker_does() {
    // `ix-ty.nix`'s `attrs` checker reads required field names and allows
    // extras, so the schema must not close the object.
    let document = schema("export default (p: { a: int }) => p;\n").expect("converts");
    assert!(!document.contains("additionalProperties"), "{document}");
}

#[test]
fn a_float_field_widens_to_number_and_says_so_nowhere_else() {
    // JSON has one number type and draft 2020-12 counts `2.0` as an integer,
    // so this schema is looser than `__ixTy.float`. Pinned because a future
    // tightening would reject `2.0`, a valid Nix float.
    let document = schema("export default (p: { r: float }) => p;\n").expect("converts");
    assert!(document.contains("\"type\": \"number\""), "{document}");
    assert!(!document.contains("\"not\""), "{document}");
}

#[test]
fn an_alias_annotation_becomes_a_resolvable_ref() {
    // The shape `properties.rs` checks resolution of. Pinned here because that
    // property iterates the refs it finds, so it would pass on a schema with
    // none.
    let document = schema("type P = { a: int };\nexport default (p: P) => p;\n").expect("converts");
    assert!(document.contains("\"$ref\": \"#/$defs/P\""), "{document}");
    assert!(document.contains("\"P\": {"), "{document}");
}

#[test]
fn a_parenthesized_default_export_keeps_its_signature() {
    // `((a: T) => e)` is the same function as `(a: T) => e`, so the parentheses
    // must not lose the parameter the schema is built from.
    for source in [
        "export default (a: int) => a;\n",
        "export default ((a: int) => a);\n",
    ] {
        assert_eq!(
            parameters(source),
            vec![Parameter {
                name: Some("a".to_owned()),
                ty: Some(Ty::Int),
            }],
            "{source}"
        );
    }
}

#[test]
fn a_destructured_optional_field_is_optional_in_both_outputs() {
    // The defect this pins: the schema read the annotation's `?` while Nix read
    // the pattern's default, so a field could be reported optional and be
    // mandatory, and a params.json omitting it validated and then failed at
    // eval. The converter now refuses the mismatched spelling, which is what
    // makes reading `required` off the annotation sound.
    let source = "export default ({ a, b = null }: { a: int; b?: string }) => a;\n";

    let nix = convert(source).expect("converts");
    assert!(nix.contains("{ a, b ? null }"), "{nix}");

    let document: serde_json::Value =
        serde_json::from_str(&schema(source).expect("converts")).expect("schema is JSON");
    let required = document["required"].as_array().expect("required is a list");
    assert_eq!(required, &[serde_json::json!("a")], "{document:#}");
    assert!(document["properties"]["b"].is_object(), "{document:#}");
}
