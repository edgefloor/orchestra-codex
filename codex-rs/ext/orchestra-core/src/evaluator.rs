use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use sha2::Digest;
use sha2::Sha256;
use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use thiserror::Error;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct EvaluatorLimits {
    pub request_bytes: usize,
    pub response_bytes: usize,
    pub bundle_bytes: usize,
    pub canonical_value_bytes: usize,
    pub wall_time_ms: u64,
    pub value_depth: usize,
    pub value_nodes: usize,
    pub collection_entries: usize,
    pub string_bytes: usize,
    pub issue_count: usize,
    pub issue_text_bytes: usize,
}

impl Default for EvaluatorLimits {
    fn default() -> Self {
        Self {
            request_bytes: 2 * 1024 * 1024,
            response_bytes: 2 * 1024 * 1024,
            bundle_bytes: 1024 * 1024,
            canonical_value_bytes: 1024 * 1024,
            wall_time_ms: 1_000,
            value_depth: 64,
            value_nodes: 100_000,
            collection_entries: 10_000,
            string_bytes: 256 * 1024,
            issue_count: 128,
            issue_text_bytes: 512,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Evaluator {
    executable: PathBuf,
    evaluator_revision: String,
    limits: EvaluatorLimits,
}

impl Evaluator {
    pub fn new(
        executable: impl Into<PathBuf>,
        evaluator_revision: impl Into<String>,
        limits: EvaluatorLimits,
    ) -> Self {
        Self {
            executable: executable.into(),
            evaluator_revision: evaluator_revision.into(),
            limits,
        }
    }

    pub fn executable(&self) -> &Path {
        &self.executable
    }

    /// Validate one canonical value in one fresh worker process.
    pub async fn validate(
        &self,
        request: ValidationRequest,
    ) -> Result<ValidationOutcome, EvaluatorError> {
        if request.bundle_source.len() > self.limits.bundle_bytes {
            return Err(EvaluatorError::new(
                EvaluatorFailure::BundleTooLarge,
                "validation bundle exceeds byte limit",
            ));
        }
        let actual_hash = sha256(request.bundle_source.as_bytes());
        if actual_hash != request.bundle_hash {
            return Err(EvaluatorError::new(
                EvaluatorFailure::ProvenanceMismatch,
                "validation bundle hash does not match its recorded identity",
            ));
        }
        validate_value_shape(&request.value, &self.limits)?;
        let raw_canonical = canonical_json(&request.value)?;
        if raw_canonical.len() > self.limits.canonical_value_bytes {
            return Err(EvaluatorError::new(
                EvaluatorFailure::ValueTooLarge,
                "canonical value exceeds byte limit",
            ));
        }

        let wire_request = WorkerRequest {
            op: "validate",
            bundle_source: &request.bundle_source,
            bundle_hash: &request.bundle_hash,
            schema_id: &request.schema_id,
            value: &request.value,
            evaluator_revision: &self.evaluator_revision,
            limits: &self.limits,
        };
        let bytes = serde_json_canonicalizer::to_vec(&wire_request)
            .map_err(|error| EvaluatorError::protocol(error.to_string()))?;
        if bytes.len() > self.limits.request_bytes {
            return Err(EvaluatorError::new(
                EvaluatorFailure::RequestTooLarge,
                "validation request exceeds byte limit",
            ));
        }

        let output = self.invoke(&bytes).await?;
        let response: WorkerResponse = serde_json::from_slice(&output).map_err(|error| {
            EvaluatorError::protocol(format!("invalid worker response: {error}"))
        })?;
        if !response.ok {
            return Err(EvaluatorError::protocol(
                "worker response did not carry a successful envelope",
            ));
        }
        let outcome = response.value;
        let provenance = outcome.provenance();
        if provenance.bundle_hash != request.bundle_hash
            || provenance.evaluator_revision != self.evaluator_revision
        {
            return Err(EvaluatorError::new(
                EvaluatorFailure::ProvenanceMismatch,
                "worker response provenance does not match the request tuple",
            ));
        }
        if outcome.raw_canonical() != raw_canonical {
            return Err(EvaluatorError::new(
                EvaluatorFailure::Nondeterministic,
                "worker and host canonical raw values differ",
            ));
        }
        validate_outcome(&outcome, &self.limits)?;
        Ok(outcome)
    }

    async fn invoke(&self, request: &[u8]) -> Result<Vec<u8>, EvaluatorError> {
        let mut child = Command::new(&self.executable)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("LANG", "C")
            .env("TMPDIR", "/tmp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| {
                EvaluatorError::new(
                    EvaluatorFailure::Spawn,
                    format!("failed to start validation worker: {error}"),
                )
            })?;

        let mut stdin = child.stdin.take().expect("piped stdin");
        let request = request.to_vec();
        let writer = tokio::spawn(async move {
            stdin.write_all(&request).await?;
            stdin.shutdown().await
        });

        let stdout = child.stdout.take().expect("piped stdout");
        let response_limit = self.limits.response_bytes as u64 + 1;
        let stdout_reader = tokio::spawn(async move {
            let mut bytes = Vec::new();
            stdout.take(response_limit).read_to_end(&mut bytes).await?;
            Ok::<_, std::io::Error>(bytes)
        });

        let stderr = child.stderr.take().expect("piped stderr");
        let stderr_reader = tokio::spawn(async move {
            let mut bytes = Vec::new();
            stderr.take(4097).read_to_end(&mut bytes).await?;
            Ok::<_, std::io::Error>(bytes)
        });

        let status = match tokio::time::timeout(
            Duration::from_millis(self.limits.wall_time_ms),
            child.wait(),
        )
        .await
        {
            Ok(result) => result.map_err(|error| {
                EvaluatorError::new(
                    EvaluatorFailure::Crash,
                    format!("failed while waiting for validation worker: {error}"),
                )
            })?,
            Err(_) => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                writer.abort();
                stdout_reader.abort();
                stderr_reader.abort();
                return Err(EvaluatorError::new(
                    EvaluatorFailure::Timeout,
                    "validation worker exceeded its wall-time limit",
                ));
            }
        };

        writer
            .await
            .map_err(|error| {
                EvaluatorError::protocol(format!("worker stdin task failed: {error}"))
            })?
            .map_err(|error| EvaluatorError::protocol(format!("worker stdin failed: {error}")))?;
        let stdout = stdout_reader
            .await
            .map_err(|error| {
                EvaluatorError::protocol(format!("worker stdout task failed: {error}"))
            })?
            .map_err(|error| EvaluatorError::protocol(format!("worker stdout failed: {error}")))?;
        let stderr = stderr_reader
            .await
            .map_err(|error| {
                EvaluatorError::protocol(format!("worker stderr task failed: {error}"))
            })?
            .map_err(|error| EvaluatorError::protocol(format!("worker stderr failed: {error}")))?;

        if stdout.len() > self.limits.response_bytes {
            return Err(EvaluatorError::new(
                EvaluatorFailure::ResponseTooLarge,
                "validation response exceeds byte limit",
            ));
        }
        if !status.success() {
            let detail = bounded_diagnostic(&stderr, self.limits.issue_text_bytes);
            let failure = if status.code().is_none() {
                EvaluatorFailure::Crash
            } else {
                EvaluatorFailure::WorkerFailure
            };
            return Err(EvaluatorError::new(
                failure,
                if detail.is_empty() {
                    "validation worker failed without diagnostics".into()
                } else {
                    detail
                },
            ));
        }
        if stdout.is_empty() {
            return Err(EvaluatorError::protocol(
                "validation worker returned no protocol response",
            ));
        }
        Ok(stdout)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ValidationRequest {
    pub bundle_source: String,
    pub bundle_hash: String,
    pub schema_id: String,
    pub value: Value,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct EvaluatorProvenance {
    pub bundle_hash: String,
    pub evaluator_revision: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ValidationIssue {
    pub code: String,
    pub path: Vec<ValuePathSegment>,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(untagged)]
pub enum ValuePathSegment {
    Key(String),
    Index(u64),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ValidationOutcome {
    Accepted {
        provenance: EvaluatorProvenance,
        raw_canonical: String,
        transformed_canonical: String,
    },
    Rejected {
        provenance: EvaluatorProvenance,
        raw_canonical: String,
        issues: Vec<ValidationIssue>,
    },
}

impl ValidationOutcome {
    pub fn provenance(&self) -> &EvaluatorProvenance {
        match self {
            Self::Accepted { provenance, .. } | Self::Rejected { provenance, .. } => provenance,
        }
    }

    pub fn raw_canonical(&self) -> &str {
        match self {
            Self::Accepted { raw_canonical, .. } | Self::Rejected { raw_canonical, .. } => {
                raw_canonical
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EvaluatorFailure {
    Spawn,
    RequestTooLarge,
    ResponseTooLarge,
    BundleTooLarge,
    ValueTooLarge,
    Timeout,
    Crash,
    WorkerFailure,
    Protocol,
    ProvenanceMismatch,
    Nondeterministic,
    Noncanonical,
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
#[error("{kind:?}: {message}")]
pub struct EvaluatorError {
    pub kind: EvaluatorFailure,
    pub message: String,
}

impl EvaluatorError {
    fn new(kind: EvaluatorFailure, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    fn protocol(message: impl Into<String>) -> Self {
        Self::new(EvaluatorFailure::Protocol, message)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkerRequest<'a> {
    op: &'static str,
    bundle_source: &'a str,
    bundle_hash: &'a str,
    schema_id: &'a str,
    value: &'a Value,
    evaluator_revision: &'a str,
    limits: &'a EvaluatorLimits,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerResponse {
    ok: bool,
    value: ValidationOutcome,
}

pub fn canonical_json(value: &Value) -> Result<String, EvaluatorError> {
    serde_json_canonicalizer::to_string(value).map_err(|error| {
        EvaluatorError::new(
            EvaluatorFailure::Noncanonical,
            format!("value is not canonical JSON: {error}"),
        )
    })
}

pub fn canonical_sha256(value: &Value) -> Result<String, EvaluatorError> {
    Ok(sha256(canonical_json(value)?.as_bytes()))
}

fn validate_outcome(
    outcome: &ValidationOutcome,
    limits: &EvaluatorLimits,
) -> Result<(), EvaluatorError> {
    match outcome {
        ValidationOutcome::Accepted {
            transformed_canonical,
            ..
        } => {
            if transformed_canonical.len() > limits.canonical_value_bytes {
                return Err(EvaluatorError::new(
                    EvaluatorFailure::ValueTooLarge,
                    "transformed canonical value exceeds byte limit",
                ));
            }
            let value: Value = serde_json::from_str(transformed_canonical).map_err(|error| {
                EvaluatorError::new(
                    EvaluatorFailure::Noncanonical,
                    format!("transformed value is not JSON: {error}"),
                )
            })?;
            validate_value_shape(&value, limits)?;
            if canonical_json(&value)? != *transformed_canonical {
                return Err(EvaluatorError::new(
                    EvaluatorFailure::Noncanonical,
                    "transformed value is not in canonical form",
                ));
            }
        }
        ValidationOutcome::Rejected { issues, .. } => {
            if issues.len() > limits.issue_count {
                return Err(EvaluatorError::protocol(
                    "worker returned more validation issues than allowed",
                ));
            }
            for issue in issues {
                if issue.message.len() > limits.issue_text_bytes {
                    return Err(EvaluatorError::protocol(
                        "worker returned an oversized validation issue",
                    ));
                }
            }
            let mut sorted = issues.clone();
            sorted.sort_by_cached_key(|issue| {
                serde_json_canonicalizer::to_string(issue).unwrap_or_default()
            });
            if sorted != *issues {
                return Err(EvaluatorError::new(
                    EvaluatorFailure::Nondeterministic,
                    "validation issues are not in canonical order",
                ));
            }
        }
    }
    Ok(())
}

fn validate_value_shape(value: &Value, limits: &EvaluatorLimits) -> Result<(), EvaluatorError> {
    let mut nodes = 0usize;
    let mut pending = vec![(value, 0usize)];
    while let Some((value, depth)) = pending.pop() {
        nodes = nodes.saturating_add(1);
        if nodes > limits.value_nodes {
            return Err(EvaluatorError::new(
                EvaluatorFailure::ValueTooLarge,
                "canonical value exceeds node limit",
            ));
        }
        if depth > limits.value_depth {
            return Err(EvaluatorError::new(
                EvaluatorFailure::ValueTooLarge,
                "canonical value exceeds depth limit",
            ));
        }
        match value {
            Value::String(value) if value.len() > limits.string_bytes => {
                return Err(EvaluatorError::new(
                    EvaluatorFailure::ValueTooLarge,
                    "canonical string exceeds byte limit",
                ));
            }
            Value::Array(values) => {
                if values.len() > limits.collection_entries {
                    return Err(EvaluatorError::new(
                        EvaluatorFailure::ValueTooLarge,
                        "canonical array exceeds entry limit",
                    ));
                }
                pending.extend(values.iter().map(|value| (value, depth + 1)));
            }
            Value::Object(values) => {
                if values.len() > limits.collection_entries {
                    return Err(EvaluatorError::new(
                        EvaluatorFailure::ValueTooLarge,
                        "canonical object exceeds key limit",
                    ));
                }
                if values.keys().any(|key| key.len() > limits.string_bytes) {
                    return Err(EvaluatorError::new(
                        EvaluatorFailure::ValueTooLarge,
                        "canonical object key exceeds byte limit",
                    ));
                }
                pending.extend(values.values().map(|value| (value, depth + 1)));
            }
            _ => {}
        }
    }
    Ok(())
}

fn bounded_diagnostic(bytes: &[u8], limit: usize) -> String {
    String::from_utf8_lossy(&bytes[..bytes.len().min(limit)])
        .trim()
        .to_owned()
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_json_uses_jcs_number_and_key_rules() {
        let value: Value = serde_json::from_str(r#"{"z":1.0,"a":1e-7}"#).unwrap();
        assert_eq!(canonical_json(&value).unwrap(), r#"{"a":1e-7,"z":1}"#);
    }

    #[tokio::test]
    async fn rejects_bundle_provenance_before_spawning() {
        let evaluator = Evaluator::new(
            "/missing/worker",
            "evaluator-v1",
            EvaluatorLimits::default(),
        );
        let error = evaluator
            .validate(ValidationRequest {
                bundle_source: "({output:z.string()})".into(),
                bundle_hash: "0".repeat(64),
                schema_id: "output".into(),
                value: Value::String("ok".into()),
            })
            .await
            .unwrap_err();
        assert_eq!(error.kind, EvaluatorFailure::ProvenanceMismatch);
    }

    #[tokio::test]
    async fn request_limit_is_enforced_before_spawning() {
        let source = "({output:z.string()})";
        let limits = EvaluatorLimits {
            request_bytes: 16,
            ..EvaluatorLimits::default()
        };
        let evaluator = Evaluator::new("/missing/worker", "evaluator-v1", limits);
        let error = evaluator
            .validate(ValidationRequest {
                bundle_source: source.into(),
                bundle_hash: sha256(source.as_bytes()),
                schema_id: "output".into(),
                value: Value::String("ok".into()),
            })
            .await
            .unwrap_err();
        assert_eq!(error.kind, EvaluatorFailure::RequestTooLarge);
    }
}
