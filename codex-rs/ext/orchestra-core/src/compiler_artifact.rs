use crate::ExecutionPlan;
use crate::compile_workflow;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use sha2::Digest;
use sha2::Sha256;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use thiserror::Error;

pub const WORKFLOW_ARTIFACT_SCHEMA: &str = "orchestra-workflow-artifact-v1";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CompatibilityTuple {
    pub product_release_id: String,
    pub agents_revision: String,
    pub zod_revision: String,
    pub evaluator_revision: String,
    pub adapter_abi: String,
    pub canonicalizer: String,
    pub issue_format: String,
    pub target: String,
    pub sandbox_identity: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CompileLimits {
    pub module_count: usize,
    pub total_source_bytes: usize,
    pub module_source_bytes: usize,
    pub validation_bundle_bytes: usize,
}

impl Default for CompileLimits {
    fn default() -> Self {
        Self {
            module_count: 256,
            total_source_bytes: 4 * 1024 * 1024,
            module_source_bytes: 1024 * 1024,
            validation_bundle_bytes: 2 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CompileRequest {
    pub entry: String,
    pub modules: BTreeMap<String, String>,
    #[serde(default)]
    pub validation_bundles: BTreeMap<String, String>,
    #[serde(default)]
    pub guidance_schemas: BTreeMap<String, Value>,
    #[serde(default)]
    pub custom_codecs: Vec<CustomCodec>,
    pub compatibility: CompatibilityTuple,
    #[serde(default)]
    pub limits: CompileLimits,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CustomCodec {
    pub type_id: String,
    pub version: String,
    pub wire_schema_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ModuleIdentity {
    pub specifier: String,
    pub source: String,
    pub bytes: usize,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SchemaBundle {
    pub schema_id: String,
    pub source: String,
    pub bytes: usize,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CompiledWorkflow {
    pub schema: String,
    pub entry: String,
    pub modules: Vec<ModuleIdentity>,
    pub plan: ExecutionPlan,
    pub validation_bundles: BTreeMap<String, SchemaBundle>,
    pub guidance_schemas: BTreeMap<String, Value>,
    pub custom_codecs: Vec<CustomCodec>,
    pub compatibility: CompatibilityTuple,
    pub limits: CompileLimits,
    pub artifact_sha256: String,
}

impl CompiledWorkflow {
    pub fn verify_identity(&self) -> Result<(), ArtifactCompileError> {
        if self.schema != WORKFLOW_ARTIFACT_SCHEMA {
            return Err(ArtifactCompileError::UnsupportedSchema(self.schema.clone()));
        }
        let mut specifiers = BTreeSet::new();
        let mut entry_source = None;
        for module in &self.modules {
            if !specifiers.insert(module.specifier.as_str()) {
                return Err(ArtifactCompileError::DuplicateModule(
                    module.specifier.clone(),
                ));
            }
            let actual = sha256(module.source.as_bytes());
            if module.bytes != module.source.len() || module.sha256 != actual {
                return Err(ArtifactCompileError::SourceIdentityMismatch(
                    module.specifier.clone(),
                ));
            }
            if module.specifier == self.entry {
                entry_source = Some(module.source.as_str());
            }
        }
        let entry_source =
            entry_source.ok_or_else(|| ArtifactCompileError::MissingEntry(self.entry.clone()))?;
        if compile_workflow(entry_source)? != self.plan {
            return Err(ArtifactCompileError::PlanMismatch);
        }
        for bundle in self.validation_bundles.values() {
            let actual = sha256(bundle.source.as_bytes());
            if bundle.bytes != bundle.source.len() || bundle.sha256 != actual {
                return Err(ArtifactCompileError::BundleIdentityMismatch(
                    bundle.schema_id.clone(),
                ));
            }
        }

        let unsigned = UnsignedCompiledWorkflow {
            schema: &self.schema,
            entry: &self.entry,
            modules: &self.modules,
            plan: &self.plan,
            validation_bundles: &self.validation_bundles,
            guidance_schemas: &self.guidance_schemas,
            custom_codecs: &self.custom_codecs,
            compatibility: &self.compatibility,
            limits: &self.limits,
        };
        let canonical = serde_json_canonicalizer::to_vec(&unsigned)
            .map_err(|error| ArtifactCompileError::Canonical(error.to_string()))?;
        let actual = sha256(&canonical);
        if actual != self.artifact_sha256 {
            return Err(ArtifactCompileError::IdentityMismatch {
                expected: self.artifact_sha256.clone(),
                actual,
            });
        }
        Ok(())
    }
}

#[derive(Debug, Error, PartialEq)]
pub enum ArtifactCompileError {
    #[error("entry module `{0}` is not present in the closed source graph")]
    MissingEntry(String),
    #[error("closed source graph must contain at least one module")]
    EmptyGraph,
    #[error("module count {actual} exceeds limit {limit}")]
    ModuleCount { actual: usize, limit: usize },
    #[error("module `{specifier}` has {actual} bytes, exceeding limit {limit}")]
    ModuleBytes {
        specifier: String,
        actual: usize,
        limit: usize,
    },
    #[error("source graph has {actual} bytes, exceeding limit {limit}")]
    TotalSourceBytes { actual: usize, limit: usize },
    #[error("validation bundle `{schema_id}` has {actual} bytes, exceeding limit {limit}")]
    BundleBytes {
        schema_id: String,
        actual: usize,
        limit: usize,
    },
    #[error("duplicate custom codec identity `{0}`")]
    DuplicateCodec(String),
    #[error("compatibility field `{0}` must not be empty")]
    EmptyCompatibility(&'static str),
    #[error(transparent)]
    Workflow(#[from] crate::CompileError),
    #[error("failed to canonicalize workflow artifact: {0}")]
    Canonical(String),
    #[error("workflow artifact identity mismatch: expected {expected}, computed {actual}")]
    IdentityMismatch { expected: String, actual: String },
    #[error("source identity mismatch for module `{0}`")]
    SourceIdentityMismatch(String),
    #[error("source identity mismatch for validation bundle `{0}`")]
    BundleIdentityMismatch(String),
    #[error("unsupported workflow artifact schema `{0}`")]
    UnsupportedSchema(String),
    #[error("duplicate module `{0}` in workflow artifact")]
    DuplicateModule(String),
    #[error("lowered plan does not match the authoritative entry source")]
    PlanMismatch,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UnsignedCompiledWorkflow<'a> {
    schema: &'a str,
    entry: &'a str,
    modules: &'a [ModuleIdentity],
    plan: &'a ExecutionPlan,
    validation_bundles: &'a BTreeMap<String, SchemaBundle>,
    guidance_schemas: &'a BTreeMap<String, Value>,
    custom_codecs: &'a [CustomCodec],
    compatibility: &'a CompatibilityTuple,
    limits: &'a CompileLimits,
}

/// Compile one exact, closed source graph into a content-addressed artifact.
///
/// The entry source is parsed and lowered by Rust. None of the supplied source
/// modules or validation bundles are evaluated during compilation.
pub fn compile_artifact(request: CompileRequest) -> Result<CompiledWorkflow, ArtifactCompileError> {
    validate_compatibility(&request.compatibility)?;
    if request.modules.is_empty() {
        return Err(ArtifactCompileError::EmptyGraph);
    }
    if request.modules.len() > request.limits.module_count {
        return Err(ArtifactCompileError::ModuleCount {
            actual: request.modules.len(),
            limit: request.limits.module_count,
        });
    }

    let mut total_source_bytes = 0usize;
    let mut modules = Vec::with_capacity(request.modules.len());
    for (specifier, source) in &request.modules {
        let bytes = source.len();
        if bytes > request.limits.module_source_bytes {
            return Err(ArtifactCompileError::ModuleBytes {
                specifier: specifier.clone(),
                actual: bytes,
                limit: request.limits.module_source_bytes,
            });
        }
        total_source_bytes = total_source_bytes.saturating_add(bytes);
        modules.push(ModuleIdentity {
            specifier: specifier.clone(),
            source: source.clone(),
            bytes,
            sha256: sha256(source.as_bytes()),
        });
    }
    if total_source_bytes > request.limits.total_source_bytes {
        return Err(ArtifactCompileError::TotalSourceBytes {
            actual: total_source_bytes,
            limit: request.limits.total_source_bytes,
        });
    }

    let entry_source = request
        .modules
        .get(&request.entry)
        .ok_or_else(|| ArtifactCompileError::MissingEntry(request.entry.clone()))?;
    let plan = compile_workflow(entry_source)?;

    let mut validation_bundles = BTreeMap::new();
    for (schema_id, source) in request.validation_bundles {
        let bytes = source.len();
        if bytes > request.limits.validation_bundle_bytes {
            return Err(ArtifactCompileError::BundleBytes {
                schema_id,
                actual: bytes,
                limit: request.limits.validation_bundle_bytes,
            });
        }
        validation_bundles.insert(
            schema_id.clone(),
            SchemaBundle {
                schema_id,
                bytes,
                sha256: sha256(source.as_bytes()),
                source,
            },
        );
    }

    let mut custom_codecs = request.custom_codecs;
    custom_codecs.sort_by(|left, right| {
        (&left.type_id, &left.version).cmp(&(&right.type_id, &right.version))
    });
    for pair in custom_codecs.windows(2) {
        if pair[0].type_id == pair[1].type_id && pair[0].version == pair[1].version {
            return Err(ArtifactCompileError::DuplicateCodec(format!(
                "{}@{}",
                pair[0].type_id, pair[0].version
            )));
        }
    }

    let unsigned = UnsignedCompiledWorkflow {
        schema: WORKFLOW_ARTIFACT_SCHEMA,
        entry: &request.entry,
        modules: &modules,
        plan: &plan,
        validation_bundles: &validation_bundles,
        guidance_schemas: &request.guidance_schemas,
        custom_codecs: &custom_codecs,
        compatibility: &request.compatibility,
        limits: &request.limits,
    };
    let canonical = serde_json_canonicalizer::to_vec(&unsigned)
        .map_err(|error| ArtifactCompileError::Canonical(error.to_string()))?;

    Ok(CompiledWorkflow {
        schema: WORKFLOW_ARTIFACT_SCHEMA.into(),
        entry: request.entry,
        modules,
        plan,
        validation_bundles,
        guidance_schemas: request.guidance_schemas,
        custom_codecs,
        compatibility: request.compatibility,
        limits: request.limits,
        artifact_sha256: sha256(&canonical),
    })
}

fn validate_compatibility(tuple: &CompatibilityTuple) -> Result<(), ArtifactCompileError> {
    for (name, value) in [
        ("productReleaseId", tuple.product_release_id.as_str()),
        ("agentsRevision", tuple.agents_revision.as_str()),
        ("zodRevision", tuple.zod_revision.as_str()),
        ("evaluatorRevision", tuple.evaluator_revision.as_str()),
        ("adapterAbi", tuple.adapter_abi.as_str()),
        ("canonicalizer", tuple.canonicalizer.as_str()),
        ("issueFormat", tuple.issue_format.as_str()),
        ("target", tuple.target.as_str()),
        ("sandboxIdentity", tuple.sandbox_identity.as_str()),
    ] {
        if value.trim().is_empty() {
            return Err(ArtifactCompileError::EmptyCompatibility(name));
        }
    }
    Ok(())
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compatibility() -> CompatibilityTuple {
        CompatibilityTuple {
            product_release_id: "product-1".into(),
            agents_revision: "agents-1".into(),
            zod_revision: "zod-1".into(),
            evaluator_revision: "evaluator-1".into(),
            adapter_abi: "adapter-1".into(),
            canonicalizer: "rfc8785-jcs-v1".into(),
            issue_format: "issues-v1".into(),
            target: "aarch64-apple-darwin".into(),
            sandbox_identity: "local-coding-harness".into(),
        }
    }

    fn source(name: &str) -> String {
        format!(
            "import {{ workflow }} from '@codex-orchestra/workflow'; export default workflow({{ name: '{name}', steps: [] }});"
        )
    }

    fn request() -> CompileRequest {
        CompileRequest {
            entry: "main.workflow.ts".into(),
            modules: BTreeMap::from([("main.workflow.ts".into(), source("compile"))]),
            validation_bundles: BTreeMap::from([(
                "output".into(),
                "({ output: z.string().trim() })".into(),
            )]),
            guidance_schemas: BTreeMap::from([(
                "output".into(),
                serde_json::json!({"type": "string"}),
            )]),
            custom_codecs: Vec::new(),
            compatibility: compatibility(),
            limits: CompileLimits::default(),
        }
    }

    #[test]
    fn identical_closed_graph_and_tuple_have_identical_identity() {
        let first = compile_artifact(request()).unwrap();
        let second = compile_artifact(request()).unwrap();
        assert_eq!(first, second);
        first.verify_identity().unwrap();
        assert_eq!(first.artifact_sha256.len(), 64);
        assert_eq!(first.validation_bundles["output"].sha256.len(), 64);
    }

    #[test]
    fn verification_rejects_modified_authoritative_source() {
        let mut artifact = compile_artifact(request()).unwrap();
        artifact.modules[0].source.push_str(" // changed");
        assert!(matches!(
            artifact.verify_identity(),
            Err(ArtifactCompileError::SourceIdentityMismatch(_))
        ));
    }

    #[test]
    fn source_or_tuple_change_changes_artifact_identity() {
        let first = compile_artifact(request()).unwrap();
        let mut changed_source = request();
        changed_source
            .modules
            .insert("main.workflow.ts".into(), source("changed"));
        let second = compile_artifact(changed_source).unwrap();
        let mut changed_tuple = request();
        changed_tuple.compatibility.evaluator_revision = "evaluator-2".into();
        let third = compile_artifact(changed_tuple).unwrap();
        assert_ne!(first.artifact_sha256, second.artifact_sha256);
        assert_ne!(first.artifact_sha256, third.artifact_sha256);
    }

    #[test]
    fn rejects_unclosed_and_oversized_graphs_before_lowering() {
        let mut missing = request();
        missing.entry = "missing.workflow.ts".into();
        assert!(matches!(
            compile_artifact(missing),
            Err(ArtifactCompileError::MissingEntry(_))
        ));

        let mut oversized = request();
        oversized.limits.module_source_bytes = 8;
        assert!(matches!(
            compile_artifact(oversized),
            Err(ArtifactCompileError::ModuleBytes { .. })
        ));
    }
}
