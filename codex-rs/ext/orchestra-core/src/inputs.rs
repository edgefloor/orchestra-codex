use crate::InputDefinition;
use crate::InputKind;
use serde_json::Map;
use serde_json::Value;
use sha2::Digest;
use sha2::Sha256;
use std::collections::BTreeMap;
use thiserror::Error;

pub type RunInputs = BTreeMap<String, Value>;
pub type TemplateOutputs = BTreeMap<String, BTreeMap<String, Value>>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedInputs {
    pub values: RunInputs,
    pub sha256: String,
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum InputError {
    #[error("run inputs must be a JSON object")]
    NotObject,
    #[error("unknown run input `{0}`")]
    Unknown(String),
    #[error("missing required run input `{0}`")]
    Missing(String),
    #[error("run input `{name}` must be {expected}, found {actual}")]
    WrongType {
        name: String,
        expected: &'static str,
        actual: &'static str,
    },
    #[error("unknown template reference `{0}`")]
    UnknownReference(String),
    #[error("unterminated template reference")]
    UnterminatedReference,
    #[error("unsupported template reference `{0}`")]
    UnsupportedReference(String),
    #[error("resolved input digest mismatch: expected {expected}, found {actual}")]
    DigestMismatch { expected: String, actual: String },
    #[error("changed run inputs: {names}; omit `inputs` to resume with the stored values")]
    Changed { names: String },
    #[error("resolved inputs differ between the checkpoint and inputs.json")]
    SnapshotMismatch,
}

pub fn resolve_inputs(
    definitions: &BTreeMap<String, InputDefinition>,
    provided: Option<&Value>,
) -> Result<ResolvedInputs, InputError> {
    let empty = Map::new();
    let provided = match provided {
        None => &empty,
        Some(Value::Object(values)) => values,
        Some(_) => return Err(InputError::NotObject),
    };
    for name in provided.keys() {
        if !definitions.contains_key(name) {
            return Err(InputError::Unknown(name.clone()));
        }
    }

    let mut values = RunInputs::new();
    for (name, definition) in definitions {
        let value = provided
            .get(name)
            .or_else(|| definition.default.value())
            .cloned();
        match value {
            Some(value) => {
                validate_value(name, &definition.kind, &value)?;
                values.insert(name.clone(), canonicalize(value));
            }
            None if definition.required => return Err(InputError::Missing(name.clone())),
            None => {}
        }
    }
    Ok(resolved(values))
}

pub fn verify_inputs(values: &RunInputs, expected_sha256: &str) -> Result<(), InputError> {
    let actual = digest(values);
    if actual == expected_sha256 {
        Ok(())
    } else {
        Err(InputError::DigestMismatch {
            expected: expected_sha256.into(),
            actual,
        })
    }
}

pub fn resolve_template(
    template: &str,
    inputs: &RunInputs,
    step_outputs: &TemplateOutputs,
) -> Result<String, InputError> {
    let mut result = String::new();
    let mut rest = template;
    while let Some(start) = rest.find("${") {
        result.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            return Err(InputError::UnterminatedReference);
        };
        let reference = &after[..end];
        match reference.split('.').collect::<Vec<_>>().as_slice() {
            ["inputs", name] if !name.is_empty() => {
                let value = inputs
                    .get(*name)
                    .ok_or_else(|| InputError::UnknownReference((*name).into()))?;
                result.push_str(&render_value(value));
            }
            ["steps", step, "outputs", output] if !step.is_empty() && !output.is_empty() => {
                let value = step_outputs
                    .get(*step)
                    .and_then(|outputs| outputs.get(*output))
                    .ok_or_else(|| InputError::UnknownReference(reference.into()))?;
                result.push_str(&render_value(value));
            }
            _ => return Err(InputError::UnsupportedReference(reference.into())),
        }
        rest = &after[end + 1..];
    }
    result.push_str(rest);
    Ok(result)
}

pub fn render_value(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        _ => serde_json::to_string(value).expect("JSON value serializes"),
    }
}

fn resolved(values: RunInputs) -> ResolvedInputs {
    let sha256 = digest(&values);
    ResolvedInputs { values, sha256 }
}

fn digest(values: &RunInputs) -> String {
    let bytes = serde_json::to_vec(values).expect("run inputs serialize");
    format!("{:x}", Sha256::digest(bytes))
}

fn canonicalize(value: Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.into_iter().map(canonicalize).collect()),
        Value::Object(values) => Value::Object(
            values
                .into_iter()
                .map(|(key, value)| (key, canonicalize(value)))
                .collect::<BTreeMap<_, _>>()
                .into_iter()
                .collect(),
        ),
        value => value,
    }
}

pub fn value_matches(kind: &InputKind, value: &Value) -> bool {
    match kind {
        InputKind::String => value.is_string(),
        InputKind::Number => value.is_number(),
        InputKind::Boolean => value.is_boolean(),
        InputKind::Object => value.is_object(),
        InputKind::Array => value.is_array(),
        InputKind::Json => true,
    }
}

fn validate_value(name: &str, kind: &InputKind, value: &Value) -> Result<(), InputError> {
    if value_matches(kind, value) {
        Ok(())
    } else {
        Err(InputError::WrongType {
            name: name.into(),
            expected: kind_name(kind),
            actual: value_name(value),
        })
    }
}

pub fn kind_name(kind: &InputKind) -> &'static str {
    match kind {
        InputKind::String => "a string",
        InputKind::Number => "a number",
        InputKind::Boolean => "a boolean",
        InputKind::Object => "an object",
        InputKind::Array => "an array",
        InputKind::Json => "JSON",
    }
}

fn value_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::InputDefault;

    fn definition(kind: InputKind, required: bool, default: InputDefault) -> InputDefinition {
        InputDefinition {
            kind,
            required,
            default,
        }
    }

    #[test]
    fn resolves_required_default_optional_and_nested_canonical_values() {
        let definitions = BTreeMap::from([
            (
                "ticket".into(),
                definition(InputKind::String, true, InputDefault::Missing),
            ),
            (
                "base".into(),
                definition(
                    InputKind::String,
                    true,
                    InputDefault::Value(Value::String("main".into())),
                ),
            ),
            (
                "notes".into(),
                definition(InputKind::Object, false, InputDefault::Missing),
            ),
        ]);
        let one = resolve_inputs(
            &definitions,
            Some(&serde_json::json!({"ticket":"#3","notes":{"z":1,"a":{"y":2,"b":3}}})),
        )
        .unwrap();
        let two = resolve_inputs(
            &definitions,
            Some(&serde_json::json!({"notes":{"a":{"b":3,"y":2},"z":1},"ticket":"#3"})),
        )
        .unwrap();
        assert_eq!(one, two);
        assert_eq!(one.values["base"], "main");
    }

    #[test]
    fn rejects_missing_unknown_and_wrong_typed_values() {
        let definitions = BTreeMap::from([(
            "ticket".into(),
            definition(InputKind::String, true, InputDefault::Missing),
        )]);
        assert!(matches!(
            resolve_inputs(&definitions, None),
            Err(InputError::Missing(name)) if name == "ticket"
        ));
        assert!(matches!(
            resolve_inputs(&definitions, Some(&serde_json::json!({"other": 1}))),
            Err(InputError::Unknown(name)) if name == "other"
        ));
        assert!(matches!(
            resolve_inputs(&definitions, Some(&serde_json::json!({"ticket": 3}))),
            Err(InputError::WrongType { name, .. }) if name == "ticket"
        ));
    }

    #[test]
    fn resolves_input_templates_and_preserves_reserved_output_templates() {
        let inputs = BTreeMap::from([
            ("ticket".into(), Value::String("#3".into())),
            ("flags".into(), serde_json::json!(["one", "two"])),
        ]);
        assert_eq!(
            resolve_template(
                "Implement ${inputs.ticket} with ${inputs.flags}: ${steps.plan.outputs.summary}",
                &inputs,
                &BTreeMap::from([(
                    "plan".into(),
                    BTreeMap::from([("summary".into(), Value::String("ready".into()))]),
                )]),
            )
            .unwrap(),
            "Implement #3 with [\"one\",\"two\"]: ready"
        );
    }
}
