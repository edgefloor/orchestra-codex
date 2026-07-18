use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use ts_rs::TS;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AutomationValidateParams {
    pub thread_id: String,
    pub profile_path: String,
    pub fixture_issue: AutomationIssue,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub attempt: Option<u32>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AutomationRunFixtureParams {
    pub thread_id: String,
    pub profile_path: String,
    pub fixture_issue: AutomationIssue,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub attempt: Option<u32>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export_to = "v2/")]
pub struct AutomationStartParams {
    pub thread_id: String,
    pub profile_path: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export_to = "v2/")]
pub struct AutomationLinearReadParams {
    pub thread_id: String,
    pub profile_path: String,
    pub kind: AutomationLinearReadKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub after: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub first: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub issue_identifier: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export_to = "v2/")]
pub enum AutomationLinearReadKind {
    Candidates,
    Terminal,
    Refresh,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AutomationLinearReadResponse {
    pub status: AutomationLinearReadStatus,
    pub issues: Vec<AutomationIssue>,
    pub has_next_page: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub end_cursor: Option<String>,
    pub next_action: OrchestraBoundedText,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export_to = "v2/")]
pub enum AutomationLinearReadStatus {
    Ready,
    Skipped,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export_to = "v2/")]
pub struct AutomationRunParams {
    pub thread_id: String,
    pub run_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export_to = "v2/")]
pub struct AutomationStatusParams {
    pub thread_id: String,
    pub run_id: String,
    #[ts(optional = nullable)]
    pub focused_issue_id: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export_to = "v2/")]
pub struct AutomationCancelIssueParams {
    pub thread_id: String,
    pub run_id: String,
    pub claim_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export_to = "v2/")]
pub struct AutomationSteerIssueParams {
    pub thread_id: String,
    pub run_id: String,
    pub claim_id: String,
    pub input: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export_to = "v2/")]
pub struct AutomationReconcileParams {
    pub thread_id: String,
    pub run_id: String,
    pub profile_path: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AutomationIssue {
    pub id: String,
    pub identifier: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub priority: Option<i64>,
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub branch_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub url: Option<String>,
    pub labels: Vec<String>,
    pub blocked_by: Vec<AutomationIssueBlocker>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub created_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub updated_at: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AutomationIssueBlocker {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub identifier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub state: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AutomationValidateResponse {
    pub valid: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub profile: Option<AutomationProfile>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub profile_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub preview: Option<AutomationWorkflowPreview>,
    pub diagnostics: Vec<AutomationDiagnostic>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AutomationRunResponse {
    pub run: AutomationRunProjection,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AutomationSteerIssueResponse {
    pub run: AutomationRunProjection,
    pub receipt: AutomationSteeringReceipt,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AutomationSteeringReceipt {
    pub sequence: u32,
    pub submitted_at_ms: u64,
    pub initiator_thread_id: String,
    pub target_thread_id: String,
    pub authority: String,
    pub input_sha256: String,
    pub input_preview: String,
    pub status: AutomationSteeringStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub provider_receipt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub failure: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export_to = "v2/")]
pub enum AutomationSteeringStatus {
    Submitted,
    Delivered,
    Failed,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export_to = "v2/")]
pub struct AutomationQueueReadParams {
    pub thread_id: String,
    pub run_id: String,
    pub category: AutomationQueueCategory,
    #[ts(optional)]
    pub offset: Option<u32>,
    #[ts(optional)]
    pub limit: Option<u32>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AutomationQueueReadResponse {
    pub category: AutomationQueueCategory,
    pub total: u32,
    pub items: Vec<AutomationQueueItemProjection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub next_offset: Option<u32>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export_to = "v2/")]
pub enum AutomationQueueCategory {
    Queued,
    Running,
    Blocked,
    WaitingGate,
    Handoff,
    Terminal,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AutomationQueueCounts {
    pub queued: u32,
    pub running: u32,
    pub blocked: u32,
    pub waiting_gate: u32,
    pub handoff: u32,
    pub terminal: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AutomationQueueItemProjection {
    pub issue_id: String,
    pub issue_identifier: String,
    pub issue_title: OrchestraBoundedText,
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub priority: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub claim_id: Option<String>,
    pub category: AutomationQueueCategory,
    pub next_action: OrchestraBoundedText,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocked_by: Vec<AutomationQueueBlockerProjection>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AutomationQueueBlockerProjection {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub id: Option<OrchestraBoundedText>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub identifier: Option<OrchestraBoundedText>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub state: Option<OrchestraBoundedText>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AutomationCoordinationProjection {
    pub cycle: u64,
    pub scan_revision: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub input_cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub output_cursor: Option<String>,
    pub intake_status: AutomationCoordinationIntakeStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub page_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub started_at_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub completed_at_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub error: Option<OrchestraBoundedText>,
    pub next_action: OrchestraBoundedText,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub dispatch_intent: Option<AutomationDispatchIntentProjection>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AutomationDispatchIntentProjection {
    pub intent_id: String,
    pub claim_id: String,
    pub issue_id: String,
    pub kind: AutomationDispatchIntentKind,
    pub status: AutomationDispatchIntentStatus,
    pub attempt: u32,
    pub profile_digest: String,
    pub created_at_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub ready_at_ms: Option<u64>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export_to = "v2/")]
pub enum AutomationCoordinationIntakeStatus {
    NotStarted,
    Ready,
    Skipped,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export_to = "v2/")]
pub enum AutomationDispatchIntentKind {
    NewClaim,
    Retry,
    Continuation,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export_to = "v2/")]
pub enum AutomationDispatchIntentStatus {
    Pending,
    Started,
    Completed,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AutomationRunProjection {
    pub schema_version: u32,
    pub run_id: String,
    pub owner_thread_id: String,
    pub source_revision: String,
    pub profile_digest: String,
    pub profile_revision: u64,
    pub profile_revision_status: AutomationProfileRevisionStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub pending_profile_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub rejected_profile_digest: Option<String>,
    pub profile_diagnostics: Vec<OrchestraBoundedText>,
    pub tracker_project_slug: String,
    pub lease_epoch: u64,
    pub revision: u64,
    pub status: AutomationRootStatus,
    pub reconciliation: AutomationReconciliationStatus,
    pub coordination: AutomationCoordinationProjection,
    pub queue_counts: AutomationQueueCounts,
    pub claims_total: u32,
    pub claims: Vec<AutomationIssueClaimProjection>,
    pub queue_preview: Vec<AutomationQueueItemProjection>,
    pub queue_preview_truncated: bool,
    pub next_action: OrchestraBoundedText,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AutomationIssueClaimProjection {
    pub claim_id: String,
    pub issue_id: String,
    pub issue_identifier: String,
    pub issue_title: OrchestraBoundedText,
    pub issue_url: Option<String>,
    pub tracker_state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub priority: Option<i64>,
    pub attempt: u32,
    pub workflow_invocations: u32,
    pub turns_in_window: u32,
    pub continuation_count: u32,
    pub retry_attempt: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub scheduled_retry: Option<AutomationRetryScheduleProjection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub last_progress_at_ms: Option<u64>,
    pub profile_digest: String,
    pub profile_revision: u64,
    pub status: AutomationClaimStatus,
    pub worktree: String,
    pub source_revision: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub issue_task: Option<OrchestraAgentReference>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub workflow_run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub workflow_status: Option<OrchestraRunStatus>,
    pub effects: Vec<AutomationEffectReceiptProjection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub latest_steering_receipt: Option<AutomationSteeringReceipt>,
    pub hook_receipts: Vec<AutomationHookReceiptProjection>,
    pub cleanup: AutomationCleanupProjection,
    pub next_action: OrchestraBoundedText,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AutomationRetryScheduleProjection {
    pub kind: AutomationRetryKind,
    pub ready_at_ms: u64,
    pub reset_turn_window: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export_to = "v2/")]
pub enum AutomationRetryKind {
    Retry,
    Continuation,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AutomationHookReceiptProjection {
    pub kind: AutomationHookKind,
    pub invocation: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub command_sha256: Option<String>,
    pub status: AutomationHookStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub exit_code: Option<i32>,
    pub stdout_preview: OrchestraBoundedText,
    pub stderr_preview: OrchestraBoundedText,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub failure: Option<OrchestraBoundedText>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AutomationCleanupProjection {
    pub status: AutomationCleanupStatus,
    pub attempts: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub last_failure: Option<OrchestraBoundedText>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export_to = "v2/")]
pub enum AutomationHookKind {
    AfterCreate,
    BeforeRun,
    AfterRun,
    BeforeRemove,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export_to = "v2/")]
pub enum AutomationHookStatus {
    Succeeded,
    Failed,
    Skipped,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export_to = "v2/")]
pub enum AutomationCleanupStatus {
    Retained,
    Eligible,
    RetryPending,
    Removed,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AutomationEffectReceiptProjection {
    pub effect_id: String,
    pub idempotency_key: String,
    pub kind: AutomationEffect,
    pub status: AutomationEffectStatus,
    pub gate_policy: AutomationGatePolicy,
    pub request_sha256: String,
    pub body_preview: OrchestraBoundedText,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub provider_receipt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub failure: Option<OrchestraBoundedText>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export_to = "v2/")]
pub enum AutomationGatePolicy {
    AutoAccept,
    AutoReject,
    AskHuman,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export_to = "v2/")]
pub enum AutomationEffectStatus {
    WaitingGate,
    Rejected,
    Executing,
    Committed,
    Failed,
    Ambiguous,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export_to = "v2/")]
pub enum AutomationRootStatus {
    Running,
    Suspended,
    Cancelled,
    Failed,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export_to = "v2/")]
pub enum AutomationProfileRevisionStatus {
    Active,
    PendingValid,
    Rejected,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export_to = "v2/")]
pub enum AutomationReconciliationStatus {
    Complete,
    Required,
    InProgress,
    Blocked,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export_to = "v2/")]
pub enum AutomationClaimStatus {
    Claimed,
    Running,
    Completed,
    Suspended,
    Cancelled,
    Failed,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
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

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AutomationTrackerProfile {
    pub kind: String,
    pub endpoint: String,
    pub project_slug: String,
    pub required_labels: Vec<String>,
    pub active_states: Vec<String>,
    pub terminal_states: Vec<String>,
    pub credential: AutomationSecretReference,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AutomationPollingProfile {
    pub interval_ms: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AutomationWorkspaceProfile {
    pub root: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AutomationHooksProfile {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub after_create: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub before_run: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub after_run: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub before_remove: Option<String>,
    pub timeout_ms: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AutomationAgentProfile {
    pub max_concurrent_agents: u32,
    pub max_turns: u32,
    pub max_retry_backoff_ms: u64,
    pub max_concurrent_agents_by_state: std::collections::BTreeMap<String, u32>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AutomationCodexPolicy {
    pub approval_policy: Value,
    pub thread_sandbox: String,
    pub turn_sandbox_policy: Value,
    pub turn_timeout_ms: u64,
    pub read_timeout_ms: u64,
    pub stall_timeout_ms: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AutomationOrchestraProfile {
    pub workflow_path: String,
    pub workflow_sha256: String,
    pub workflow_name: String,
    pub effects: Vec<AutomationEffect>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[ts(export_to = "v2/")]
pub enum AutomationEffect {
    #[serde(rename = "tracker.comment")]
    #[ts(rename = "tracker.comment")]
    TrackerComment,
    #[serde(rename = "tracker.transition")]
    #[ts(rename = "tracker.transition")]
    TrackerTransition,
    #[serde(rename = "tracker.link_pull_request")]
    #[ts(rename = "tracker.link_pull_request")]
    TrackerLinkPullRequest,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export_to = "v2/")]
pub enum AutomationSecretKind {
    Environment,
    InlineDigest,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AutomationSecretReference {
    pub kind: AutomationSecretKind,
    pub reference: String,
    pub digest: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AutomationWorkflowPreview {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub rendered_prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub workflow: Option<String>,
    pub effects: Vec<AutomationEffect>,
    pub inputs: Vec<AutomationWorkflowInput>,
    pub secret_references: Vec<AutomationSecretReference>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AutomationWorkflowInput {
    pub name: String,
    pub kind: AutomationWorkflowInputKind,
    pub required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub default: Option<Value>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export_to = "v2/")]
pub enum AutomationWorkflowInputKind {
    String,
    Number,
    Boolean,
    Object,
    Array,
    Json,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export_to = "v2/")]
pub enum AutomationValidationSeverity {
    Error,
    Warning,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export_to = "v2/")]
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

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AutomationDiagnostic {
    pub severity: AutomationValidationSeverity,
    pub code: AutomationDiagnosticCode,
    pub path: String,
    pub message: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct OrchestraValidateParams {
    pub thread_id: String,
    pub workflow_path: String,
}

#[cfg(test)]
mod automation_protocol_tests {
    use super::*;

    fn fixture_issue() -> AutomationIssue {
        AutomationIssue {
            id: "issue-1".into(),
            identifier: "ORC-32".into(),
            title: "Validate Automation".into(),
            description: None,
            priority: Some(1),
            state: "Todo".into(),
            branch_name: None,
            url: None,
            labels: vec!["automation".into()],
            blocked_by: Vec::new(),
            created_at: None,
            updated_at: None,
        }
    }

    #[test]
    fn automation_validation_request_is_closed_and_task_scoped() {
        let value = serde_json::to_value(AutomationValidateParams {
            thread_id: "thread-1".into(),
            profile_path: "WORKFLOW.md".into(),
            fixture_issue: fixture_issue(),
            attempt: None,
        })
        .unwrap();
        assert_eq!(value["threadId"], "thread-1");
        assert_eq!(value["fixtureIssue"]["identifier"], "ORC-32");
        assert!(value.get("repositoryRoot").is_none());
        assert!(value.get("approvalPolicy").is_none());
    }

    #[test]
    fn automation_run_request_cannot_supply_repository_or_authority() {
        let value = serde_json::to_value(AutomationRunFixtureParams {
            thread_id: "thread-1".into(),
            profile_path: "WORKFLOW.md".into(),
            fixture_issue: fixture_issue(),
            attempt: Some(1),
        })
        .unwrap();
        assert_eq!(value["threadId"], "thread-1");
        assert_eq!(value["attempt"], 1);
        assert!(value.get("repositoryRoot").is_none());
        assert!(value.get("workspaceRoot").is_none());
        assert!(value.get("approvalPolicy").is_none());
    }

    #[test]
    fn automation_start_and_steering_are_closed_and_task_scoped() {
        let start = serde_json::to_value(AutomationStartParams {
            thread_id: "thread-1".into(),
            profile_path: "WORKFLOW.md".into(),
        })
        .unwrap();
        assert_eq!(start["threadId"], "thread-1");
        assert!(start.get("repositoryRoot").is_none());
        assert!(
            serde_json::from_value::<AutomationStartParams>(serde_json::json!({
                "threadId": "thread-1",
                "profilePath": "WORKFLOW.md",
                "repositoryRoot": "/tmp/foreign"
            }))
            .is_err()
        );

        let steer = serde_json::to_value(AutomationSteerIssueParams {
            thread_id: "thread-1".into(),
            run_id: "automation-1".into(),
            claim_id: "claim-1".into(),
            input: "Focus on recovery.".into(),
        })
        .unwrap();
        assert_eq!(steer["claimId"], "claim-1");
        assert!(steer.get("authority").is_none());
        assert!(
            serde_json::from_value::<AutomationSteerIssueParams>(serde_json::json!({
                "threadId": "thread-1",
                "runId": "automation-1",
                "claimId": "claim-1",
                "input": "Focus on recovery.",
                "authority": "foreign"
            }))
            .is_err()
        );
    }

    #[test]
    fn automation_status_focus_is_optional_while_shared_lifecycle_params_stay_strict() {
        let status = serde_json::to_value(AutomationStatusParams {
            thread_id: "thread-1".into(),
            run_id: "automation-1".into(),
            focused_issue_id: Some("issue-42".into()),
        })
        .unwrap();
        assert_eq!(status["focusedIssueId"], "issue-42");

        let legacy: AutomationStatusParams = serde_json::from_value(serde_json::json!({
            "threadId": "thread-1",
            "runId": "automation-1"
        }))
        .unwrap();
        assert_eq!(legacy.focused_issue_id, None);
        assert_eq!(
            serde_json::to_value(&legacy).unwrap()["focusedIssueId"],
            serde_json::Value::Null
        );
        assert!(serde_json::from_value::<AutomationRunParams>(status).is_err());
    }

    #[test]
    fn automation_optional_fields_are_omitted_instead_of_serialized_as_null() {
        let hooks = serde_json::to_value(AutomationHooksProfile {
            after_create: None,
            before_run: None,
            after_run: None,
            before_remove: None,
            timeout_ms: 30_000,
        })
        .unwrap();
        assert!(hooks.get("afterCreate").is_none());
        assert!(hooks.get("beforeRun").is_none());
        assert!(hooks.get("afterRun").is_none());
        assert!(hooks.get("beforeRemove").is_none());

        let input = serde_json::to_value(AutomationWorkflowInput {
            name: "issue".into(),
            kind: AutomationWorkflowInputKind::Object,
            required: true,
            default: None,
        })
        .unwrap();
        assert!(input.get("default").is_none());
    }

    #[test]
    fn automation_claim_exposes_only_the_latest_durable_steering_receipt() {
        let claim = |latest_steering_receipt, scheduled_retry| AutomationIssueClaimProjection {
            claim_id: "claim-1".into(),
            issue_id: "issue-1".into(),
            issue_identifier: "ORC-32".into(),
            issue_title: OrchestraBoundedText {
                text: "Validate Automation".into(),
                truncated: false,
            },
            issue_url: None,
            tracker_state: "Todo".into(),
            priority: None,
            attempt: 1,
            workflow_invocations: 2,
            turns_in_window: 1,
            continuation_count: 0,
            retry_attempt: 0,
            scheduled_retry,
            last_progress_at_ms: Some(42),
            profile_digest: "profile-sha".into(),
            profile_revision: 1,
            status: AutomationClaimStatus::Running,
            worktree: "/tmp/orc-32".into(),
            source_revision: "abc123".into(),
            issue_task: None,
            workflow_run_id: None,
            workflow_status: None,
            effects: Vec::new(),
            latest_steering_receipt,
            hook_receipts: Vec::new(),
            cleanup: AutomationCleanupProjection {
                status: AutomationCleanupStatus::Retained,
                attempts: 0,
                last_failure: None,
            },
            next_action: OrchestraBoundedText {
                text: "observe native Issue task".into(),
                truncated: false,
            },
        };

        let absent = serde_json::to_value(claim(None, None)).unwrap();
        assert_eq!(absent["issueUrl"], serde_json::Value::Null);
        assert!(absent.get("latestSteeringReceipt").is_none());
        assert!(absent.get("scheduledRetry").is_none());
        let legacy: AutomationIssueClaimProjection = serde_json::from_value(absent).unwrap();
        assert!(legacy.issue_url.is_none());
        assert!(legacy.scheduled_retry.is_none());

        let present = serde_json::to_value(claim(
            Some(AutomationSteeringReceipt {
                sequence: 2,
                submitted_at_ms: 42,
                initiator_thread_id: "task-1".into(),
                target_thread_id: "issue-task-1".into(),
                authority: "automation-claim-native-send-input-v1".into(),
                input_sha256: "input-sha".into(),
                input_preview: "Focus on recovery.".into(),
                status: AutomationSteeringStatus::Delivered,
                provider_receipt: Some("submission-2".into()),
                failure: None,
            }),
            Some(AutomationRetryScheduleProjection {
                kind: AutomationRetryKind::Continuation,
                ready_at_ms: 84,
                reset_turn_window: true,
            }),
        ))
        .unwrap();
        assert_eq!(present["latestSteeringReceipt"]["sequence"], 2);
        assert_eq!(present["latestSteeringReceipt"]["status"], "delivered");
        assert_eq!(
            present["latestSteeringReceipt"]["providerReceipt"],
            "submission-2"
        );
        assert_eq!(present["scheduledRetry"]["kind"], "continuation");
        assert_eq!(present["scheduledRetry"]["readyAtMs"], 84);
        assert_eq!(present["scheduledRetry"]["resetTurnWindow"], true);
    }

    #[test]
    fn automation_queue_blockers_default_for_legacy_payloads() {
        let legacy = serde_json::json!({
            "issueId": "issue-1",
            "issueIdentifier": "ORC-1",
            "issueTitle": { "text": "Blocked issue", "truncated": false },
            "state": "Todo",
            "category": "blocked",
            "nextAction": { "text": "inspect blockers", "truncated": false }
        });

        let item: AutomationQueueItemProjection = serde_json::from_value(legacy).unwrap();
        assert!(item.blocked_by.is_empty());

        let projected = AutomationQueueItemProjection {
            blocked_by: vec![
                AutomationQueueBlockerProjection {
                    id: Some(OrchestraBoundedText {
                        text: "blocker-1".into(),
                        truncated: false,
                    }),
                    identifier: Some(OrchestraBoundedText {
                        text: "ORC-2".into(),
                        truncated: false,
                    }),
                    state: Some(OrchestraBoundedText {
                        text: "In Progress".into(),
                        truncated: false,
                    }),
                },
                AutomationQueueBlockerProjection {
                    id: Some(OrchestraBoundedText {
                        text: "blocker-2".into(),
                        truncated: false,
                    }),
                    identifier: Some(OrchestraBoundedText {
                        text: "ORC-3".into(),
                        truncated: false,
                    }),
                    state: None,
                },
            ],
            ..item
        };
        let value = serde_json::to_value(projected).unwrap();
        assert_eq!(value["blockedBy"][0]["identifier"]["text"], "ORC-2");
        assert_eq!(value["blockedBy"][1]["identifier"]["text"], "ORC-3");
    }

    #[test]
    fn linear_read_request_exposes_only_typed_read_selection() {
        let value = serde_json::to_value(AutomationLinearReadParams {
            thread_id: "thread-1".into(),
            profile_path: "WORKFLOW.md".into(),
            kind: AutomationLinearReadKind::Candidates,
            after: Some("cursor-1".into()),
            first: Some(25),
            issue_identifier: None,
        })
        .unwrap();
        assert_eq!(value["kind"], "candidates");
        assert!(value.get("query").is_none());
        assert!(value.get("variables").is_none());
        assert!(value.get("credential").is_none());
        let mut forged = value;
        forged["query"] = Value::String("mutation { nope }".into());
        assert!(serde_json::from_value::<AutomationLinearReadParams>(forged).is_err());
    }

    #[test]
    fn queue_read_request_is_bounded_task_owned_selection() {
        let value = serde_json::to_value(AutomationQueueReadParams {
            thread_id: "thread-1".into(),
            run_id: "automation-1".into(),
            category: AutomationQueueCategory::Blocked,
            offset: Some(25),
            limit: Some(25),
        })
        .unwrap();
        assert_eq!(value["category"], "blocked");
        assert!(value.get("repositoryRoot").is_none());
        assert!(value.get("trackerQuery").is_none());
        let mut forged = value;
        forged["limit"] = Value::from(100_000);
        forged["repositoryRoot"] = Value::String("/tmp/foreign".into());
        assert!(serde_json::from_value::<AutomationQueueReadParams>(forged).is_err());
    }

    #[test]
    fn lifecycle_requests_cannot_supply_lease_or_reconciliation_results() {
        let value = serde_json::to_value(AutomationReconcileParams {
            thread_id: "thread-1".into(),
            run_id: "automation-1".into(),
            profile_path: "WORKFLOW.md".into(),
        })
        .unwrap();
        assert!(value.get("leaseEpoch").is_none());
        assert!(value.get("claims").is_none());
        let mut forged = value;
        forged["leaseEpoch"] = Value::from(99);
        assert!(serde_json::from_value::<AutomationReconcileParams>(forged).is_err());
    }

    #[test]
    fn cancel_issue_request_selects_only_one_task_owned_claim() {
        let value = serde_json::to_value(AutomationCancelIssueParams {
            thread_id: "task-42".into(),
            run_id: "automation-42".into(),
            claim_id: "claim-42".into(),
        })
        .unwrap();
        assert_eq!(value["claimId"], "claim-42");
        let mut forged = value;
        forged["issueId"] = serde_json::json!("another-issue");
        assert!(serde_json::from_value::<AutomationCancelIssueParams>(forged).is_err());
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct OrchestraInvokeParams {
    pub thread_id: String,
    pub workflow_path: String,
    #[ts(optional)]
    pub inputs: Option<Value>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct OrchestraRunParams {
    pub thread_id: String,
    pub run_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct OrchestraResumeParams {
    pub thread_id: String,
    pub run_id: String,
    #[ts(optional)]
    pub approval_decision: Option<String>,
    #[ts(optional)]
    pub inputs: Option<Value>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct OrchestraQueryParams {
    pub thread_id: String,
    pub run_id: String,
    pub selector: OrchestraQueryKind,
    #[ts(optional)]
    pub step_id: Option<String>,
    #[ts(optional)]
    pub evidence_id: Option<String>,
    #[ts(optional)]
    pub after: Option<String>,
    #[ts(optional)]
    pub history_after: Option<OrchestraHistoryCursor>,
    #[ts(optional)]
    pub max_items: Option<u32>,
    #[ts(optional)]
    pub max_bytes: Option<u32>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export_to = "v2/")]
pub enum OrchestraQueryKind {
    Run,
    Steps,
    Outputs,
    Evidence,
    EvidenceContent,
    History,
    Digest,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct OrchestraHistoryCursor {
    pub sequence: u64,
    pub item_id: String,
    pub revision: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct OrchestraValidateResponse {
    pub valid: bool,
    pub plan: OrchestraWorkflowPlan,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct OrchestraRunResponse {
    pub run: OrchestraRunProjection,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct OrchestraQueryResponse {
    pub result: OrchestraQueryResult,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(tag = "selector", content = "result", rename_all = "snake_case")]
#[ts(export_to = "v2/")]
pub enum OrchestraQueryResult {
    Run(OrchestraExecutionRunProjection),
    Steps(OrchestraStepsPage),
    Outputs(OrchestraOutputsPage),
    Evidence(OrchestraEvidencePage),
    EvidenceContent(OrchestraEvidenceContentProjection),
    History(OrchestraHistoryPage),
    Digest(OrchestraRunDigest),
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct OrchestraBoundedText {
    pub text: String,
    pub truncated: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct OrchestraStepCounts {
    pub pending: usize,
    pub running: usize,
    pub retrying: usize,
    pub waiting_approval: usize,
    pub completed: usize,
    pub failed: usize,
    pub cancelled: usize,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct OrchestraExecutionRunProjection {
    pub schema_version: u32,
    pub run_id: String,
    pub workflow_sha256: String,
    pub source_revision: String,
    pub status: OrchestraRunStatus,
    pub promotion: OrchestraPromotionStatus,
    pub step_counts: OrchestraStepCounts,
    pub next_action: OrchestraBoundedText,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct OrchestraAgentReference {
    pub thread_id: String,
    pub task_path: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct OrchestraExecutionStepProjection {
    pub id: String,
    pub status: OrchestraStepStatus,
    pub attempts: u32,
    pub rounds: u32,
    #[ts(optional)]
    pub agent: Option<OrchestraAgentReference>,
    #[ts(optional)]
    pub context_sha256: Option<String>,
    #[ts(optional)]
    pub approval_decision: Option<OrchestraBoundedText>,
    #[ts(optional)]
    pub error: Option<OrchestraBoundedText>,
    pub output_count: usize,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct OrchestraStepsPage {
    pub items: Vec<OrchestraExecutionStepProjection>,
    #[ts(optional)]
    pub next: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct OrchestraOutputProjection {
    pub step_id: String,
    pub name: String,
    pub sha256: String,
    pub canonical_bytes: usize,
    #[ts(optional)]
    pub value: Option<Value>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct OrchestraOutputsPage {
    pub items: Vec<OrchestraOutputProjection>,
    #[ts(optional)]
    pub next: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export_to = "v2/")]
pub enum OrchestraEvidenceKind {
    Check,
    Change,
    Skill,
    Other,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct OrchestraEvidenceReference {
    pub evidence_id: String,
    pub name: String,
    pub kind: OrchestraEvidenceKind,
    pub provenance: OrchestraEvidenceProvenance,
    #[ts(optional)]
    pub step_id: Option<String>,
    pub bytes: u64,
    #[ts(optional)]
    pub sha256: Option<String>,
    pub availability: OrchestraEvidenceAvailability,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export_to = "v2/")]
pub enum OrchestraEvidenceProvenance {
    RuntimeCheck,
    RuntimeChange,
    SkillSnapshot,
    RuntimeOther,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export_to = "v2/")]
pub enum OrchestraEvidenceAvailability {
    Available,
    ContentTooLarge,
    Malformed,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct OrchestraEvidenceContentProjection {
    pub evidence_id: String,
    pub name: String,
    pub kind: OrchestraEvidenceKind,
    pub provenance: OrchestraEvidenceProvenance,
    pub availability: OrchestraEvidenceAvailability,
    pub bytes: u64,
    #[ts(optional)]
    pub sha256: Option<String>,
    pub media_type: String,
    #[ts(optional)]
    pub content: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct OrchestraEvidencePage {
    pub items: Vec<OrchestraEvidenceReference>,
    #[ts(optional)]
    pub next: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct OrchestraHistoryRecord {
    pub sequence: u64,
    pub item_id: String,
    pub revision: u64,
    pub kind: String,
    #[ts(optional)]
    pub step_id: Option<String>,
    pub summary: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct OrchestraHistoryPage {
    pub items: Vec<OrchestraHistoryRecord>,
    #[ts(optional)]
    pub next: Option<OrchestraHistoryCursor>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct OrchestraRunDigest {
    pub run_id: String,
    pub state_sha256: String,
    pub text: String,
    pub omitted_steps: usize,
}

/// Bounded task-local lifecycle replay returned by `thread/read`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct OrchestraTaskReplay {
    pub latest: OrchestraReplayEvent,
    pub events: Vec<OrchestraReplayEvent>,
    pub replay_truncated: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct OrchestraReplayEvent {
    pub schema_version: u32,
    pub event_id: String,
    pub run_id: String,
    pub sequence: u64,
    pub revision: u64,
    pub kind: OrchestraLifecycleKind,
    pub projection: OrchestraRunProjection,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub enum OrchestraLifecycleKind {
    Invoked,
    Resumed,
    Cancelled,
    Recovered,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct OrchestraWorkflowPlan {
    pub name: String,
    pub description: String,
    pub max_parallel: u32,
    pub steps: Vec<OrchestraWorkflowStep>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct OrchestraWorkflowStep {
    pub id: String,
    pub kind: OrchestraStepKind,
    pub needs: Vec<String>,
    pub max_attempts: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub enum OrchestraStepKind {
    Agent,
    Check,
    Approval,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct OrchestraRunProjection {
    pub schema_version: u32,
    pub run_id: String,
    pub workflow_sha256: String,
    pub parent_thread_id: String,
    pub source_revision: String,
    pub status: OrchestraRunStatus,
    pub promotion: OrchestraPromotionStatus,
    pub steps: Vec<OrchestraStepProjection>,
    pub next_action: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub enum OrchestraRunStatus {
    Pending,
    Running,
    WaitingApproval,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub enum OrchestraPromotionStatus {
    Pending,
    Applied,
    NotRequired,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct OrchestraStepProjection {
    pub id: String,
    pub status: OrchestraStepStatus,
    pub attempts: u32,
    pub rounds: u32,
    pub output_keys: Vec<String>,
    #[ts(optional)]
    pub final_response: Option<String>,
    #[ts(optional)]
    pub error: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub enum OrchestraStepStatus {
    Pending,
    Running,
    Retrying,
    WaitingApproval,
    Completed,
    Failed,
    Cancelled,
}
