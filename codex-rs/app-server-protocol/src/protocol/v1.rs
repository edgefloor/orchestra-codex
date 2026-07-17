use std::collections::BTreeMap;
use std::collections::HashMap;
use std::path::PathBuf;

use codex_protocol::ThreadId;
use codex_protocol::config_types::ForcedLoginMethod;
use codex_protocol::config_types::ReasoningSummary;
use codex_protocol::config_types::SandboxMode;
use codex_protocol::config_types::Verbosity;
use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::parse_command::ParsedCommand;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::FileChange;
pub use codex_protocol::protocol::GitSha;
use codex_protocol::protocol::ReviewDecision;
use codex_protocol::protocol::SandboxPolicy;
use codex_protocol::protocol::SessionSource;
use codex_protocol::protocol::TurnAbortReason;
use codex_utils_absolute_path::AbsolutePathBuf;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

use crate::protocol::common::AuthMode;
use crate::protocol::v2::ForcedChatgptWorkspaceIds;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    pub client_info: ClientInfo,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<InitializeCapabilities>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ClientInfo {
    pub name: String,
    pub title: Option<String>,
    pub version: String,
}

/// Client-declared capabilities negotiated during initialize.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct InitializeCapabilities {
    /// Opt into receiving experimental API methods and fields.
    #[serde(default)]
    pub experimental_api: bool,
    /// Opt into `attestation/generate` requests for upstream `x-oai-attestation`.
    #[serde(default)]
    pub request_attestation: bool,
    /// Allow downstream MCP servers to request OpenAI extended form elicitations.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub mcp_server_openai_form_elicitation: bool,
    /// Exact notification method names that should be suppressed for this
    /// connection (for example `thread/started`).
    #[ts(optional = nullable)]
    pub opt_out_notification_methods: Option<Vec<String>>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResponse {
    pub user_agent: String,
    /// Absolute path to the server's $CODEX_HOME directory.
    pub codex_home: AbsolutePathBuf,
    /// Platform family for the running app-server target, for example
    /// `"unix"` or `"windows"`.
    pub platform_family: String,
    /// Operating system for the running app-server target, for example
    /// `"macos"`, `"linux"`, or `"windows"`.
    pub platform_os: String,
    /// Exact native Product tuple. Stock Codex builds omit this field; the
    /// Orchestra desktop requires it and fails closed when it is absent.
    #[serde(default)]
    pub orchestra_product: Option<OrchestraReleaseManifest>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct OrchestraReleaseManifest {
    pub schema_version: u32,
    pub product_version: String,
    pub minimum_macos: String,
    pub target: String,
    pub sources: BTreeMap<String, String>,
    pub schemas: BTreeMap<String, OrchestraSchemaIdentity>,
    pub evaluator: OrchestraEvaluatorIdentity,
    pub capabilities: Vec<String>,
    pub limits: BTreeMap<String, u64>,
    pub artifacts: BTreeMap<String, OrchestraArtifactIdentity>,
    pub manifest_sha256: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct OrchestraSchemaIdentity {
    pub identity: String,
    pub sha256: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct OrchestraEvaluatorIdentity {
    pub revision: String,
    pub adapter_abi: String,
    pub canonicalizer: String,
    pub issue_format: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct OrchestraArtifactIdentity {
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(untagged)]
pub enum GetConversationSummaryParams {
    RolloutPath {
        #[serde(rename = "rolloutPath")]
        rollout_path: PathBuf,
    },
    ThreadId {
        #[serde(rename = "conversationId")]
        conversation_id: ThreadId,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct GetConversationSummaryResponse {
    pub summary: ConversationSummary,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ConversationSummary {
    pub conversation_id: ThreadId,
    pub path: PathBuf,
    pub preview: String,
    pub timestamp: Option<String>,
    pub updated_at: Option<String>,
    pub model_provider: String,
    pub cwd: PathBuf,
    pub cli_version: String,
    pub source: SessionSource,
    pub git_info: Option<ConversationGitInfo>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub struct ConversationGitInfo {
    pub sha: Option<String>,
    pub branch: Option<String>,
    pub origin_url: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct LoginApiKeyParams {
    pub api_key: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct GitDiffToRemoteResponse {
    pub sha: GitSha,
    pub diff: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ApplyPatchApprovalParams {
    pub conversation_id: ThreadId,
    /// Use to correlate this with [codex_protocol::protocol::PatchApplyBeginEvent]
    /// and [codex_protocol::protocol::PatchApplyEndEvent].
    pub call_id: String,
    pub file_changes: HashMap<PathBuf, FileChange>,
    /// Optional explanatory reason (e.g. request for extra write access).
    pub reason: Option<String>,
    /// When set, the agent is asking the user to allow writes under this root
    /// for the remainder of the session (unclear if this is honored today).
    pub grant_root: Option<PathBuf>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ApplyPatchApprovalResponse {
    pub decision: ReviewDecision,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ExecCommandApprovalParams {
    pub conversation_id: ThreadId,
    /// Use to correlate this with [codex_protocol::protocol::ExecCommandBeginEvent]
    /// and [codex_protocol::protocol::ExecCommandEndEvent].
    pub call_id: String,
    /// Identifier for this specific approval callback.
    pub approval_id: Option<String>,
    pub command: Vec<String>,
    pub cwd: PathBuf,
    pub reason: Option<String>,
    pub parsed_cmd: Vec<ParsedCommand>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
pub struct ExecCommandApprovalResponse {
    pub decision: ReviewDecision,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct GitDiffToRemoteParams {
    pub cwd: PathBuf,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct GetAuthStatusParams {
    pub include_token: Option<bool>,
    pub refresh_token: Option<bool>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ExecOneOffCommandParams {
    pub command: Vec<String>,
    pub timeout_ms: Option<u64>,
    pub cwd: Option<PathBuf>,
    pub sandbox_policy: Option<SandboxPolicy>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct GetAuthStatusResponse {
    pub auth_method: Option<AuthMode>,
    pub auth_token: Option<String>,
    pub requires_openai_auth: Option<bool>,
}

#[derive(Deserialize, Debug, Clone, PartialEq, Serialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct UserSavedConfig {
    pub approval_policy: Option<AskForApproval>,
    pub sandbox_mode: Option<SandboxMode>,
    pub sandbox_settings: Option<SandboxSettings>,
    pub forced_chatgpt_workspace_id: Option<ForcedChatgptWorkspaceIds>,
    pub forced_login_method: Option<ForcedLoginMethod>,
    pub model: Option<String>,
    pub model_reasoning_effort: Option<ReasoningEffort>,
    pub model_reasoning_summary: Option<ReasoningSummary>,
    pub model_verbosity: Option<Verbosity>,
    pub tools: Option<Tools>,
}

#[derive(Deserialize, Debug, Clone, PartialEq, Serialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct Tools {
    pub web_search: Option<bool>,
}

#[derive(Deserialize, Debug, Clone, PartialEq, Serialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SandboxSettings {
    #[serde(default)]
    pub writable_roots: Vec<AbsolutePathBuf>,
    pub network_access: Option<bool>,
    pub exclude_tmpdir_env_var: Option<bool>,
    pub exclude_slash_tmp: Option<bool>,
}

#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct InterruptConversationResponse {
    pub abort_reason: TurnAbortReason,
}
