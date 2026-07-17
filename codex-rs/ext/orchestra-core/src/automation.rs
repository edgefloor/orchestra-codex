use crate::InputDefault;
use crate::InputKind;
use crate::canonical_sha256;
use crate::compile_workflow;
use minijinja::Environment;
use minijinja::UndefinedBehavior;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use sha2::Digest;
use sha2::Sha256;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fs;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

const DEFAULT_LINEAR_ENDPOINT: &str = "https://api.linear.app/graphql";
const DEFAULT_PROMPT: &str = "You are working on an issue from Linear.";

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationProfile {
    pub tracker: AutomationTrackerProfile,
    pub polling: AutomationPollingProfile,
    pub workspace: AutomationWorkspaceProfile,
    pub hooks: AutomationHooksProfile,
    pub agent: AutomationAgentProfile,
    pub codex: AutomationCodexPolicy,
    pub orchestra: AutomationOrchestraProfile,
    pub prompt_template: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationTrackerProfile {
    pub kind: String,
    pub endpoint: String,
    pub project_slug: String,
    pub required_labels: Vec<String>,
    pub active_states: Vec<String>,
    pub terminal_states: Vec<String>,
    pub credential: AutomationSecretReference,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationPollingProfile {
    pub interval_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationWorkspaceProfile {
    pub root: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationHooksProfile {
    pub after_create: Option<String>,
    pub before_run: Option<String>,
    pub after_run: Option<String>,
    pub before_remove: Option<String>,
    pub timeout_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationAgentProfile {
    pub max_concurrent_agents: u32,
    pub max_turns: u32,
    pub max_retry_backoff_ms: u64,
    pub max_concurrent_agents_by_state: BTreeMap<String, u32>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationCodexPolicy {
    pub approval_policy: Value,
    pub thread_sandbox: String,
    pub turn_sandbox_policy: Value,
    pub turn_timeout_ms: u64,
    pub read_timeout_ms: u64,
    pub stall_timeout_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationOrchestraProfile {
    pub workflow_path: String,
    pub workflow_sha256: String,
    pub workflow_name: String,
    pub effects: Vec<AutomationEffect>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub enum AutomationEffect {
    #[serde(rename = "tracker.comment")]
    TrackerComment,
    #[serde(rename = "tracker.transition")]
    TrackerTransition,
    #[serde(rename = "tracker.link_pull_request")]
    TrackerLinkPullRequest,
}

impl AutomationEffect {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "tracker.comment" => Some(Self::TrackerComment),
            "tracker.transition" => Some(Self::TrackerTransition),
            "tracker.link_pull_request" => Some(Self::TrackerLinkPullRequest),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationSecretKind {
    Environment,
    InlineDigest,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationSecretReference {
    pub kind: AutomationSecretKind,
    pub reference: String,
    pub digest: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationValidationResult {
    pub valid: bool,
    pub profile: Option<AutomationProfile>,
    pub profile_digest: Option<String>,
    pub preview: Option<AutomationWorkflowPreview>,
    pub diagnostics: Vec<AutomationDiagnostic>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationWorkflowPreview {
    pub rendered_prompt: Option<String>,
    pub workflow: Option<String>,
    pub effects: Vec<AutomationEffect>,
    pub inputs: Vec<AutomationWorkflowInput>,
    pub secret_references: Vec<AutomationSecretReference>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationWorkflowInput {
    pub name: String,
    pub kind: InputKind,
    pub required: bool,
    pub default: Option<Value>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationValidationSeverity {
    Error,
    Warning,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationDiagnosticCode {
    MissingWorkflowFile,
    WorkflowParseError,
    WorkflowFrontMatterNotAMap,
    MissingField,
    InvalidValue,
    UnknownField,
    UnsupportedTracker,
    MissingSecret,
    ProhibitedCodexCommand,
    PolicyBroadening,
    UnsafeWorkspaceRoot,
    MissingOrchestraExtension,
    UnsupportedEffect,
    WorkflowCompileError,
    WorkflowInputMissing,
    WorkflowInputIncompatible,
    WorkflowInputNeedsDefault,
    TemplateParseError,
    TemplateRenderError,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationDiagnostic {
    pub severity: AutomationValidationSeverity,
    pub code: AutomationDiagnosticCode,
    pub path: String,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AutomationValidationRequest {
    pub workflow_md_path: PathBuf,
    pub repository_root: PathBuf,
    pub fixture_issue: AutomationIssue,
    pub attempt: Option<u32>,
    pub environment: BTreeMap<String, String>,
    pub home_dir: Option<PathBuf>,
    pub inherited_policy: InheritedCodexPolicy,
}

#[derive(Clone, Debug, PartialEq)]
pub struct InheritedCodexPolicy {
    pub approval_policy: Value,
    pub thread_sandbox: String,
    pub turn_sandbox_policy: Value,
}

impl Default for InheritedCodexPolicy {
    fn default() -> Self {
        Self {
            approval_policy: Value::String("on-request".into()),
            thread_sandbox: "workspace-write".into(),
            turn_sandbox_policy: Value::Null,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationIssue {
    pub id: String,
    pub identifier: String,
    pub title: String,
    pub description: Option<String>,
    pub priority: Option<i64>,
    pub state: String,
    pub branch_name: Option<String>,
    pub url: Option<String>,
    pub labels: Vec<String>,
    pub blocked_by: Vec<AutomationIssueBlocker>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationIssueBlocker {
    pub id: Option<String>,
    pub identifier: Option<String>,
    pub state: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct RawProfile {
    tracker: Option<RawTracker>,
    polling: Option<RawPolling>,
    workspace: Option<RawWorkspace>,
    hooks: Option<RawHooks>,
    agent: Option<RawAgent>,
    codex: Option<RawCodex>,
    orchestra: Option<RawOrchestra>,
    #[serde(flatten)]
    extensions: BTreeMap<String, serde_yaml_ng::Value>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTracker {
    kind: Option<String>,
    endpoint: Option<String>,
    #[serde(alias = "credential")]
    api_key: Option<String>,
    project_slug: Option<String>,
    #[serde(default)]
    required_labels: Vec<String>,
    active_states: Option<Vec<String>>,
    terminal_states: Option<Vec<String>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPolling {
    interval_ms: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawWorkspace {
    root: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawHooks {
    after_create: Option<String>,
    before_run: Option<String>,
    after_run: Option<String>,
    before_remove: Option<String>,
    timeout_ms: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAgent {
    max_concurrent_agents: Option<u32>,
    max_turns: Option<u32>,
    max_retry_backoff_ms: Option<u64>,
    #[serde(default)]
    max_concurrent_agents_by_state: BTreeMap<String, i64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCodex {
    command: Option<String>,
    approval_policy: Option<Value>,
    thread_sandbox: Option<String>,
    turn_sandbox_policy: Option<Value>,
    turn_timeout_ms: Option<u64>,
    read_timeout_ms: Option<u64>,
    stall_timeout_ms: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawOrchestra {
    workflow: Option<String>,
    #[serde(default)]
    effects: Vec<String>,
}

pub fn validate_automation_profile(
    request: AutomationValidationRequest,
) -> AutomationValidationResult {
    let mut diagnostics = Vec::new();
    let source = match fs::read_to_string(&request.workflow_md_path) {
        Ok(source) => source,
        Err(error) => {
            diagnostics.push(error_diagnostic(
                AutomationDiagnosticCode::MissingWorkflowFile,
                "WORKFLOW.md",
                format!(
                    "failed to read {}: {error}",
                    request.workflow_md_path.display()
                ),
            ));
            return invalid_result(diagnostics);
        }
    };
    let (front_matter, prompt_template) = match split_workflow_document(&source) {
        Ok(parts) => parts,
        Err(diagnostic) => return invalid_result(vec![diagnostic]),
    };
    let raw = match parse_profile(front_matter) {
        Ok(raw) => raw,
        Err(diagnostic) => return invalid_result(vec![diagnostic]),
    };
    for extension in raw.extensions.keys() {
        diagnostics.push(AutomationDiagnostic {
            severity: AutomationValidationSeverity::Warning,
            code: AutomationDiagnosticCode::UnknownField,
            path: extension.clone(),
            message: "unrecognized top-level Symphony extension is ignored by this Product tuple"
                .into(),
        });
    }

    let tracker = raw.tracker.unwrap_or_default();
    let tracker_kind = required_string(tracker.kind, "tracker.kind", &mut diagnostics);
    if !tracker_kind.is_empty() && tracker_kind != "linear" {
        diagnostics.push(error_diagnostic(
            AutomationDiagnosticCode::UnsupportedTracker,
            "tracker.kind",
            format!("tracker `{tracker_kind}` is not supported; the MVP supports `linear`"),
        ));
    }
    let project_slug = required_string(
        tracker.project_slug,
        "tracker.project_slug",
        &mut diagnostics,
    );
    let (credential, credential_available) = secret_reference(
        tracker.api_key,
        &request.environment,
        "tracker.api_key",
        &mut diagnostics,
    );
    if !credential_available {
        diagnostics.push(AutomationDiagnostic {
            severity: AutomationValidationSeverity::Warning,
            code: AutomationDiagnosticCode::MissingSecret,
            path: "tracker.api_key".into(),
            message: "tracker credential is unavailable; fixture validation remains available but live Linear reads will be skipped"
                .into(),
        });
    }
    let required_labels = normalize_labels(tracker.required_labels, &mut diagnostics);
    let active_states = normalized_nonempty_list(
        tracker
            .active_states
            .unwrap_or_else(|| vec!["Todo".into(), "In Progress".into()]),
        "tracker.active_states",
        &mut diagnostics,
    );
    let terminal_states = normalized_nonempty_list(
        tracker.terminal_states.unwrap_or_else(|| {
            vec![
                "Closed".into(),
                "Cancelled".into(),
                "Canceled".into(),
                "Duplicate".into(),
                "Done".into(),
            ]
        }),
        "tracker.terminal_states",
        &mut diagnostics,
    );

    let polling = raw.polling.unwrap_or_default();
    let interval_ms = positive_u64(
        polling.interval_ms.unwrap_or(30_000),
        "polling.interval_ms",
        &mut diagnostics,
    );

    let workflow_dir = request
        .workflow_md_path
        .parent()
        .unwrap_or_else(|| Path::new("."));
    let workspace = raw.workspace.unwrap_or_default();
    let workspace_raw = workspace
        .root
        .unwrap_or_else(|| ".codex/orchestra/worktrees".into());
    let workspace_root = resolve_path_value(
        &workspace_raw,
        workflow_dir,
        request.home_dir.as_deref(),
        &request.environment,
        "workspace.root",
        &mut diagnostics,
    );
    validate_workspace_root(&workspace_root, &request.repository_root, &mut diagnostics);

    let hooks = raw.hooks.unwrap_or_default();
    let hooks_timeout_ms = positive_u64(
        hooks.timeout_ms.unwrap_or(60_000),
        "hooks.timeout_ms",
        &mut diagnostics,
    );

    let agent = raw.agent.unwrap_or_default();
    let max_concurrent_agents = positive_u32(
        agent.max_concurrent_agents.unwrap_or(10),
        "agent.max_concurrent_agents",
        &mut diagnostics,
    );
    let max_turns = positive_u32(
        agent.max_turns.unwrap_or(20),
        "agent.max_turns",
        &mut diagnostics,
    );
    let max_retry_backoff_ms = positive_u64(
        agent.max_retry_backoff_ms.unwrap_or(300_000),
        "agent.max_retry_backoff_ms",
        &mut diagnostics,
    );
    let mut by_state = BTreeMap::new();
    for (state, value) in agent.max_concurrent_agents_by_state {
        let state = state.trim().to_lowercase();
        if state.is_empty() || value <= 0 || value > u32::MAX as i64 {
            diagnostics.push(AutomationDiagnostic {
                severity: AutomationValidationSeverity::Warning,
                code: AutomationDiagnosticCode::InvalidValue,
                path: "agent.max_concurrent_agents_by_state".into(),
                message: "ignored a blank state or non-positive concurrency override".into(),
            });
            continue;
        }
        by_state.insert(state, value as u32);
    }

    let codex = raw.codex.unwrap_or_default();
    if codex.command.is_some() {
        diagnostics.push(error_diagnostic(
            AutomationDiagnosticCode::ProhibitedCodexCommand,
            "codex.command",
            "Orchestra uses the resident native Codex runtime; `codex.command` is prohibited",
        ));
    }
    let effective_approval = validate_approval_policy(
        codex.approval_policy,
        &request.inherited_policy.approval_policy,
        &mut diagnostics,
    );
    let effective_thread_sandbox = validate_thread_sandbox(
        codex.thread_sandbox,
        &request.inherited_policy.thread_sandbox,
        &mut diagnostics,
    );
    let effective_turn_sandbox = validate_turn_sandbox(
        codex.turn_sandbox_policy,
        &request.inherited_policy.turn_sandbox_policy,
        &mut diagnostics,
    );

    let orchestra = raw.orchestra.unwrap_or_default();
    let workflow_raw = match orchestra.workflow {
        Some(value) if !value.trim().is_empty() => value,
        _ => {
            diagnostics.push(error_diagnostic(
                AutomationDiagnosticCode::MissingOrchestraExtension,
                "orchestra.workflow",
                "the Orchestra extension must select an issue `.workflow.ts`",
            ));
            String::new()
        }
    };
    let mut effects = BTreeSet::new();
    for value in orchestra.effects {
        match AutomationEffect::parse(value.trim()) {
            Some(effect) => {
                effects.insert(effect);
            }
            None => diagnostics.push(error_diagnostic(
                AutomationDiagnosticCode::UnsupportedEffect,
                "orchestra.effects",
                format!("`{value}` is not an allowlisted tracker effect"),
            )),
        }
    }
    let effects = effects.into_iter().collect::<Vec<_>>();

    let workflow_path = resolve_workflow_path(
        &workflow_raw,
        workflow_dir,
        &request.repository_root,
        &mut diagnostics,
    );
    let mut workflow_name = String::new();
    let mut workflow_sha256 = String::new();
    let mut workflow_inputs = Vec::new();
    if let Some(path) = &workflow_path {
        match fs::read_to_string(path) {
            Ok(source) => {
                workflow_sha256 = automation_source_sha256(&source);
                match compile_workflow(&source) {
                    Ok(plan) => {
                        workflow_name = plan.name.clone();
                        workflow_inputs = plan
                            .inputs
                            .iter()
                            .map(|(name, input)| AutomationWorkflowInput {
                                name: name.clone(),
                                kind: input.kind.clone(),
                                required: input.required,
                                default: input.default.value().cloned(),
                            })
                            .collect();
                        validate_workflow_inputs(&plan.inputs, &mut diagnostics);
                    }
                    Err(error) => diagnostics.push(error_diagnostic(
                        AutomationDiagnosticCode::WorkflowCompileError,
                        "orchestra.workflow",
                        error.to_string(),
                    )),
                }
            }
            Err(error) => diagnostics.push(error_diagnostic(
                AutomationDiagnosticCode::WorkflowCompileError,
                "orchestra.workflow",
                format!(
                    "failed to read selected workflow {}: {error}",
                    path.display()
                ),
            )),
        }
    }

    let prompt_template = if prompt_template.is_empty() {
        DEFAULT_PROMPT.to_owned()
    } else {
        prompt_template.to_owned()
    };
    let rendered_prompt = render_prompt(
        &prompt_template,
        &request.fixture_issue,
        request.attempt,
        &mut diagnostics,
    );

    let profile = AutomationProfile {
        tracker: AutomationTrackerProfile {
            kind: tracker_kind,
            endpoint: tracker
                .endpoint
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_LINEAR_ENDPOINT.into()),
            project_slug,
            required_labels,
            active_states,
            terminal_states,
            credential: credential.clone(),
        },
        polling: AutomationPollingProfile { interval_ms },
        workspace: AutomationWorkspaceProfile {
            root: workspace_root.to_string_lossy().into_owned(),
        },
        hooks: AutomationHooksProfile {
            after_create: hooks.after_create,
            before_run: hooks.before_run,
            after_run: hooks.after_run,
            before_remove: hooks.before_remove,
            timeout_ms: hooks_timeout_ms,
        },
        agent: AutomationAgentProfile {
            max_concurrent_agents,
            max_turns,
            max_retry_backoff_ms,
            max_concurrent_agents_by_state: by_state,
        },
        codex: AutomationCodexPolicy {
            approval_policy: effective_approval,
            thread_sandbox: effective_thread_sandbox,
            turn_sandbox_policy: effective_turn_sandbox,
            turn_timeout_ms: codex.turn_timeout_ms.unwrap_or(3_600_000),
            read_timeout_ms: codex.read_timeout_ms.unwrap_or(5_000),
            stall_timeout_ms: codex.stall_timeout_ms.unwrap_or(300_000),
        },
        orchestra: AutomationOrchestraProfile {
            workflow_path: workflow_path
                .as_deref()
                .unwrap_or_else(|| Path::new(&workflow_raw))
                .to_string_lossy()
                .into_owned(),
            workflow_sha256,
            workflow_name: workflow_name.clone(),
            effects: effects.clone(),
        },
        prompt_template,
    };
    let profile_digest = serde_json::to_value(&profile)
        .ok()
        .and_then(|value| canonical_sha256(&value).ok());
    let valid = diagnostics
        .iter()
        .all(|diagnostic| diagnostic.severity != AutomationValidationSeverity::Error);
    AutomationValidationResult {
        valid,
        profile: Some(profile),
        profile_digest,
        preview: Some(AutomationWorkflowPreview {
            rendered_prompt,
            workflow: (!workflow_name.is_empty()).then_some(workflow_name),
            effects,
            inputs: workflow_inputs,
            secret_references: vec![credential],
        }),
        diagnostics,
    }
}

fn split_workflow_document(source: &str) -> Result<(&str, &str), AutomationDiagnostic> {
    let source = source.strip_prefix('\u{feff}').unwrap_or(source);
    let mut lines = source.split_inclusive('\n');
    let Some(first) = lines.next() else {
        return Ok(("", ""));
    };
    if first.trim_end_matches(['\r', '\n']) != "---" {
        return Ok(("", source.trim()));
    }
    let front_start = first.len();
    let mut offset = front_start;
    for line in lines {
        if line.trim_end_matches(['\r', '\n']) == "---" {
            let front = &source[front_start..offset];
            let body = &source[offset + line.len()..];
            return Ok((front, body.trim()));
        }
        offset += line.len();
    }
    Err(error_diagnostic(
        AutomationDiagnosticCode::WorkflowParseError,
        "WORKFLOW.md",
        "YAML front matter is missing its closing `---` delimiter",
    ))
}

fn parse_profile(front_matter: &str) -> Result<RawProfile, AutomationDiagnostic> {
    if front_matter.trim().is_empty() {
        return Ok(RawProfile::default());
    }
    let value: serde_yaml_ng::Value = serde_yaml_ng::from_str(front_matter).map_err(|error| {
        error_diagnostic(
            AutomationDiagnosticCode::WorkflowParseError,
            "WORKFLOW.md.front_matter",
            error.to_string(),
        )
    })?;
    if !value.is_mapping() {
        return Err(error_diagnostic(
            AutomationDiagnosticCode::WorkflowFrontMatterNotAMap,
            "WORKFLOW.md.front_matter",
            "YAML front matter must decode to a map/object",
        ));
    }
    serde_yaml_ng::from_value(value).map_err(|error| {
        error_diagnostic(
            AutomationDiagnosticCode::WorkflowParseError,
            "WORKFLOW.md.front_matter",
            error.to_string(),
        )
    })
}

fn required_string(
    value: Option<String>,
    path: &str,
    diagnostics: &mut Vec<AutomationDiagnostic>,
) -> String {
    match value.map(|value| value.trim().to_owned()) {
        Some(value) if !value.is_empty() => value,
        _ => {
            diagnostics.push(error_diagnostic(
                AutomationDiagnosticCode::MissingField,
                path,
                "required value is missing or empty",
            ));
            String::new()
        }
    }
}

fn secret_reference(
    raw: Option<String>,
    environment: &BTreeMap<String, String>,
    path: &str,
    diagnostics: &mut Vec<AutomationDiagnostic>,
) -> (AutomationSecretReference, bool) {
    let raw = required_string(raw, path, diagnostics);
    if let Some(variable) = raw.strip_prefix('$') {
        let valid = !variable.is_empty()
            && variable
                .chars()
                .all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
            && variable
                .chars()
                .next()
                .is_some_and(|ch| ch == '_' || ch.is_ascii_alphabetic());
        if !valid {
            diagnostics.push(error_diagnostic(
                AutomationDiagnosticCode::InvalidValue,
                path,
                "secret references must use the exact `$VAR_NAME` form",
            ));
        }
        return (
            AutomationSecretReference {
                kind: AutomationSecretKind::Environment,
                reference: variable.to_owned(),
                digest: sha256(raw.as_bytes()),
            },
            valid
                && environment
                    .get(variable)
                    .is_some_and(|value| !value.trim().is_empty()),
        );
    }
    let available = !raw.is_empty();
    (
        AutomationSecretReference {
            kind: AutomationSecretKind::InlineDigest,
            reference: "inline".into(),
            digest: sha256(raw.as_bytes()),
        },
        available,
    )
}

fn normalize_labels(
    labels: Vec<String>,
    diagnostics: &mut Vec<AutomationDiagnostic>,
) -> Vec<String> {
    let mut normalized = BTreeSet::new();
    for label in labels {
        let label = label.trim().to_lowercase();
        if label.is_empty() {
            diagnostics.push(error_diagnostic(
                AutomationDiagnosticCode::InvalidValue,
                "tracker.required_labels",
                "required labels must not be blank",
            ));
        } else {
            normalized.insert(label);
        }
    }
    normalized.into_iter().collect()
}

fn normalized_nonempty_list(
    values: Vec<String>,
    path: &str,
    diagnostics: &mut Vec<AutomationDiagnostic>,
) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for value in values {
        let value = value.trim().to_owned();
        if value.is_empty() {
            diagnostics.push(error_diagnostic(
                AutomationDiagnosticCode::InvalidValue,
                path,
                "values must not be blank",
            ));
        } else if seen.insert(value.to_lowercase()) {
            out.push(value);
        }
    }
    if out.is_empty() {
        diagnostics.push(error_diagnostic(
            AutomationDiagnosticCode::InvalidValue,
            path,
            "at least one value is required",
        ));
    }
    out
}

fn positive_u64(value: u64, path: &str, diagnostics: &mut Vec<AutomationDiagnostic>) -> u64 {
    if value == 0 {
        diagnostics.push(error_diagnostic(
            AutomationDiagnosticCode::InvalidValue,
            path,
            "must be greater than zero",
        ));
    }
    value
}

fn positive_u32(value: u32, path: &str, diagnostics: &mut Vec<AutomationDiagnostic>) -> u32 {
    if value == 0 {
        diagnostics.push(error_diagnostic(
            AutomationDiagnosticCode::InvalidValue,
            path,
            "must be greater than zero",
        ));
    }
    value
}

fn resolve_path_value(
    raw: &str,
    base: &Path,
    home: Option<&Path>,
    environment: &BTreeMap<String, String>,
    path: &str,
    diagnostics: &mut Vec<AutomationDiagnostic>,
) -> PathBuf {
    let expanded = if let Some(variable) = raw.strip_prefix('$') {
        match environment
            .get(variable)
            .filter(|value| !value.trim().is_empty())
        {
            Some(value) => value.clone(),
            None => {
                diagnostics.push(error_diagnostic(
                    AutomationDiagnosticCode::MissingSecret,
                    path,
                    format!("environment reference `${variable}` is unavailable"),
                ));
                raw.to_owned()
            }
        }
    } else if raw == "~" || raw.starts_with("~/") {
        match home {
            Some(home) => home
                .join(raw.trim_start_matches("~/"))
                .to_string_lossy()
                .into_owned(),
            None => {
                diagnostics.push(error_diagnostic(
                    AutomationDiagnosticCode::InvalidValue,
                    path,
                    "cannot expand `~` because no home directory was supplied",
                ));
                raw.to_owned()
            }
        }
    } else {
        raw.to_owned()
    };
    lexical_absolute(Path::new(&expanded), base)
}

fn lexical_absolute(path: &Path, base: &Path) -> PathBuf {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    };
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => out.push(prefix.as_os_str()),
            Component::RootDir => out.push(Path::new("/")),
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(value) => out.push(value),
        }
    }
    out
}

fn validate_workspace_root(
    workspace_root: &Path,
    repository_root: &Path,
    diagnostics: &mut Vec<AutomationDiagnostic>,
) {
    let repository_root = lexical_absolute(repository_root, Path::new("/"));
    let unsafe_root = workspace_root.parent().is_none()
        || workspace_root == repository_root
        || workspace_root.starts_with(repository_root.join(".git"))
        || fs::symlink_metadata(workspace_root)
            .is_ok_and(|metadata| metadata.file_type().is_symlink());
    if unsafe_root {
        diagnostics.push(error_diagnostic(
            AutomationDiagnosticCode::UnsafeWorkspaceRoot,
            "workspace.root",
            "workspace root must be a dedicated non-symlink directory and cannot be `/`, the repository root, or `.git`",
        ));
    }
}

fn resolve_workflow_path(
    raw: &str,
    base: &Path,
    repository_root: &Path,
    diagnostics: &mut Vec<AutomationDiagnostic>,
) -> Option<PathBuf> {
    if raw.is_empty() {
        return None;
    }
    let path = lexical_absolute(Path::new(raw), base);
    let repository_root = fs::canonicalize(repository_root)
        .unwrap_or_else(|_| lexical_absolute(repository_root, Path::new("/")));
    let checked = fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
    if !checked.starts_with(&repository_root)
        || checked.extension().and_then(|value| value.to_str()) != Some("ts")
    {
        diagnostics.push(error_diagnostic(
            AutomationDiagnosticCode::WorkflowCompileError,
            "orchestra.workflow",
            "selected workflow must be a `.workflow.ts` file contained by the target repository",
        ));
        return None;
    }
    if !checked
        .file_name()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.ends_with(".workflow.ts"))
    {
        diagnostics.push(error_diagnostic(
            AutomationDiagnosticCode::WorkflowCompileError,
            "orchestra.workflow",
            "selected workflow filename must end in `.workflow.ts`",
        ));
        return None;
    }
    Some(checked)
}

fn validate_approval_policy(
    requested: Option<Value>,
    inherited: &Value,
    diagnostics: &mut Vec<AutomationDiagnostic>,
) -> Value {
    let Some(requested) = requested else {
        return inherited.clone();
    };
    let requested_rank = requested.as_str().and_then(approval_rank);
    let inherited_rank = inherited.as_str().and_then(approval_rank);
    let allowed = requested == *inherited
        || matches!((requested_rank, inherited_rank), (Some(requested), Some(inherited)) if requested <= inherited);
    if !allowed {
        diagnostics.push(error_diagnostic(
            AutomationDiagnosticCode::PolicyBroadening,
            "codex.approval_policy",
            "Automation policy may only narrow the active task approval policy",
        ));
        inherited.clone()
    } else {
        requested
    }
}

fn approval_rank(value: &str) -> Option<u8> {
    match value {
        "untrusted" => Some(0),
        "on-request" => Some(1),
        "never" => Some(2),
        _ => None,
    }
}

fn validate_thread_sandbox(
    requested: Option<String>,
    inherited: &str,
    diagnostics: &mut Vec<AutomationDiagnostic>,
) -> String {
    let Some(requested) = requested else {
        return inherited.to_owned();
    };
    let allowed = sandbox_rank(&requested)
        .zip(sandbox_rank(inherited))
        .is_some_and(|(requested, inherited)| requested <= inherited);
    if !allowed {
        diagnostics.push(error_diagnostic(
            AutomationDiagnosticCode::PolicyBroadening,
            "codex.thread_sandbox",
            "Automation sandbox may only narrow the active task sandbox",
        ));
        inherited.to_owned()
    } else {
        requested
    }
}

fn sandbox_rank(value: &str) -> Option<u8> {
    match value {
        "read-only" => Some(0),
        "workspace-write" => Some(1),
        "danger-full-access" => Some(2),
        _ => None,
    }
}

fn validate_turn_sandbox(
    requested: Option<Value>,
    inherited: &Value,
    diagnostics: &mut Vec<AutomationDiagnostic>,
) -> Value {
    let Some(requested) = requested else {
        return inherited.clone();
    };
    if requested != *inherited {
        diagnostics.push(error_diagnostic(
            AutomationDiagnosticCode::PolicyBroadening,
            "codex.turn_sandbox_policy",
            "the MVP accepts the inherited turn sandbox policy exactly; native policy comparison must prove any future narrowing",
        ));
        inherited.clone()
    } else {
        requested
    }
}

fn validate_workflow_inputs(
    inputs: &BTreeMap<String, crate::InputDefinition>,
    diagnostics: &mut Vec<AutomationDiagnostic>,
) {
    for (name, expected) in [
        ("issue", &[InputKind::Object, InputKind::Json][..]),
        ("task_prompt", &[InputKind::String][..]),
        ("automation", &[InputKind::Object, InputKind::Json][..]),
    ] {
        match inputs.get(name) {
            None => diagnostics.push(error_diagnostic(
                AutomationDiagnosticCode::WorkflowInputMissing,
                format!("orchestra.workflow.inputs.{name}"),
                format!("selected workflow must declare required `{name}` input"),
            )),
            Some(input) if !input.required || !expected.contains(&input.kind) => {
                diagnostics.push(error_diagnostic(
                    AutomationDiagnosticCode::WorkflowInputIncompatible,
                    format!("orchestra.workflow.inputs.{name}"),
                    format!("`{name}` must be required and use an Automation-compatible type"),
                ));
            }
            Some(_) => {}
        }
    }
    for (name, input) in inputs {
        if !matches!(name.as_str(), "issue" | "task_prompt" | "automation")
            && matches!(input.default, InputDefault::Missing)
        {
            diagnostics.push(error_diagnostic(
                AutomationDiagnosticCode::WorkflowInputNeedsDefault,
                format!("orchestra.workflow.inputs.{name}"),
                "additional Automation workflow inputs must declare a default",
            ));
        }
    }
}

fn render_prompt(
    prompt: &str,
    issue: &AutomationIssue,
    attempt: Option<u32>,
    diagnostics: &mut Vec<AutomationDiagnostic>,
) -> Option<String> {
    let mut environment = Environment::new();
    environment.set_undefined_behavior(UndefinedBehavior::Strict);
    let template = match environment.template_from_str(prompt) {
        Ok(template) => template,
        Err(error) => {
            diagnostics.push(error_diagnostic(
                AutomationDiagnosticCode::TemplateParseError,
                "prompt_template",
                error.to_string(),
            ));
            return None;
        }
    };
    match template.render(minijinja::context! { issue => issue, attempt => attempt }) {
        Ok(rendered) => Some(rendered),
        Err(error) => {
            diagnostics.push(error_diagnostic(
                AutomationDiagnosticCode::TemplateRenderError,
                "prompt_template",
                error.to_string(),
            ));
            None
        }
    }
}

fn error_diagnostic(
    code: AutomationDiagnosticCode,
    path: impl Into<String>,
    message: impl Into<String>,
) -> AutomationDiagnostic {
    AutomationDiagnostic {
        severity: AutomationValidationSeverity::Error,
        code,
        path: path.into(),
        message: message.into(),
    }
}

fn invalid_result(diagnostics: Vec<AutomationDiagnostic>) -> AutomationValidationResult {
    AutomationValidationResult {
        valid: false,
        profile: None,
        profile_digest: None,
        preview: None,
        diagnostics,
    }
}

pub fn automation_source_sha256(source: &str) -> String {
    sha256(source.as_bytes())
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn issue() -> AutomationIssue {
        AutomationIssue {
            id: "issue-1".into(),
            identifier: "ORC-32".into(),
            title: "Validate Automation".into(),
            description: Some("Build the validation slice".into()),
            priority: Some(1),
            state: "Todo".into(),
            branch_name: None,
            url: Some("https://linear.example/ORC-32".into()),
            labels: vec!["Automation".into()],
            blocked_by: Vec::new(),
            created_at: None,
            updated_at: None,
        }
    }

    fn workflow(extra_inputs: &str) -> String {
        format!(
            r#"import {{ workflow, agent }} from "@codex-orchestra/workflow";
export default workflow({{
  name: "automation-issue",
  inputs: {{
    issue: {{ type: "object" }},
    task_prompt: {{ type: "string" }},
    automation: {{ type: "object" }}{extra_inputs}
  }},
  steps: [agent({{ id: "implement", prompt: "Implement", model: "gpt-5.4" }})]
}});"#
        )
    }

    fn valid_document() -> String {
        r#"---
tracker:
  kind: linear
  api_key: $LINEAR_API_KEY
  project_slug: orchestra
  required_labels: [Automation, ready]
polling:
  interval_ms: 15000
workspace:
  root: .codex/orchestra/worktrees
hooks:
  before_run: git status --short
agent:
  max_concurrent_agents: 3
  max_turns: 7
  max_concurrent_agents_by_state:
    Todo: 2
codex:
  approval_policy: untrusted
  thread_sandbox: read-only
orchestra:
  workflow: issue.workflow.ts
  effects:
    - tracker.comment
    - tracker.transition
---
Work on {{ issue.identifier }}: {{ issue.title }} (attempt={{ attempt }}).
{% for label in issue.labels %}[{{ label | lower }}]{% endfor %}
"#
        .into()
    }

    fn request(document: &str, workflow: &str) -> (tempfile::TempDir, AutomationValidationRequest) {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join("WORKFLOW.md"), document).unwrap();
        fs::write(temp.path().join("issue.workflow.ts"), workflow).unwrap();
        let request = AutomationValidationRequest {
            workflow_md_path: temp.path().join("WORKFLOW.md"),
            repository_root: temp.path().to_path_buf(),
            fixture_issue: issue(),
            attempt: Some(2),
            environment: BTreeMap::from([("LINEAR_API_KEY".into(), "secret-value".into())]),
            home_dir: Some(temp.path().to_path_buf()),
            inherited_policy: InheritedCodexPolicy {
                approval_policy: Value::String("on-request".into()),
                thread_sandbox: "workspace-write".into(),
                turn_sandbox_policy: Value::Null,
            },
        };
        (temp, request)
    }

    #[test]
    fn validates_and_previews_a_symphony_compatible_profile() {
        let (_temp, request) = request(&valid_document(), &workflow(""));
        let first = validate_automation_profile(request.clone());
        let second = validate_automation_profile(request);
        assert!(first.valid, "{:#?}", first.diagnostics);
        assert_eq!(first.profile_digest, second.profile_digest);
        let profile = first.profile.unwrap();
        assert_eq!(profile.tracker.required_labels, ["automation", "ready"]);
        assert_eq!(profile.agent.max_concurrent_agents_by_state["todo"], 2);
        assert_eq!(profile.codex.approval_policy, "untrusted");
        assert_eq!(profile.codex.thread_sandbox, "read-only");
        let preview = first.preview.unwrap();
        assert!(preview.rendered_prompt.as_ref().unwrap().contains("ORC-32"));
        assert_eq!(preview.inputs.len(), 3);
        assert_eq!(preview.secret_references[0].reference, "LINEAR_API_KEY");
        let serialized = serde_json::to_string(&(profile, preview)).unwrap();
        assert!(!serialized.contains("secret-value"));
    }

    #[test]
    fn missing_live_credential_warns_without_invalidating_fixture_validation() {
        let (_temp, mut request) = request(&valid_document(), &workflow(""));
        request.environment.clear();
        let result = validate_automation_profile(request);
        assert!(result.valid, "{:#?}", result.diagnostics);
        let diagnostic = result
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == AutomationDiagnosticCode::MissingSecret)
            .unwrap();
        assert_eq!(diagnostic.severity, AutomationValidationSeverity::Warning);
        let serialized = serde_json::to_string(&result).unwrap();
        assert!(!serialized.contains("secret-value"));
    }

    #[test]
    fn rejects_codex_command_and_policy_broadening() {
        let document = valid_document()
            .replace("codex:\n", "codex:\n  command: codex app-server\n")
            .replace("approval_policy: untrusted", "approval_policy: never")
            .replace(
                "thread_sandbox: read-only",
                "thread_sandbox: danger-full-access",
            );
        let (_temp, request) = request(&document, &workflow(""));
        let result = validate_automation_profile(request);
        assert!(!result.valid);
        assert!(has_code(
            &result,
            AutomationDiagnosticCode::ProhibitedCodexCommand
        ));
        assert_eq!(
            result
                .diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.code == AutomationDiagnosticCode::PolicyBroadening)
                .count(),
            2
        );
    }

    #[test]
    fn rejects_workflow_input_mismatches_and_missing_defaults() {
        let incompatible = workflow(",\n    extra: { type: \"string\", required: false }")
            .replace("issue: { type: \"object\" }", "issue: { type: \"string\" }");
        let (_temp, request) = request(&valid_document(), &incompatible);
        let result = validate_automation_profile(request);
        assert!(!result.valid);
        assert!(has_code(
            &result,
            AutomationDiagnosticCode::WorkflowInputIncompatible
        ));
        assert!(has_code(
            &result,
            AutomationDiagnosticCode::WorkflowInputNeedsDefault
        ));
    }

    #[test]
    fn rejects_unknown_prompt_values_and_unsafe_workspace_roots() {
        let document = valid_document()
            .replace(".codex/orchestra/worktrees", "/")
            .replace("issue.title", "issue.missing");
        let (_temp, request) = request(&document, &workflow(""));
        let result = validate_automation_profile(request);
        assert!(!result.valid);
        assert!(has_code(
            &result,
            AutomationDiagnosticCode::UnsafeWorkspaceRoot
        ));
        assert!(has_code(
            &result,
            AutomationDiagnosticCode::TemplateRenderError
        ));
    }

    #[test]
    fn reports_non_map_and_unknown_nested_front_matter() {
        let (_temp, validation_request) = request("---\n- nope\n---\nprompt", &workflow(""));
        let result = validate_automation_profile(validation_request);
        assert!(has_code(
            &result,
            AutomationDiagnosticCode::WorkflowFrontMatterNotAMap
        ));

        let document = valid_document().replace(
            "interval_ms: 15000",
            "interval_ms: 15000\n  required_future_field: true",
        );
        let (_temp, request) = request(&document, &workflow(""));
        let result = validate_automation_profile(request);
        assert!(has_code(
            &result,
            AutomationDiagnosticCode::WorkflowParseError
        ));
    }

    #[test]
    fn additional_workflow_inputs_with_defaults_are_observable() {
        let workflow =
            workflow(",\n    model: { type: \"string\", required: false, default: \"gpt-5.4\" }");
        let (_temp, request) = request(&valid_document(), &workflow);
        let result = validate_automation_profile(request);
        assert!(result.valid, "{:#?}", result.diagnostics);
        let model = result
            .preview
            .unwrap()
            .inputs
            .into_iter()
            .find(|input| input.name == "model")
            .unwrap();
        assert_eq!(model.default, Some(Value::String("gpt-5.4".into())));
    }

    fn has_code(result: &AutomationValidationResult, code: AutomationDiagnosticCode) -> bool {
        result
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == code)
    }
}
