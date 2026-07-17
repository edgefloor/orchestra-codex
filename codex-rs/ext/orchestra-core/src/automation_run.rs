use crate::AgentHandle;
use crate::AutomationEffect;
use crate::AutomationIssue;
use crate::AutomationProfile;
use crate::RunStatus;
use serde::Deserialize;
use serde::Serialize;
use sha2::Digest;
use sha2::Sha256;
use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::fs::{self};
use std::io::Write;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;
use thiserror::Error;

static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationRootStatus {
    Running,
    Suspended,
    Cancelled,
    Failed,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationReconciliationStatus {
    #[default]
    Complete,
    Required,
    InProgress,
    Blocked,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationClaimStatus {
    Claimed,
    Running,
    Completed,
    Suspended,
    Cancelled,
    Failed,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationRetryKind {
    Retry,
    Continuation,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationRetrySchedule {
    pub kind: AutomationRetryKind,
    pub attempt: u32,
    pub delay_ms: u64,
    pub ready_at_ms: u64,
    pub reset_turn_window: bool,
    pub reason: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AutomationClaimLiveness {
    Active,
    WaitingGate,
    WaitingRetry,
    Stalled,
    Handoff,
    Terminal,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationProfileRevisionStatus {
    #[default]
    Active,
    PendingValid,
    Rejected,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationProfileRevision {
    pub revision: u64,
    pub status: AutomationProfileRevisionStatus,
    pub pending_digest: Option<String>,
    pub rejected_digest: Option<String>,
    pub diagnostics: Vec<String>,
}

impl Default for AutomationProfileRevision {
    fn default() -> Self {
        Self {
            revision: 1,
            status: AutomationProfileRevisionStatus::Active,
            pending_digest: None,
            rejected_digest: None,
            diagnostics: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationQueueStatus {
    Queued,
    Blocked,
    Terminal,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationQueueCategory {
    Queued,
    Running,
    Blocked,
    WaitingGate,
    Handoff,
    Terminal,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationQueueItem {
    pub issue_id: String,
    pub issue_identifier: String,
    pub issue_title: String,
    pub state: String,
    pub priority: Option<i64>,
    pub status: AutomationQueueStatus,
    pub reason: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationQueueProjectionItem {
    pub issue_id: String,
    pub issue_identifier: String,
    pub issue_title: String,
    pub state: String,
    pub priority: Option<i64>,
    pub claim_id: Option<String>,
    pub category: AutomationQueueCategory,
    pub next_action: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationQueuePage {
    pub category: AutomationQueueCategory,
    pub total: u32,
    pub items: Vec<AutomationQueueProjectionItem>,
    pub next_offset: Option<u32>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationQueueCounts {
    pub queued: u32,
    pub running: u32,
    pub blocked: u32,
    pub waiting_gate: u32,
    pub handoff: u32,
    pub terminal: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationCoordinationResult {
    pub dispatched_claim_ids: Vec<String>,
    pub counts: AutomationQueueCounts,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationIssueClaim {
    pub claim_id: String,
    pub issue_id: String,
    pub issue_identifier: String,
    pub issue_title: String,
    #[serde(default)]
    pub tracker_state: String,
    #[serde(default)]
    pub priority: Option<i64>,
    pub attempt: u32,
    #[serde(default)]
    pub profile_digest: String,
    #[serde(default)]
    pub profile_revision: u64,
    #[serde(default)]
    pub task_prompt: String,
    #[serde(default)]
    pub workflow_invocations: u32,
    #[serde(default)]
    pub turns_in_window: u32,
    #[serde(default)]
    pub continuation_count: u32,
    #[serde(default)]
    pub retry_attempt: u32,
    #[serde(default)]
    pub last_progress_at_ms: Option<u64>,
    #[serde(default)]
    pub retry: Option<AutomationRetrySchedule>,
    pub status: AutomationClaimStatus,
    pub worktree: PathBuf,
    pub source_revision: String,
    pub issue_task: Option<AgentHandle>,
    pub workflow_run_id: Option<String>,
    pub workflow_status: Option<RunStatus>,
    #[serde(default)]
    pub effects: Vec<AutomationEffectReceipt>,
    #[serde(default)]
    pub hook_receipts: Vec<AutomationHookReceipt>,
    #[serde(default)]
    pub cleanup: AutomationCleanupState,
    pub next_action: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationHookKind {
    AfterCreate,
    BeforeRun,
    AfterRun,
    BeforeRemove,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationHookStatus {
    Succeeded,
    Failed,
    Skipped,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationHookReceipt {
    pub kind: AutomationHookKind,
    pub invocation: u32,
    pub command_sha256: Option<String>,
    pub status: AutomationHookStatus,
    pub exit_code: Option<i32>,
    pub stdout_preview: String,
    pub stderr_preview: String,
    pub failure: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationCleanupStatus {
    #[default]
    Retained,
    Eligible,
    RetryPending,
    Removed,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationCleanupState {
    pub status: AutomationCleanupStatus,
    pub attempts: u32,
    pub last_failure: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationGatePolicy {
    AutoAccept,
    AutoReject,
    AskHuman,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationEffectStatus {
    WaitingGate,
    Rejected,
    Executing,
    Committed,
    Failed,
    Ambiguous,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationEffectReceipt {
    pub effect_id: String,
    pub idempotency_key: String,
    pub kind: AutomationEffect,
    pub claim_id: String,
    pub tracker_project_slug: String,
    pub issue_id: String,
    pub request_sha256: String,
    pub body_preview: String,
    pub gate_policy: AutomationGatePolicy,
    pub status: AutomationEffectStatus,
    pub provider_receipt: Option<String>,
    pub failure: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AutomationTrackerCommentRequest {
    pub effect_id: String,
    pub idempotency_key: String,
    pub claim_id: String,
    pub tracker_project_slug: String,
    pub issue_id: String,
    pub body: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AutomationTrackerTransitionRequest {
    pub effect_id: String,
    pub idempotency_key: String,
    pub claim_id: String,
    pub tracker_project_slug: String,
    pub issue_id: String,
    pub expected_state: String,
    pub target_state: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AutomationTrackerPullRequestLinkRequest {
    pub effect_id: String,
    pub idempotency_key: String,
    pub claim_id: String,
    pub tracker_project_slug: String,
    pub issue_id: String,
    pub pull_request_url: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AutomationEffectExecution {
    Committed { provider_receipt: String },
    Failed { message: String },
    Ambiguous { message: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PreparedAutomationEffect {
    receipt: AutomationEffectReceipt,
    execute: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationRootCheckpoint {
    pub schema_version: u32,
    pub run_id: String,
    pub owner_thread_id: String,
    pub repository: PathBuf,
    pub source_revision: String,
    pub profile_digest: String,
    #[serde(default)]
    pub profile_revision: AutomationProfileRevision,
    pub tracker_project_slug: String,
    pub workspace_root: PathBuf,
    pub lease_key: String,
    #[serde(default)]
    pub lease_epoch: u64,
    #[serde(default)]
    pub revision: u64,
    pub status: AutomationRootStatus,
    #[serde(default)]
    pub reconciliation: AutomationReconciliationStatus,
    pub claims: BTreeMap<String, AutomationIssueClaim>,
    #[serde(default)]
    pub queue: BTreeMap<String, AutomationQueueItem>,
    pub next_action: String,
}

pub struct AutomationRunStart<'a> {
    pub repository: &'a Path,
    pub owner_thread_id: &'a str,
    pub source_revision: &'a str,
    pub profile: &'a AutomationProfile,
    pub profile_digest: &'a str,
}

#[derive(Debug, Error)]
pub enum AutomationRunError {
    #[error("Automation state storage failed: {0}")]
    Storage(#[from] std::io::Error),
    #[error("Automation profile digest does not match its canonical snapshot")]
    ProfileDigestMismatch,
    #[error("Automation profile reload cannot change the tracker project")]
    ProfileProjectMismatch,
    #[error("Automation lease `{lease_key}` is already owned by task `{owner_thread_id}`")]
    LeaseConflict {
        lease_key: String,
        owner_thread_id: String,
    },
    #[error("issue `{0}` already has a claim in this Automation Root Run")]
    DuplicateIssue(String),
    #[error("Automation workspace path is outside its configured root")]
    UnsafeWorkspace,
    #[error("Automation claim `{0}` was not found")]
    MissingClaim(String),
    #[error("Automation claim `{0}` is not active")]
    InactiveClaim(String),
    #[error("Automation claim `{0}` has no retry ready for dispatch")]
    RetryNotReady(String),
    #[error(
        "Automation lease is stale (expected epoch {expected_epoch} revision {expected_revision}, found epoch {actual_epoch} revision {actual_revision})"
    )]
    StaleLease {
        expected_epoch: u64,
        expected_revision: u64,
        actual_epoch: u64,
        actual_revision: u64,
    },
    #[error("Automation Root Run must be suspended before reconciliation")]
    NotSuspended,
    #[error("Automation reconciliation has not started")]
    ReconciliationNotStarted,
    #[error("Automation reconciliation is incomplete: {0}")]
    ReconciliationBlocked(String),
    #[error("Tracker effect is not authorized by the effective Automation profile")]
    MissingEffectAuthority,
    #[error("tracker.comment body must contain 1..=4096 bytes")]
    InvalidComment,
    #[error("tracker.transition target must be a configured tracker state")]
    InvalidTransition,
    #[error("tracker.link_pull_request requires a canonical HTTPS GitHub pull-request URL")]
    InvalidPullRequestLink,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct AutomationLease {
    lease_key: String,
    run_id: String,
    owner_thread_id: String,
    repository: PathBuf,
    tracker_project_slug: String,
    #[serde(default)]
    lease_epoch: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AutomationClaimReconciliation {
    pub claim_id: String,
    pub issue_task_active: bool,
    pub descendants_cancelled: bool,
    pub tracker_terminal: bool,
    pub workflow_status: Option<RunStatus>,
}

pub struct AutomationRunStore {
    repository: PathBuf,
    root: PathBuf,
    lease_path: PathBuf,
}

impl AutomationRunStore {
    /// Start a resident Automation Root Run, or reopen the one already owned by
    /// the same task. The lease is repository/project scoped; a changed profile
    /// is staged explicitly after the resident Run has been reopened.
    pub fn start(
        request: AutomationRunStart<'_>,
    ) -> Result<(Self, AutomationRootCheckpoint), AutomationRunError> {
        let canonical_profile = serde_json::to_value(request.profile)
            .map_err(std::io::Error::other)
            .and_then(|value| crate::canonical_sha256(&value).map_err(std::io::Error::other))?;
        if canonical_profile != request.profile_digest {
            return Err(AutomationRunError::ProfileDigestMismatch);
        }
        let repository = canonical_or_lexical(request.repository)?;
        let lease_key = sha256(
            format!(
                "{}\0{}",
                repository.display(),
                request.profile.tracker.project_slug
            )
            .as_bytes(),
        );
        let leases = repository.join(".codex/orchestra/leases");
        fs::create_dir_all(&leases)?;
        let lease_path = leases.join(format!("automation-{lease_key}.json"));

        if lease_path.exists() {
            let lease: AutomationLease = read_json(&lease_path)?;
            let store = Self::open(&repository, &lease.run_id)?;
            let checkpoint = store.load()?;
            if lease.owner_thread_id == request.owner_thread_id
                && checkpoint.status == AutomationRootStatus::Running
            {
                return Ok((store, checkpoint));
            }
            return Err(AutomationRunError::LeaseConflict {
                lease_key,
                owner_thread_id: lease.owner_thread_id,
            });
        }

        let run_id = automation_run_id(request.profile_digest);
        let root = repository.join(".codex/orchestra/runs").join(&run_id);
        fs::create_dir_all(&root)?;
        let lease = AutomationLease {
            lease_key: lease_key.clone(),
            run_id: run_id.clone(),
            owner_thread_id: request.owner_thread_id.into(),
            repository: repository.clone(),
            tracker_project_slug: request.profile.tracker.project_slug.clone(),
            lease_epoch: 0,
        };
        create_json(&lease_path, &lease)?;
        let store = Self {
            repository: repository.clone(),
            root,
            lease_path,
        };
        if let Err(error) =
            store.write_active_profile_snapshot(request.profile_digest, request.profile)
        {
            let _ = fs::remove_file(&store.lease_path);
            return Err(error.into());
        }
        let mut checkpoint = AutomationRootCheckpoint {
            schema_version: 1,
            run_id,
            owner_thread_id: request.owner_thread_id.into(),
            repository,
            source_revision: request.source_revision.into(),
            profile_digest: request.profile_digest.into(),
            profile_revision: AutomationProfileRevision::default(),
            tracker_project_slug: request.profile.tracker.project_slug.clone(),
            workspace_root: PathBuf::from(&request.profile.workspace.root),
            lease_key,
            lease_epoch: 0,
            revision: 0,
            status: AutomationRootStatus::Running,
            reconciliation: AutomationReconciliationStatus::Complete,
            claims: BTreeMap::new(),
            queue: BTreeMap::new(),
            next_action: "dispatch one eligible issue".into(),
        };
        if let Err(error) = store.save(&mut checkpoint) {
            let _ = fs::remove_file(&store.lease_path);
            return Err(error);
        }
        Ok((store, checkpoint))
    }

    pub fn open(repository: &Path, run_id: &str) -> Result<Self, AutomationRunError> {
        let repository = canonical_or_lexical(repository)?;
        let root = repository.join(".codex/orchestra/runs").join(run_id);
        let checkpoint: AutomationRootCheckpoint = read_json(&root.join("automation-state.json"))?;
        let lease_path = repository
            .join(".codex/orchestra/leases")
            .join(format!("automation-{}.json", checkpoint.lease_key));
        Ok(Self {
            repository,
            root,
            lease_path,
        })
    }

    pub fn load(&self) -> Result<AutomationRootCheckpoint, AutomationRunError> {
        let mut checkpoint: AutomationRootCheckpoint =
            read_json(&self.root.join("automation-state.json"))?;
        if checkpoint.profile_revision.revision == 0 {
            checkpoint.profile_revision.revision = 1;
        }
        for claim in checkpoint.claims.values_mut() {
            if claim.profile_digest.is_empty() {
                claim.profile_digest.clone_from(&checkpoint.profile_digest);
            }
            if claim.profile_revision == 0 {
                claim.profile_revision = checkpoint.profile_revision.revision;
            }
        }
        Ok(checkpoint)
    }

    pub fn load_profile(&self) -> Result<AutomationProfile, AutomationRunError> {
        let checkpoint = self.load()?;
        self.load_profile_revision(&checkpoint.profile_digest)
    }

    pub fn load_profile_revision(
        &self,
        digest: &str,
    ) -> Result<AutomationProfile, AutomationRunError> {
        if !is_sha256(digest) {
            return Err(AutomationRunError::ProfileDigestMismatch);
        }
        let versioned = self
            .root
            .join("automation-profiles")
            .join(format!("{digest}.json"));
        let path = if versioned.exists() {
            versioned
        } else {
            let checkpoint = self.load()?;
            if checkpoint.profile_digest != digest {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("Automation profile revision `{digest}` is unavailable"),
                )
                .into());
            }
            self.root.join("automation-profile.json")
        };
        let profile: AutomationProfile = read_json(&path)?;
        let canonical_profile = serde_json::to_value(&profile)
            .map_err(std::io::Error::other)
            .and_then(|value| crate::canonical_sha256(&value).map_err(std::io::Error::other))?;
        if canonical_profile != digest {
            return Err(AutomationRunError::ProfileDigestMismatch);
        }
        Ok(profile)
    }

    pub fn stage_profile_revision(
        &self,
        checkpoint: &mut AutomationRootCheckpoint,
        profile: &AutomationProfile,
        profile_digest: &str,
    ) -> Result<(), AutomationRunError> {
        let canonical_profile = serde_json::to_value(profile)
            .map_err(std::io::Error::other)
            .and_then(|value| crate::canonical_sha256(&value).map_err(std::io::Error::other))?;
        if canonical_profile != profile_digest {
            return Err(AutomationRunError::ProfileDigestMismatch);
        }
        if profile.tracker.project_slug != checkpoint.tracker_project_slug {
            return Err(AutomationRunError::ProfileProjectMismatch);
        }
        self.ensure_active_profile_snapshot(checkpoint)?;
        self.write_versioned_profile_snapshot(profile_digest, profile)?;
        checkpoint.profile_revision.status = AutomationProfileRevisionStatus::PendingValid;
        checkpoint.profile_revision.pending_digest = Some(profile_digest.into());
        checkpoint.profile_revision.rejected_digest = None;
        checkpoint.profile_revision.diagnostics.clear();
        checkpoint.next_action = "valid profile revision pending future dispatch".into();
        self.save(checkpoint)
    }

    pub fn confirm_active_profile(
        &self,
        checkpoint: &mut AutomationRootCheckpoint,
    ) -> Result<(), AutomationRunError> {
        checkpoint.profile_revision.status = AutomationProfileRevisionStatus::Active;
        checkpoint.profile_revision.pending_digest = None;
        checkpoint.profile_revision.rejected_digest = None;
        checkpoint.profile_revision.diagnostics.clear();
        checkpoint.next_action = "active profile revision confirmed".into();
        self.save(checkpoint)
    }

    pub fn reject_profile_revision(
        &self,
        checkpoint: &mut AutomationRootCheckpoint,
        rejected_digest: Option<&str>,
        diagnostics: &[String],
    ) -> Result<(), AutomationRunError> {
        checkpoint.profile_revision.status = AutomationProfileRevisionStatus::Rejected;
        checkpoint.profile_revision.pending_digest = None;
        checkpoint.profile_revision.rejected_digest = rejected_digest.map(str::to_owned);
        checkpoint.profile_revision.diagnostics = diagnostics
            .iter()
            .take(16)
            .map(|diagnostic| bounded_preview(diagnostic, 512))
            .collect();
        checkpoint.next_action =
            "profile reload rejected; last-known-good revision remains active".into();
        self.save(checkpoint)
    }

    pub fn claim_fixture(
        &self,
        checkpoint: &mut AutomationRootCheckpoint,
        issue: &AutomationIssue,
        attempt: u32,
    ) -> Result<String, AutomationRunError> {
        let profile_digest = checkpoint.profile_digest.clone();
        let profile_revision = checkpoint.profile_revision.revision;
        self.claim_fixture_with_profile(
            checkpoint,
            issue,
            attempt,
            &profile_digest,
            profile_revision,
        )
    }

    fn claim_fixture_with_profile(
        &self,
        checkpoint: &mut AutomationRootCheckpoint,
        issue: &AutomationIssue,
        attempt: u32,
        profile_digest: &str,
        profile_revision: u64,
    ) -> Result<String, AutomationRunError> {
        if checkpoint
            .claims
            .values()
            .any(|claim| claim.issue_id == issue.id)
        {
            return Err(AutomationRunError::DuplicateIssue(issue.identifier.clone()));
        }
        let claim_id = format!(
            "claim-{}",
            &sha256(format!("{}\0{attempt}", issue.id).as_bytes())[..16]
        );
        let workspace_root = canonical_or_lexical(&checkpoint.workspace_root)?;
        let issue_hash = &sha256(issue.id.as_bytes())[..12];
        let worktree = workspace_root.join(format!(
            "{}-{issue_hash}-a{attempt}",
            safe_segment(&issue.identifier)
        ));
        if !worktree.starts_with(&workspace_root) || worktree == workspace_root {
            return Err(AutomationRunError::UnsafeWorkspace);
        }
        checkpoint.claims.insert(
            claim_id.clone(),
            AutomationIssueClaim {
                claim_id: claim_id.clone(),
                issue_id: issue.id.clone(),
                issue_identifier: issue.identifier.clone(),
                issue_title: issue.title.clone(),
                tracker_state: issue.state.clone(),
                priority: issue.priority,
                attempt,
                profile_digest: profile_digest.into(),
                profile_revision,
                task_prompt: String::new(),
                workflow_invocations: 0,
                turns_in_window: 0,
                continuation_count: 0,
                retry_attempt: 0,
                last_progress_at_ms: None,
                retry: None,
                status: AutomationClaimStatus::Claimed,
                worktree,
                source_revision: checkpoint.source_revision.clone(),
                issue_task: None,
                workflow_run_id: None,
                workflow_status: None,
                effects: Vec::new(),
                hook_receipts: Vec::new(),
                cleanup: AutomationCleanupState::default(),
                next_action: "create persistent issue worktree and native Issue task".into(),
            },
        );
        checkpoint.next_action = format!("start claim `{claim_id}`");
        self.save(checkpoint)?;
        Ok(claim_id)
    }

    /// Reconcile normalized tracker pages into a deterministic queue and claim
    /// as much eligible work as the effective profile permits. A saturated
    /// state is skipped rather than ending the scan, so capacity in another
    /// state remains usable.
    pub fn coordinate_fixture(
        &self,
        checkpoint: &mut AutomationRootCheckpoint,
        profile: &AutomationProfile,
        issues: &[AutomationIssue],
        attempt: u32,
    ) -> Result<AutomationCoordinationResult, AutomationRunError> {
        let canonical_profile = serde_json::to_value(profile)
            .map_err(std::io::Error::other)
            .and_then(|value| crate::canonical_sha256(&value).map_err(std::io::Error::other))?;
        let dispatch_profile_digest = checkpoint
            .profile_revision
            .pending_digest
            .clone()
            .unwrap_or_else(|| checkpoint.profile_digest.clone());
        if canonical_profile != dispatch_profile_digest {
            return Err(AutomationRunError::ProfileDigestMismatch);
        }
        let dispatch_profile_revision = if checkpoint.profile_revision.pending_digest.is_some() {
            checkpoint.profile_revision.revision.saturating_add(1)
        } else {
            checkpoint.profile_revision.revision
        };
        let mut observations = BTreeMap::<String, AutomationIssue>::new();
        for issue in issues {
            observations
                .entry(issue.id.clone())
                .and_modify(|current| {
                    if issue_observation_key(issue) > issue_observation_key(current) {
                        *current = issue.clone();
                    }
                })
                .or_insert_with(|| issue.clone());
        }

        checkpoint.queue.clear();
        for claim in checkpoint.claims.values_mut() {
            if let Some(issue) = observations.get(&claim.issue_id) {
                claim.tracker_state = issue.state.clone();
                if is_terminal_state(profile, &issue.state) && claim_is_active(claim.status) {
                    claim.next_action =
                        "reconcile externally terminal tracker state before dispatch".into();
                }
            }
        }

        let claimed_issue_ids = checkpoint
            .claims
            .values()
            .filter(|claim| claim_is_active(claim.status))
            .map(|claim| claim.issue_id.clone())
            .collect::<std::collections::BTreeSet<_>>();
        let required_labels = profile
            .tracker
            .required_labels
            .iter()
            .map(|label| label.trim().to_ascii_lowercase())
            .collect::<std::collections::BTreeSet<_>>();
        let mut eligible = Vec::new();
        for issue in observations.values() {
            if claimed_issue_ids.contains(&issue.id) {
                continue;
            }
            if is_terminal_state(profile, &issue.state) {
                checkpoint.queue.insert(
                    issue.id.clone(),
                    queue_item(
                        issue,
                        AutomationQueueStatus::Terminal,
                        "tracker state is terminal",
                    ),
                );
                continue;
            }
            if !is_active_state(profile, &issue.state) {
                continue;
            }
            let labels = issue
                .labels
                .iter()
                .map(|label| label.trim().to_ascii_lowercase())
                .collect::<std::collections::BTreeSet<_>>();
            if !required_labels.is_subset(&labels) {
                continue;
            }
            if has_nonterminal_blocker(profile, issue) {
                checkpoint.queue.insert(
                    issue.id.clone(),
                    queue_item(
                        issue,
                        AutomationQueueStatus::Blocked,
                        "waiting for a nonterminal blocker",
                    ),
                );
                continue;
            }
            eligible.push(issue.clone());
        }
        eligible.sort_by(|left, right| dispatch_key(left).cmp(&dispatch_key(right)));

        let mut active_total = checkpoint
            .claims
            .values()
            .filter(|claim| claim_is_active(claim.status))
            .count() as u32;
        let mut active_by_state = BTreeMap::<String, u32>::new();
        for claim in checkpoint
            .claims
            .values()
            .filter(|claim| claim_is_active(claim.status))
        {
            *active_by_state
                .entry(claim.tracker_state.to_ascii_lowercase())
                .or_default() += 1;
        }

        let mut dispatched_claim_ids = Vec::new();
        for issue in eligible {
            if active_total >= profile.agent.max_concurrent_agents {
                checkpoint.queue.insert(
                    issue.id.clone(),
                    queue_item(
                        &issue,
                        AutomationQueueStatus::Queued,
                        "waiting for global capacity",
                    ),
                );
                continue;
            }
            let state_key = issue.state.to_ascii_lowercase();
            let state_limit = state_limit(profile, &issue.state);
            let state_active = active_by_state.get(&state_key).copied().unwrap_or_default();
            if state_limit.is_some_and(|limit| state_active >= limit) {
                checkpoint.queue.insert(
                    issue.id.clone(),
                    queue_item(
                        &issue,
                        AutomationQueueStatus::Queued,
                        "waiting for state capacity",
                    ),
                );
                continue;
            }
            let claim_id = self.claim_fixture_with_profile(
                checkpoint,
                &issue,
                attempt,
                &dispatch_profile_digest,
                dispatch_profile_revision,
            )?;
            dispatched_claim_ids.push(claim_id);
            active_total += 1;
            *active_by_state.entry(state_key).or_default() += 1;
        }
        if !dispatched_claim_ids.is_empty()
            && checkpoint.profile_revision.pending_digest.as_deref()
                == Some(dispatch_profile_digest.as_str())
        {
            self.write_active_profile_snapshot(&dispatch_profile_digest, profile)?;
            checkpoint.profile_digest = dispatch_profile_digest;
            checkpoint.profile_revision.revision = dispatch_profile_revision;
            checkpoint.profile_revision.status = AutomationProfileRevisionStatus::Active;
            checkpoint.profile_revision.pending_digest = None;
            checkpoint.profile_revision.rejected_digest = None;
            checkpoint.profile_revision.diagnostics.clear();
        }
        checkpoint.next_action = if dispatched_claim_ids.is_empty() {
            "queue reconciled; wait for eligible capacity or tracker changes".into()
        } else {
            format!(
                "start {} deterministically selected claim(s)",
                dispatched_claim_ids.len()
            )
        };
        self.save(checkpoint)?;
        Ok(AutomationCoordinationResult {
            dispatched_claim_ids,
            counts: automation_queue_counts(checkpoint),
        })
    }

    pub fn queue_page(
        &self,
        checkpoint: &AutomationRootCheckpoint,
        category: AutomationQueueCategory,
        offset: u32,
        limit: u32,
    ) -> AutomationQueuePage {
        automation_queue_page(checkpoint, category, offset, limit)
    }

    pub fn update_claim<F>(
        &self,
        checkpoint: &mut AutomationRootCheckpoint,
        claim_id: &str,
        update: F,
    ) -> Result<(), AutomationRunError>
    where
        F: FnOnce(&mut AutomationIssueClaim),
    {
        let claim = checkpoint
            .claims
            .get_mut(claim_id)
            .ok_or_else(|| AutomationRunError::MissingClaim(claim_id.into()))?;
        update(claim);
        self.save(checkpoint)
    }

    pub fn record_hook_receipt(
        &self,
        checkpoint: &mut AutomationRootCheckpoint,
        claim_id: &str,
        kind: AutomationHookKind,
        command: Option<&str>,
        outcome: Result<&crate::CommandOutcome, &str>,
    ) -> Result<AutomationHookReceipt, AutomationRunError> {
        let claim = checkpoint
            .claims
            .get_mut(claim_id)
            .ok_or_else(|| AutomationRunError::MissingClaim(claim_id.into()))?;
        let invocation = claim
            .hook_receipts
            .iter()
            .filter(|receipt| receipt.kind == kind)
            .map(|receipt| receipt.invocation)
            .max()
            .unwrap_or_default()
            .saturating_add(1);
        let receipt = match (command, outcome) {
            (None, _) => AutomationHookReceipt {
                kind,
                invocation,
                command_sha256: None,
                status: AutomationHookStatus::Skipped,
                exit_code: None,
                stdout_preview: String::new(),
                stderr_preview: String::new(),
                failure: None,
            },
            (Some(command), Ok(outcome)) => AutomationHookReceipt {
                kind,
                invocation,
                command_sha256: Some(sha256(command.as_bytes())),
                status: if outcome.exit_code == 0 {
                    AutomationHookStatus::Succeeded
                } else {
                    AutomationHookStatus::Failed
                },
                exit_code: Some(outcome.exit_code),
                stdout_preview: bounded_preview(&outcome.stdout, 512),
                stderr_preview: bounded_preview(&outcome.stderr, 512),
                failure: (outcome.exit_code != 0).then(|| {
                    bounded_preview(
                        if outcome.stderr.trim().is_empty() {
                            "hook command returned a non-zero exit code"
                        } else {
                            &outcome.stderr
                        },
                        512,
                    )
                }),
            },
            (Some(command), Err(error)) => AutomationHookReceipt {
                kind,
                invocation,
                command_sha256: Some(sha256(command.as_bytes())),
                status: AutomationHookStatus::Failed,
                exit_code: None,
                stdout_preview: String::new(),
                stderr_preview: String::new(),
                failure: Some(bounded_preview(error, 512)),
            },
        };
        claim.hook_receipts.push(receipt.clone());
        if claim.hook_receipts.len() > 32 {
            claim.hook_receipts.remove(0);
        }
        self.save(checkpoint)?;
        Ok(receipt)
    }

    pub fn record_cleanup_attempt(
        &self,
        checkpoint: &mut AutomationRootCheckpoint,
        claim_id: &str,
        result: Result<(), &str>,
    ) -> Result<(), AutomationRunError> {
        let claim = checkpoint
            .claims
            .get_mut(claim_id)
            .ok_or_else(|| AutomationRunError::MissingClaim(claim_id.into()))?;
        if !matches!(
            claim.cleanup.status,
            AutomationCleanupStatus::Eligible | AutomationCleanupStatus::RetryPending
        ) {
            return Err(AutomationRunError::InactiveClaim(claim_id.into()));
        }
        claim.cleanup.attempts = claim.cleanup.attempts.saturating_add(1);
        match result {
            Ok(()) => {
                claim.cleanup.status = AutomationCleanupStatus::Removed;
                claim.cleanup.last_failure = None;
                claim.next_action = "terminal Issue resources removed; evidence retained".into();
            }
            Err(error) => {
                claim.cleanup.status = AutomationCleanupStatus::RetryPending;
                claim.cleanup.last_failure = Some(bounded_preview(error, 512));
                claim.next_action = "retry terminal Issue worktree cleanup".into();
            }
        }
        self.save(checkpoint)
    }

    pub fn record_claim_progress(
        &self,
        checkpoint: &mut AutomationRootCheckpoint,
        claim_id: &str,
        now_ms: u64,
    ) -> Result<(), AutomationRunError> {
        let claim = active_claim_mut(checkpoint, claim_id)?;
        claim.last_progress_at_ms = Some(now_ms);
        claim.retry = None;
        claim.status = AutomationClaimStatus::Running;
        claim.next_action = "Workflow progress recorded in the native Issue task".into();
        self.save(checkpoint)
    }

    pub fn schedule_claim_retry(
        &self,
        checkpoint: &mut AutomationRootCheckpoint,
        claim_id: &str,
        profile: &AutomationProfile,
        now_ms: u64,
        reason: &str,
    ) -> Result<AutomationRetrySchedule, AutomationRunError> {
        let claim = active_claim_mut(checkpoint, claim_id)?;
        claim.retry_attempt = claim.retry_attempt.saturating_add(1);
        let delay_ms = retry_delay_ms(
            profile.polling.interval_ms,
            profile.agent.max_retry_backoff_ms,
            claim.retry_attempt,
        );
        let schedule = AutomationRetrySchedule {
            kind: AutomationRetryKind::Retry,
            attempt: claim.retry_attempt,
            delay_ms,
            ready_at_ms: now_ms.saturating_add(delay_ms),
            reset_turn_window: false,
            reason: bounded_preview(reason, 240),
        };
        claim.retry = Some(schedule.clone());
        claim.next_action = format!(
            "retry attempt {} in {}ms: {}",
            schedule.attempt, schedule.delay_ms, schedule.reason
        );
        self.save(checkpoint)?;
        Ok(schedule)
    }

    pub fn record_completed_invocation(
        &self,
        checkpoint: &mut AutomationRootCheckpoint,
        claim_id: &str,
        profile: &AutomationProfile,
        tracker_issue_active: bool,
        now_ms: u64,
    ) -> Result<Option<AutomationRetrySchedule>, AutomationRunError> {
        let claim = active_claim_mut(checkpoint, claim_id)?;
        claim.workflow_invocations = claim.workflow_invocations.saturating_add(1);
        claim.turns_in_window = claim.turns_in_window.saturating_add(1);
        claim.retry_attempt = 0;
        claim.last_progress_at_ms = Some(now_ms);
        if !tracker_issue_active {
            claim.retry = None;
            claim.status = AutomationClaimStatus::Completed;
            claim.next_action = "claim complete; tracker issue is no longer active".into();
            self.save(checkpoint)?;
            return Ok(None);
        }

        let reset_turn_window = claim.turns_in_window >= profile.agent.max_turns;
        let delay_ms = profile
            .polling
            .interval_ms
            .min(profile.agent.max_retry_backoff_ms);
        let schedule = AutomationRetrySchedule {
            kind: AutomationRetryKind::Continuation,
            attempt: claim.continuation_count.saturating_add(1),
            delay_ms,
            ready_at_ms: now_ms.saturating_add(delay_ms),
            reset_turn_window,
            reason: if reset_turn_window {
                "Workflow turn window exhausted while the tracker issue remains active".into()
            } else {
                "tracker issue remains active after a completed Workflow invocation".into()
            },
        };
        claim.retry = Some(schedule.clone());
        claim.status = AutomationClaimStatus::Running;
        claim.next_action = if reset_turn_window {
            format!(
                "continuation retry in {}ms after max_turns",
                schedule.delay_ms
            )
        } else {
            format!("continue claim in {}ms", schedule.delay_ms)
        };
        self.save(checkpoint)?;
        Ok(Some(schedule))
    }

    pub fn dispatch_due_claim_work(
        &self,
        checkpoint: &mut AutomationRootCheckpoint,
        claim_id: &str,
        tracker_issue_active: bool,
        now_ms: u64,
    ) -> Result<AutomationRetrySchedule, AutomationRunError> {
        let claim = active_claim_mut(checkpoint, claim_id)?;
        if !tracker_issue_active {
            claim.retry = None;
            claim.status = AutomationClaimStatus::Cancelled;
            claim.next_action = "tracker issue became terminal; stop future invocations".into();
            self.save(checkpoint)?;
            return Err(AutomationRunError::InactiveClaim(claim_id.into()));
        }
        let schedule = claim
            .retry
            .clone()
            .filter(|retry| retry.ready_at_ms <= now_ms)
            .ok_or_else(|| AutomationRunError::RetryNotReady(claim_id.into()))?;
        if schedule.reset_turn_window {
            claim.turns_in_window = 0;
            claim.continuation_count = claim.continuation_count.saturating_add(1);
        }
        claim.retry = None;
        claim.status = AutomationClaimStatus::Running;
        claim.last_progress_at_ms = Some(now_ms);
        claim.next_action = "invoke the selected typed Workflow in the retained Issue task".into();
        self.save(checkpoint)?;
        Ok(schedule)
    }

    /// Fence all work before descendants are interrupted. Advancing the lease
    /// epoch makes every previously loaded checkpoint and in-flight provider
    /// callback unable to commit.
    pub fn pause(
        &self,
        checkpoint: &mut AutomationRootCheckpoint,
        reason: &str,
    ) -> Result<(), AutomationRunError> {
        let expected_epoch = checkpoint.lease_epoch;
        let expected_revision = checkpoint.revision;
        self.ensure_fresh(expected_epoch, expected_revision)?;
        checkpoint.lease_epoch = checkpoint.lease_epoch.saturating_add(1);
        checkpoint.status = AutomationRootStatus::Suspended;
        checkpoint.reconciliation = AutomationReconciliationStatus::Required;
        checkpoint.next_action =
            format!("Automation fenced for {reason}; reconcile retained work before dispatch");
        for claim in checkpoint.claims.values_mut() {
            if matches!(
                claim.status,
                AutomationClaimStatus::Claimed | AutomationClaimStatus::Running
            ) {
                claim.status = AutomationClaimStatus::Suspended;
                claim.next_action =
                    "inspect retained Issue task and Workflow during reconciliation".into();
            }
            for effect in &mut claim.effects {
                if effect.status == AutomationEffectStatus::Executing {
                    effect.status = AutomationEffectStatus::Ambiguous;
                    effect.failure = Some(
                        "Automation was fenced before the provider result became durable".into(),
                    );
                }
            }
        }
        self.persist(checkpoint, expected_epoch, expected_revision)?;
        self.write_lease(checkpoint)?;
        Ok(())
    }

    pub fn begin_reconciliation(
        &self,
        checkpoint: &mut AutomationRootCheckpoint,
    ) -> Result<(), AutomationRunError> {
        if checkpoint.status != AutomationRootStatus::Suspended {
            return Err(AutomationRunError::NotSuspended);
        }
        checkpoint.reconciliation = AutomationReconciliationStatus::InProgress;
        checkpoint.next_action =
            "reconcile profile, lease, tracker, worktrees, tasks, Child Runs, and effects".into();
        self.save(checkpoint)
    }

    /// Complete the fenced reconciliation pass. Existing native identities and
    /// receipts are only observed and retained here; this method never creates
    /// a replacement worktree, task, Child Run, or provider mutation.
    pub fn reconcile(
        &self,
        checkpoint: &mut AutomationRootCheckpoint,
        profile: &AutomationProfile,
        tracker_issues: &[AutomationIssue],
        observations: &[AutomationClaimReconciliation],
    ) -> Result<(), AutomationRunError> {
        if checkpoint.status != AutomationRootStatus::Suspended {
            return Err(AutomationRunError::NotSuspended);
        }
        if checkpoint.reconciliation != AutomationReconciliationStatus::InProgress {
            return Err(AutomationRunError::ReconciliationNotStarted);
        }
        let canonical_profile = serde_json::to_value(profile)
            .map_err(std::io::Error::other)
            .and_then(|value| crate::canonical_sha256(&value).map_err(std::io::Error::other))?;
        if canonical_profile != checkpoint.profile_digest {
            return Err(AutomationRunError::ProfileDigestMismatch);
        }
        self.verify_lease(checkpoint)?;

        let issues = tracker_issues
            .iter()
            .map(|issue| (issue.id.as_str(), issue))
            .collect::<BTreeMap<_, _>>();
        let observed = observations
            .iter()
            .map(|observation| (observation.claim_id.as_str(), observation))
            .collect::<BTreeMap<_, _>>();
        let mut blockers = Vec::new();
        for claim in checkpoint.claims.values_mut() {
            if !claim_is_active(claim.status) {
                continue;
            }
            let Some(issue) = issues.get(claim.issue_id.as_str()) else {
                blockers.push(format!("{} tracker state", claim.issue_identifier));
                claim.next_action = "refresh tracker state before dispatch".into();
                continue;
            };
            claim.tracker_state = issue.state.clone();
            claim.priority = issue.priority;
            let Some(observation) = observed.get(claim.claim_id.as_str()) else {
                blockers.push(format!("{} native descendants", claim.issue_identifier));
                claim.next_action = "inspect retained Issue task and Child Run".into();
                continue;
            };
            if observation.tracker_terminal {
                if !observation.descendants_cancelled {
                    blockers.push(format!(
                        "{} descendants still active",
                        claim.issue_identifier
                    ));
                    claim.next_action = "cancel descendants before terminal reconciliation".into();
                    continue;
                }
                if claim.effects.iter().any(|effect| {
                    matches!(
                        effect.status,
                        AutomationEffectStatus::Executing | AutomationEffectStatus::Ambiguous
                    )
                }) {
                    blockers.push(format!("{} ambiguous effects", claim.issue_identifier));
                    claim.next_action =
                        "resolve ambiguous Tracker effects before cleanup eligibility".into();
                    continue;
                }
                claim.status = AutomationClaimStatus::Cancelled;
                claim.workflow_status = observation.workflow_status.clone();
                claim.cleanup.status = AutomationCleanupStatus::Eligible;
                claim.cleanup.last_failure = None;
                claim.next_action =
                    "externally terminal; retained resources are cleanup eligible".into();
                continue;
            }
            if claim.issue_task.is_some() && !observation.issue_task_active {
                blockers.push(format!("{} missing Issue task", claim.issue_identifier));
                claim.next_action = "inspect missing Issue task; do not create a duplicate".into();
                continue;
            }
            if claim.worktree.exists() == false && claim.issue_task.is_some() {
                blockers.push(format!("{} missing worktree", claim.issue_identifier));
                claim.next_action = "inspect missing worktree; do not create a duplicate".into();
                continue;
            }
            claim.workflow_status = observation.workflow_status.clone();
            claim.status = AutomationClaimStatus::Suspended;
            claim.next_action = if claim.workflow_run_id.is_some() {
                "resume the existing Child Run from its native checkpoint".into()
            } else if claim.issue_task.is_some() {
                "continue the existing Issue task without respawning it".into()
            } else {
                "continue the retained claim without duplicating resources".into()
            };
        }
        if blockers.is_empty() {
            checkpoint.status = AutomationRootStatus::Running;
            checkpoint.reconciliation = AutomationReconciliationStatus::Complete;
            checkpoint.next_action =
                "reconciliation complete; eligible dispatch may continue".into();
            self.save(checkpoint)?;
            return Ok(());
        }
        checkpoint.reconciliation = AutomationReconciliationStatus::Blocked;
        checkpoint.next_action = format!("reconciliation blocked: {}", blockers.join(", "));
        self.save(checkpoint)?;
        Err(AutomationRunError::ReconciliationBlocked(
            blockers.join(", "),
        ))
    }

    pub fn cancel(
        &self,
        checkpoint: &mut AutomationRootCheckpoint,
    ) -> Result<(), AutomationRunError> {
        checkpoint.status = AutomationRootStatus::Cancelled;
        checkpoint.next_action =
            "Automation cancelled; retain checkpoints and worktrees for inspection".into();
        for claim in checkpoint.claims.values_mut() {
            if claim_is_active(claim.status) {
                claim.status = AutomationClaimStatus::Cancelled;
                claim.retry = None;
                claim.next_action = "inspect or explicitly remove retained worktree".into();
            }
        }
        self.save(checkpoint)?;
        if self.lease_path.exists() {
            fs::remove_file(&self.lease_path)?;
        }
        Ok(())
    }

    /// Fence one Issue claim before native descendants are interrupted. The
    /// root revision is advanced by the durable save, so any provider callback
    /// prepared from the previous revision cannot commit after this point.
    pub fn begin_claim_cancellation(
        &self,
        checkpoint: &mut AutomationRootCheckpoint,
        claim_id: &str,
    ) -> Result<(), AutomationRunError> {
        let claim = checkpoint
            .claims
            .get_mut(claim_id)
            .ok_or_else(|| AutomationRunError::MissingClaim(claim_id.into()))?;
        if matches!(
            claim.status,
            AutomationClaimStatus::Completed
                | AutomationClaimStatus::Cancelled
                | AutomationClaimStatus::Failed
        ) {
            return Ok(());
        }
        claim.status = AutomationClaimStatus::Suspended;
        claim.retry = None;
        claim.next_action = "cancel native Issue descendants, then reconcile effects".into();
        for effect in &mut claim.effects {
            if effect.status == AutomationEffectStatus::Executing {
                effect.status = AutomationEffectStatus::Ambiguous;
                effect.failure = Some(
                    "Issue cancellation fenced the claim before the provider result became durable"
                        .into(),
                );
            }
        }
        checkpoint.next_action = format!("finish cancellation for claim `{claim_id}`");
        self.save(checkpoint)
    }

    pub fn complete_claim_cancellation(
        &self,
        checkpoint: &mut AutomationRootCheckpoint,
        claim_id: &str,
        descendants_cancelled: bool,
    ) -> Result<(), AutomationRunError> {
        let claim = checkpoint
            .claims
            .get_mut(claim_id)
            .ok_or_else(|| AutomationRunError::MissingClaim(claim_id.into()))?;
        if claim.status == AutomationClaimStatus::Cancelled {
            return Ok(());
        }
        if !descendants_cancelled {
            claim.status = AutomationClaimStatus::Suspended;
            claim.next_action = "retry native Issue descendant cancellation".into();
        } else if claim.effects.iter().any(|effect| {
            matches!(
                effect.status,
                AutomationEffectStatus::Executing | AutomationEffectStatus::Ambiguous
            )
        }) {
            claim.status = AutomationClaimStatus::Suspended;
            claim.next_action =
                "reconcile ambiguous Tracker effects before cancellation cleanup".into();
        } else {
            claim.status = AutomationClaimStatus::Cancelled;
            claim.retry = None;
            claim.cleanup.status = AutomationCleanupStatus::Eligible;
            claim.cleanup.last_failure = None;
            claim.next_action = "Issue claim cancelled; terminal cleanup is eligible".into();
        }
        checkpoint.next_action = claim.next_action.clone();
        self.save(checkpoint)
    }

    pub fn prepare_tracker_comment(
        &self,
        checkpoint: &mut AutomationRootCheckpoint,
        claim_id: &str,
        profile: &AutomationProfile,
        body: &str,
        gate_policy: AutomationGatePolicy,
    ) -> Result<
        (
            AutomationEffectReceipt,
            Option<AutomationTrackerCommentRequest>,
        ),
        AutomationRunError,
    > {
        let body = body.trim();
        if body.is_empty() || body.len() > 4096 {
            return Err(AutomationRunError::InvalidComment);
        }
        let prepared = self.prepare_tracker_effect(
            checkpoint,
            claim_id,
            profile,
            AutomationEffect::TrackerComment,
            body,
            gate_policy,
            None,
        )?;
        let request = prepared.execute.then(|| AutomationTrackerCommentRequest {
            effect_id: prepared.receipt.effect_id.clone(),
            idempotency_key: prepared.receipt.idempotency_key.clone(),
            claim_id: claim_id.into(),
            tracker_project_slug: prepared.receipt.tracker_project_slug.clone(),
            issue_id: prepared.receipt.issue_id.clone(),
            body: body.into(),
        });
        Ok((prepared.receipt, request))
    }

    pub fn resolve_tracker_comment<F>(
        &self,
        checkpoint: &mut AutomationRootCheckpoint,
        claim_id: &str,
        profile: &AutomationProfile,
        body: &str,
        gate_policy: AutomationGatePolicy,
        execute: F,
    ) -> Result<AutomationEffectReceipt, AutomationRunError>
    where
        F: FnOnce(&AutomationTrackerCommentRequest) -> AutomationEffectExecution,
    {
        let (receipt, request) =
            self.prepare_tracker_comment(checkpoint, claim_id, profile, body, gate_policy)?;
        let Some(request) = request else {
            return Ok(receipt);
        };
        let execution = execute(&request);
        self.complete_tracker_effect(checkpoint, claim_id, &request.idempotency_key, execution)
    }

    pub fn complete_tracker_transition(
        &self,
        checkpoint: &mut AutomationRootCheckpoint,
        claim_id: &str,
        request: &AutomationTrackerTransitionRequest,
        execution: AutomationEffectExecution,
    ) -> Result<AutomationEffectReceipt, AutomationRunError> {
        let receipt = self.complete_tracker_effect(
            checkpoint,
            claim_id,
            &request.idempotency_key,
            execution,
        )?;
        if receipt.status == AutomationEffectStatus::Committed {
            let claim = checkpoint
                .claims
                .get_mut(claim_id)
                .ok_or_else(|| AutomationRunError::MissingClaim(claim_id.into()))?;
            if claim.tracker_state != request.target_state {
                claim.tracker_state.clone_from(&request.target_state);
                self.save(checkpoint)?;
            }
        }
        Ok(receipt)
    }

    pub fn prepare_tracker_transition(
        &self,
        checkpoint: &mut AutomationRootCheckpoint,
        claim_id: &str,
        profile: &AutomationProfile,
        refreshed_state: &str,
        target_state: &str,
        gate_policy: AutomationGatePolicy,
    ) -> Result<
        (
            AutomationEffectReceipt,
            Option<AutomationTrackerTransitionRequest>,
        ),
        AutomationRunError,
    > {
        let refreshed_state = refreshed_state.trim();
        let target_state = target_state.trim();
        let configured_target = profile
            .tracker
            .active_states
            .iter()
            .chain(profile.tracker.terminal_states.iter())
            .find(|state| state.eq_ignore_ascii_case(target_state))
            .cloned()
            .ok_or(AutomationRunError::InvalidTransition)?;
        let claim = checkpoint
            .claims
            .get_mut(claim_id)
            .ok_or_else(|| AutomationRunError::MissingClaim(claim_id.into()))?;
        claim.tracker_state = refreshed_state.into();
        let already_applied = refreshed_state.eq_ignore_ascii_case(&configured_target);
        if !already_applied
            && profile
                .tracker
                .terminal_states
                .iter()
                .any(|state| state.eq_ignore_ascii_case(refreshed_state))
        {
            return Err(AutomationRunError::InactiveClaim(claim_id.into()));
        }
        let prepared = self.prepare_tracker_effect(
            checkpoint,
            claim_id,
            profile,
            AutomationEffect::TrackerTransition,
            &configured_target,
            gate_policy,
            already_applied.then(|| format!("already-applied:{configured_target}")),
        )?;
        let request = prepared
            .execute
            .then(|| AutomationTrackerTransitionRequest {
                effect_id: prepared.receipt.effect_id.clone(),
                idempotency_key: prepared.receipt.idempotency_key.clone(),
                claim_id: claim_id.into(),
                tracker_project_slug: prepared.receipt.tracker_project_slug.clone(),
                issue_id: prepared.receipt.issue_id.clone(),
                expected_state: refreshed_state.into(),
                target_state: configured_target,
            });
        Ok((prepared.receipt, request))
    }

    pub fn resolve_tracker_transition<F>(
        &self,
        checkpoint: &mut AutomationRootCheckpoint,
        claim_id: &str,
        profile: &AutomationProfile,
        refreshed_state: &str,
        target_state: &str,
        gate_policy: AutomationGatePolicy,
        execute: F,
    ) -> Result<AutomationEffectReceipt, AutomationRunError>
    where
        F: FnOnce(&AutomationTrackerTransitionRequest) -> AutomationEffectExecution,
    {
        let (receipt, request) = self.prepare_tracker_transition(
            checkpoint,
            claim_id,
            profile,
            refreshed_state,
            target_state,
            gate_policy,
        )?;
        let Some(request) = request else {
            return Ok(receipt);
        };
        let execution = execute(&request);
        self.complete_tracker_transition(checkpoint, claim_id, &request, execution)
    }

    pub fn prepare_tracker_pull_request_link(
        &self,
        checkpoint: &mut AutomationRootCheckpoint,
        claim_id: &str,
        profile: &AutomationProfile,
        pull_request_url: &str,
        gate_policy: AutomationGatePolicy,
    ) -> Result<
        (
            AutomationEffectReceipt,
            Option<AutomationTrackerPullRequestLinkRequest>,
        ),
        AutomationRunError,
    > {
        let normalized = normalize_pull_request_url(pull_request_url)
            .ok_or(AutomationRunError::InvalidPullRequestLink)?;
        let prepared = self.prepare_tracker_effect(
            checkpoint,
            claim_id,
            profile,
            AutomationEffect::TrackerLinkPullRequest,
            &normalized,
            gate_policy,
            None,
        )?;
        let request = prepared
            .execute
            .then(|| AutomationTrackerPullRequestLinkRequest {
                effect_id: prepared.receipt.effect_id.clone(),
                idempotency_key: prepared.receipt.idempotency_key.clone(),
                claim_id: claim_id.into(),
                tracker_project_slug: prepared.receipt.tracker_project_slug.clone(),
                issue_id: prepared.receipt.issue_id.clone(),
                pull_request_url: normalized,
            });
        Ok((prepared.receipt, request))
    }

    pub fn resolve_tracker_pull_request_link<F>(
        &self,
        checkpoint: &mut AutomationRootCheckpoint,
        claim_id: &str,
        profile: &AutomationProfile,
        pull_request_url: &str,
        gate_policy: AutomationGatePolicy,
        execute: F,
    ) -> Result<AutomationEffectReceipt, AutomationRunError>
    where
        F: FnOnce(&AutomationTrackerPullRequestLinkRequest) -> AutomationEffectExecution,
    {
        let (receipt, request) = self.prepare_tracker_pull_request_link(
            checkpoint,
            claim_id,
            profile,
            pull_request_url,
            gate_policy,
        )?;
        let Some(request) = request else {
            return Ok(receipt);
        };
        let execution = execute(&request);
        self.complete_tracker_effect(checkpoint, claim_id, &request.idempotency_key, execution)
    }

    fn prepare_tracker_effect(
        &self,
        checkpoint: &mut AutomationRootCheckpoint,
        claim_id: &str,
        profile: &AutomationProfile,
        kind: AutomationEffect,
        request_value: &str,
        gate_policy: AutomationGatePolicy,
        already_applied_receipt: Option<String>,
    ) -> Result<PreparedAutomationEffect, AutomationRunError> {
        if !profile.orchestra.effects.contains(&kind) {
            return Err(AutomationRunError::MissingEffectAuthority);
        }
        let claim = checkpoint
            .claims
            .get_mut(claim_id)
            .ok_or_else(|| AutomationRunError::MissingClaim(claim_id.into()))?;
        let request_sha256 = sha256(request_value.as_bytes());
        let kind_name = automation_effect_name(kind);
        let idempotency_key = sha256(
            format!(
                "{}\0{}\0{}\0{}",
                claim.profile_digest, claim_id, kind_name, request_sha256
            )
            .as_bytes(),
        );
        if let Some(index) = claim
            .effects
            .iter()
            .position(|receipt| receipt.idempotency_key == idempotency_key)
        {
            if claim.effects[index].status == AutomationEffectStatus::Executing {
                claim.effects[index].status = AutomationEffectStatus::Ambiguous;
                claim.effects[index].failure =
                    Some("execution was interrupted before a durable provider receipt".into());
                let receipt = claim.effects[index].clone();
                self.save(checkpoint)?;
                return Ok(PreparedAutomationEffect {
                    receipt,
                    execute: false,
                });
            }
            return Ok(PreparedAutomationEffect {
                receipt: claim.effects[index].clone(),
                execute: false,
            });
        }
        if !matches!(
            claim.status,
            AutomationClaimStatus::Claimed | AutomationClaimStatus::Running
        ) {
            return Err(AutomationRunError::InactiveClaim(claim_id.into()));
        }
        let effect_id = format!("effect-{}", &idempotency_key[..16]);
        let receipt = AutomationEffectReceipt {
            effect_id: effect_id.clone(),
            idempotency_key: idempotency_key.clone(),
            kind,
            claim_id: claim_id.into(),
            tracker_project_slug: checkpoint.tracker_project_slug.clone(),
            issue_id: claim.issue_id.clone(),
            request_sha256,
            body_preview: bounded_preview(request_value, 240),
            gate_policy,
            status: already_applied_receipt.as_ref().map_or_else(
                || match gate_policy {
                    AutomationGatePolicy::AutoAccept => AutomationEffectStatus::Executing,
                    AutomationGatePolicy::AutoReject => AutomationEffectStatus::Rejected,
                    AutomationGatePolicy::AskHuman => AutomationEffectStatus::WaitingGate,
                },
                |_| AutomationEffectStatus::Committed,
            ),
            provider_receipt: already_applied_receipt,
            failure: None,
        };
        claim.effects.push(receipt.clone());
        self.save(checkpoint)?;
        Ok(PreparedAutomationEffect {
            execute: receipt.status == AutomationEffectStatus::Executing,
            receipt,
        })
    }

    pub fn complete_tracker_effect(
        &self,
        checkpoint: &mut AutomationRootCheckpoint,
        claim_id: &str,
        idempotency_key: &str,
        execution: AutomationEffectExecution,
    ) -> Result<AutomationEffectReceipt, AutomationRunError> {
        let claim = checkpoint
            .claims
            .get_mut(claim_id)
            .ok_or_else(|| AutomationRunError::MissingClaim(claim_id.into()))?;
        let receipt = claim
            .effects
            .iter_mut()
            .find(|receipt| receipt.idempotency_key == idempotency_key)
            .ok_or_else(|| AutomationRunError::MissingClaim(claim_id.into()))?;
        if receipt.status != AutomationEffectStatus::Executing {
            return Ok(receipt.clone());
        }
        match execution {
            AutomationEffectExecution::Committed { provider_receipt } => {
                receipt.status = AutomationEffectStatus::Committed;
                receipt.provider_receipt = Some(bounded_preview(&provider_receipt, 512));
            }
            AutomationEffectExecution::Failed { message } => {
                receipt.status = AutomationEffectStatus::Failed;
                receipt.failure = Some(bounded_preview(&message, 512));
            }
            AutomationEffectExecution::Ambiguous { message } => {
                receipt.status = AutomationEffectStatus::Ambiguous;
                receipt.failure = Some(bounded_preview(&message, 512));
            }
        }
        let receipt = receipt.clone();
        self.save(checkpoint)?;
        Ok(receipt)
    }

    pub fn save(
        &self,
        checkpoint: &mut AutomationRootCheckpoint,
    ) -> Result<(), AutomationRunError> {
        let expected_epoch = checkpoint.lease_epoch;
        let expected_revision = checkpoint.revision;
        self.persist(checkpoint, expected_epoch, expected_revision)
    }

    pub fn repository(&self) -> &Path {
        &self.repository
    }

    fn ensure_fresh(
        &self,
        expected_epoch: u64,
        expected_revision: u64,
    ) -> Result<(), AutomationRunError> {
        let path = self.root.join("automation-state.json");
        if !path.exists() {
            return Ok(());
        }
        let current: AutomationRootCheckpoint = read_json(&path)?;
        if current.lease_epoch != expected_epoch || current.revision != expected_revision {
            return Err(AutomationRunError::StaleLease {
                expected_epoch,
                expected_revision,
                actual_epoch: current.lease_epoch,
                actual_revision: current.revision,
            });
        }
        Ok(())
    }

    fn persist(
        &self,
        checkpoint: &mut AutomationRootCheckpoint,
        expected_epoch: u64,
        expected_revision: u64,
    ) -> Result<(), AutomationRunError> {
        self.ensure_fresh(expected_epoch, expected_revision)?;
        let previous_revision = checkpoint.revision;
        checkpoint.revision = expected_revision.saturating_add(1);
        if let Err(error) = atomic_json(&self.root.join("automation-state.json"), checkpoint) {
            checkpoint.revision = previous_revision;
            return Err(error.into());
        }
        Ok(())
    }

    fn write_lease(&self, checkpoint: &AutomationRootCheckpoint) -> Result<(), AutomationRunError> {
        atomic_json(
            &self.lease_path,
            &AutomationLease {
                lease_key: checkpoint.lease_key.clone(),
                run_id: checkpoint.run_id.clone(),
                owner_thread_id: checkpoint.owner_thread_id.clone(),
                repository: checkpoint.repository.clone(),
                tracker_project_slug: checkpoint.tracker_project_slug.clone(),
                lease_epoch: checkpoint.lease_epoch,
            },
        )?;
        Ok(())
    }

    fn write_versioned_profile_snapshot(
        &self,
        digest: &str,
        profile: &AutomationProfile,
    ) -> Result<(), AutomationRunError> {
        if !is_sha256(digest) {
            return Err(AutomationRunError::ProfileDigestMismatch);
        }
        let profiles = self.root.join("automation-profiles");
        fs::create_dir_all(&profiles)?;
        let versioned = profiles.join(format!("{digest}.json"));
        if versioned.exists() {
            let existing: AutomationProfile = read_json(&versioned)?;
            let existing_digest = serde_json::to_value(&existing)
                .map_err(std::io::Error::other)
                .and_then(|value| crate::canonical_sha256(&value).map_err(std::io::Error::other))?;
            if existing_digest != digest {
                return Err(AutomationRunError::ProfileDigestMismatch);
            }
        } else {
            create_json(&versioned, profile)?;
        }
        Ok(())
    }

    fn write_active_profile_snapshot(
        &self,
        digest: &str,
        profile: &AutomationProfile,
    ) -> Result<(), AutomationRunError> {
        self.write_versioned_profile_snapshot(digest, profile)?;
        atomic_json(&self.root.join("automation-profile.json"), profile)?;
        Ok(())
    }

    fn ensure_active_profile_snapshot(
        &self,
        checkpoint: &AutomationRootCheckpoint,
    ) -> Result<(), AutomationRunError> {
        let path = self
            .root
            .join("automation-profiles")
            .join(format!("{}.json", checkpoint.profile_digest));
        if path.exists() {
            return Ok(());
        }
        let profile: AutomationProfile = read_json(&self.root.join("automation-profile.json"))?;
        let digest = serde_json::to_value(&profile)
            .map_err(std::io::Error::other)
            .and_then(|value| crate::canonical_sha256(&value).map_err(std::io::Error::other))?;
        if digest != checkpoint.profile_digest {
            return Err(AutomationRunError::ProfileDigestMismatch);
        }
        self.write_versioned_profile_snapshot(&checkpoint.profile_digest, &profile)
    }

    fn verify_lease(
        &self,
        checkpoint: &AutomationRootCheckpoint,
    ) -> Result<(), AutomationRunError> {
        let lease: AutomationLease = read_json(&self.lease_path)?;
        if lease.run_id != checkpoint.run_id
            || lease.owner_thread_id != checkpoint.owner_thread_id
            || lease.lease_epoch != checkpoint.lease_epoch
        {
            return Err(AutomationRunError::LeaseConflict {
                lease_key: checkpoint.lease_key.clone(),
                owner_thread_id: lease.owner_thread_id,
            });
        }
        Ok(())
    }
}

fn automation_run_id(profile_digest: &str) -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    format!("automation-{millis}-{}", &profile_digest[..12])
}

fn automation_effect_name(kind: AutomationEffect) -> &'static str {
    match kind {
        AutomationEffect::TrackerComment => "tracker.comment",
        AutomationEffect::TrackerTransition => "tracker.transition",
        AutomationEffect::TrackerLinkPullRequest => "tracker.link_pull_request",
    }
}

pub fn normalize_pull_request_url(value: &str) -> Option<String> {
    let without_suffix = value.trim().split(['?', '#']).next()?.trim_end_matches('/');
    let path = without_suffix.strip_prefix("https://github.com/")?;
    let segments = path.split('/').collect::<Vec<_>>();
    let [owner, repository, pull, number] = segments.as_slice() else {
        return None;
    };
    if *pull != "pull" {
        return None;
    }
    let safe_segment = |segment: &str| {
        !segment.is_empty()
            && segment.len() <= 100
            && segment
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    };
    let number = number.parse::<u64>().ok().filter(|number| *number > 0)?;
    if !safe_segment(owner) || !safe_segment(repository) {
        return None;
    }
    Some(format!(
        "https://github.com/{owner}/{repository}/pull/{number}"
    ))
}

fn safe_segment(value: &str) -> String {
    let segment = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    let segment = segment.trim_matches('-');
    let bounded = segment.chars().take(48).collect::<String>();
    if bounded.is_empty() {
        "issue".into()
    } else {
        bounded
    }
}

fn claim_is_active(status: AutomationClaimStatus) -> bool {
    matches!(
        status,
        AutomationClaimStatus::Claimed
            | AutomationClaimStatus::Running
            | AutomationClaimStatus::Suspended
    )
}

fn active_claim_mut<'a>(
    checkpoint: &'a mut AutomationRootCheckpoint,
    claim_id: &str,
) -> Result<&'a mut AutomationIssueClaim, AutomationRunError> {
    let claim = checkpoint
        .claims
        .get_mut(claim_id)
        .ok_or_else(|| AutomationRunError::MissingClaim(claim_id.into()))?;
    if !claim_is_active(claim.status) {
        return Err(AutomationRunError::InactiveClaim(claim_id.into()));
    }
    Ok(claim)
}

fn retry_delay_ms(base_ms: u64, cap_ms: u64, attempt: u32) -> u64 {
    let exponent = attempt.saturating_sub(1).min(63);
    base_ms.saturating_mul(1_u64 << exponent).min(cap_ms)
}

pub fn automation_claim_liveness(
    claim: &AutomationIssueClaim,
    profile: &AutomationProfile,
    now_ms: u64,
) -> AutomationClaimLiveness {
    if matches!(
        claim.status,
        AutomationClaimStatus::Completed
            | AutomationClaimStatus::Cancelled
            | AutomationClaimStatus::Failed
    ) {
        return AutomationClaimLiveness::Terminal;
    }
    if claim
        .effects
        .iter()
        .any(|effect| effect.status == AutomationEffectStatus::WaitingGate)
    {
        return AutomationClaimLiveness::WaitingGate;
    }
    if claim.retry.is_some() {
        return AutomationClaimLiveness::WaitingRetry;
    }
    if claim.status == AutomationClaimStatus::Suspended {
        return AutomationClaimLiveness::Handoff;
    }
    if claim.last_progress_at_ms.is_some_and(|last_progress| {
        now_ms.saturating_sub(last_progress) >= profile.codex.stall_timeout_ms
    }) {
        return AutomationClaimLiveness::Stalled;
    }
    AutomationClaimLiveness::Active
}

fn is_active_state(profile: &AutomationProfile, state: &str) -> bool {
    profile
        .tracker
        .active_states
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(state))
}

fn is_terminal_state(profile: &AutomationProfile, state: &str) -> bool {
    profile
        .tracker
        .terminal_states
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(state))
}

fn has_nonterminal_blocker(profile: &AutomationProfile, issue: &AutomationIssue) -> bool {
    issue.blocked_by.iter().any(|blocker| {
        blocker
            .state
            .as_deref()
            .is_none_or(|state| !is_terminal_state(profile, state))
    })
}

fn state_limit(profile: &AutomationProfile, state: &str) -> Option<u32> {
    profile
        .agent
        .max_concurrent_agents_by_state
        .iter()
        .find(|(candidate, _)| candidate.eq_ignore_ascii_case(state))
        .map(|(_, limit)| *limit)
}

fn dispatch_key(issue: &AutomationIssue) -> (i64, &str, &str, &str) {
    (
        issue
            .priority
            .filter(|priority| *priority > 0)
            .unwrap_or(i64::MAX),
        issue.created_at.as_deref().unwrap_or("~"),
        issue.identifier.as_str(),
        issue.id.as_str(),
    )
}

fn issue_observation_key(issue: &AutomationIssue) -> (&str, &str, &str) {
    (
        issue.updated_at.as_deref().unwrap_or(""),
        issue.identifier.as_str(),
        issue.title.as_str(),
    )
}

fn queue_item(
    issue: &AutomationIssue,
    status: AutomationQueueStatus,
    reason: &str,
) -> AutomationQueueItem {
    AutomationQueueItem {
        issue_id: issue.id.clone(),
        issue_identifier: issue.identifier.clone(),
        issue_title: issue.title.clone(),
        state: issue.state.clone(),
        priority: issue.priority,
        status,
        reason: reason.into(),
    }
}

pub fn automation_queue_counts(checkpoint: &AutomationRootCheckpoint) -> AutomationQueueCounts {
    let mut counts = AutomationQueueCounts::default();
    for item in checkpoint.queue.values() {
        match item.status {
            AutomationQueueStatus::Queued => counts.queued += 1,
            AutomationQueueStatus::Blocked => counts.blocked += 1,
            AutomationQueueStatus::Terminal => counts.terminal += 1,
        }
    }
    for claim in checkpoint.claims.values() {
        let waiting_gate = claim
            .effects
            .iter()
            .any(|effect| effect.status == AutomationEffectStatus::WaitingGate);
        match claim.status {
            AutomationClaimStatus::Claimed | AutomationClaimStatus::Running if waiting_gate => {
                counts.waiting_gate += 1;
            }
            AutomationClaimStatus::Claimed | AutomationClaimStatus::Running => {
                counts.running += 1;
            }
            AutomationClaimStatus::Suspended if waiting_gate => counts.waiting_gate += 1,
            AutomationClaimStatus::Suspended => counts.handoff += 1,
            AutomationClaimStatus::Completed
            | AutomationClaimStatus::Cancelled
            | AutomationClaimStatus::Failed => counts.terminal += 1,
        }
    }
    counts
}

pub fn automation_queue_page(
    checkpoint: &AutomationRootCheckpoint,
    category: AutomationQueueCategory,
    offset: u32,
    limit: u32,
) -> AutomationQueuePage {
    let mut items = checkpoint
        .queue
        .values()
        .filter_map(|item| {
            let item_category = match item.status {
                AutomationQueueStatus::Queued => AutomationQueueCategory::Queued,
                AutomationQueueStatus::Blocked => AutomationQueueCategory::Blocked,
                AutomationQueueStatus::Terminal => AutomationQueueCategory::Terminal,
            };
            (item_category == category).then(|| AutomationQueueProjectionItem {
                issue_id: item.issue_id.clone(),
                issue_identifier: item.issue_identifier.clone(),
                issue_title: item.issue_title.clone(),
                state: item.state.clone(),
                priority: item.priority,
                claim_id: None,
                category: item_category,
                next_action: item.reason.clone(),
            })
        })
        .chain(checkpoint.claims.values().filter_map(|claim| {
            let waiting_gate = claim
                .effects
                .iter()
                .any(|effect| effect.status == AutomationEffectStatus::WaitingGate);
            let claim_category = match claim.status {
                AutomationClaimStatus::Claimed | AutomationClaimStatus::Running if waiting_gate => {
                    AutomationQueueCategory::WaitingGate
                }
                AutomationClaimStatus::Claimed | AutomationClaimStatus::Running => {
                    AutomationQueueCategory::Running
                }
                AutomationClaimStatus::Suspended if waiting_gate => {
                    AutomationQueueCategory::WaitingGate
                }
                AutomationClaimStatus::Suspended => AutomationQueueCategory::Handoff,
                AutomationClaimStatus::Completed
                | AutomationClaimStatus::Cancelled
                | AutomationClaimStatus::Failed => AutomationQueueCategory::Terminal,
            };
            (claim_category == category).then(|| AutomationQueueProjectionItem {
                issue_id: claim.issue_id.clone(),
                issue_identifier: claim.issue_identifier.clone(),
                issue_title: claim.issue_title.clone(),
                state: claim.tracker_state.clone(),
                priority: claim.priority,
                claim_id: Some(claim.claim_id.clone()),
                category: claim_category,
                next_action: claim.next_action.clone(),
            })
        }))
        .collect::<Vec<_>>();
    items.sort_by(|left, right| {
        (
            left.priority.unwrap_or(i64::MAX),
            left.issue_identifier.as_str(),
            left.issue_id.as_str(),
        )
            .cmp(&(
                right.priority.unwrap_or(i64::MAX),
                right.issue_identifier.as_str(),
                right.issue_id.as_str(),
            ))
    });
    let total = items.len() as u32;
    let start = usize::min(offset as usize, items.len());
    let bounded_limit = limit.clamp(1, 50) as usize;
    let end = usize::min(start + bounded_limit, items.len());
    let page = items[start..end].to_vec();
    AutomationQueuePage {
        category,
        total,
        items: page,
        next_offset: (end < items.len()).then_some(end as u32),
    }
}

fn bounded_preview(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.into();
    }
    let ellipsis = "…";
    if max_bytes < ellipsis.len() {
        return String::new();
    }
    let mut end = max_bytes - ellipsis.len();
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{}", &value[..end], ellipsis)
}

fn canonical_or_lexical(path: &Path) -> Result<PathBuf, std::io::Error> {
    if path.exists() {
        return path.canonicalize();
    }
    let base = if path.is_absolute() {
        PathBuf::new()
    } else {
        std::env::current_dir()?
    };
    let mut out = base;
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
    Ok(out)
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, std::io::Error> {
    serde_json::from_slice(&fs::read(path)?).map_err(std::io::Error::other)
}

fn create_json<T: Serialize>(path: &Path, value: &T) -> Result<(), std::io::Error> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    let mut data = serde_json::to_vec_pretty(value).map_err(std::io::Error::other)?;
    data.push(b'\n');
    file.write_all(&data)?;
    file.sync_all()
}

fn atomic_json<T: Serialize>(path: &Path, value: &T) -> Result<(), std::io::Error> {
    let mut data = serde_json::to_vec_pretty(value).map_err(std::io::Error::other)?;
    data.push(b'\n');
    let nonce = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temp = path.with_extension(format!("tmp-{}-{nanos}-{nonce}", std::process::id()));
    fs::write(&temp, data)?;
    fs::rename(temp, path)
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AutomationAgentProfile;
    use crate::AutomationCodexPolicy;
    use crate::AutomationEffect;
    use crate::AutomationHooksProfile;
    use crate::AutomationOrchestraProfile;
    use crate::AutomationPollingProfile;
    use crate::AutomationSecretKind;
    use crate::AutomationSecretReference;
    use crate::AutomationTrackerProfile;
    use crate::AutomationWorkspaceProfile;
    use serde_json::Value;
    use serde_json::json;
    use tempfile::tempdir;

    fn profile(workspace: &Path) -> AutomationProfile {
        AutomationProfile {
            tracker: AutomationTrackerProfile {
                kind: "linear".into(),
                endpoint: "https://api.linear.app/graphql".into(),
                project_slug: "orchestra".into(),
                required_labels: vec!["automation".into()],
                active_states: vec!["Todo".into()],
                terminal_states: vec!["Done".into()],
                credential: AutomationSecretReference {
                    kind: AutomationSecretKind::Environment,
                    reference: "LINEAR_API_KEY".into(),
                    digest: "digest".into(),
                },
            },
            polling: AutomationPollingProfile {
                interval_ms: 30_000,
            },
            workspace: AutomationWorkspaceProfile {
                root: workspace.to_string_lossy().into_owned(),
            },
            hooks: AutomationHooksProfile {
                after_create: None,
                before_run: None,
                after_run: None,
                before_remove: None,
                timeout_ms: 60_000,
            },
            agent: AutomationAgentProfile {
                max_concurrent_agents: 1,
                max_turns: 20,
                max_retry_backoff_ms: 300_000,
                max_concurrent_agents_by_state: BTreeMap::new(),
            },
            codex: AutomationCodexPolicy {
                approval_policy: json!("on-request"),
                thread_sandbox: "workspace-write".into(),
                turn_sandbox_policy: Value::Null,
                turn_timeout_ms: 3_600_000,
                read_timeout_ms: 5_000,
                stall_timeout_ms: 300_000,
            },
            orchestra: AutomationOrchestraProfile {
                workflow_path: "issue.workflow.ts".into(),
                workflow_sha256: "workflow".into(),
                workflow_name: "issue".into(),
                effects: vec![AutomationEffect::TrackerComment],
            },
            prompt_template: "Implement {{ issue.identifier }}".into(),
        }
    }

    fn issue() -> AutomationIssue {
        AutomationIssue {
            id: "issue-33".into(),
            identifier: "ORC-33".into(),
            title: "Run fixture".into(),
            description: None,
            priority: None,
            state: "Todo".into(),
            branch_name: None,
            url: None,
            labels: vec!["automation".into()],
            blocked_by: Vec::new(),
            created_at: None,
            updated_at: None,
        }
    }

    fn queued_issue(
        id: &str,
        identifier: &str,
        state: &str,
        priority: Option<i64>,
    ) -> AutomationIssue {
        AutomationIssue {
            id: id.into(),
            identifier: identifier.into(),
            title: format!("Coordinate {identifier}"),
            description: None,
            priority,
            state: state.into(),
            branch_name: None,
            url: None,
            labels: vec!["automation".into()],
            blocked_by: Vec::new(),
            created_at: Some(format!("2026-07-{:02}T00:00:00Z", priority.unwrap_or(9))),
            updated_at: Some("2026-07-16T00:00:00Z".into()),
        }
    }

    #[test]
    fn root_lease_and_claim_are_stable_and_task_scoped() {
        let repository = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let profile = profile(workspace.path());
        let digest = crate::canonical_sha256(&serde_json::to_value(&profile).unwrap()).unwrap();
        let request = AutomationRunStart {
            repository: repository.path(),
            owner_thread_id: "task-1",
            source_revision: "abc123",
            profile: &profile,
            profile_digest: &digest,
        };
        let (store, mut checkpoint) = AutomationRunStore::start(request).unwrap();
        let claim_id = store.claim_fixture(&mut checkpoint, &issue(), 1).unwrap();
        assert_eq!(checkpoint.owner_thread_id, "task-1");
        assert_eq!(checkpoint.claims[&claim_id].source_revision, "abc123");
        assert!(
            checkpoint.claims[&claim_id]
                .worktree
                .starts_with(workspace.path().canonicalize().unwrap())
        );
        assert!(
            checkpoint.claims[&claim_id]
                .worktree
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("orc-33-")
        );

        let (reopened, reopened_checkpoint) = AutomationRunStore::start(AutomationRunStart {
            repository: repository.path(),
            owner_thread_id: "task-1",
            source_revision: "different-is-ignored-for-resident-root",
            profile: &profile,
            profile_digest: &digest,
        })
        .unwrap();
        assert_eq!(reopened_checkpoint.run_id, checkpoint.run_id);
        assert!(matches!(
            reopened.claim_fixture(&mut reopened_checkpoint.clone(), &issue(), 1),
            Err(AutomationRunError::DuplicateIssue(_))
        ));
    }

    #[test]
    fn another_task_cannot_take_the_repository_project_lease() {
        let repository = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let profile = profile(workspace.path());
        let digest = crate::canonical_sha256(&serde_json::to_value(&profile).unwrap()).unwrap();
        AutomationRunStore::start(AutomationRunStart {
            repository: repository.path(),
            owner_thread_id: "task-1",
            source_revision: "abc123",
            profile: &profile,
            profile_digest: &digest,
        })
        .unwrap();
        let result = AutomationRunStore::start(AutomationRunStart {
            repository: repository.path(),
            owner_thread_id: "task-2",
            source_revision: "abc123",
            profile: &profile,
            profile_digest: &digest,
        });
        assert!(matches!(
            result,
            Err(AutomationRunError::LeaseConflict { .. })
        ));
    }

    #[test]
    fn claim_worktree_names_are_contained_bounded_and_collision_safe() {
        let repository = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let mut profile = profile(workspace.path());
        profile.agent.max_concurrent_agents = 2;
        let digest = crate::canonical_sha256(&serde_json::to_value(&profile).unwrap()).unwrap();
        let (store, mut root) = AutomationRunStore::start(AutomationRunStart {
            repository: repository.path(),
            owner_thread_id: "task-worktree-names",
            source_revision: "abc123",
            profile: &profile,
            profile_digest: &digest,
        })
        .unwrap();
        let mut first = issue();
        first.id = "issue-collision-one".into();
        first.identifier =
            "../../SAME !!! very-long-identifier-that-must-not-escape-or-grow-forever".into();
        let mut second = first.clone();
        second.id = "issue-collision-two".into();
        second.identifier =
            "SAME---very-long-identifier-that-must-not-escape-or-grow-forever".into();

        let first_claim = store.claim_fixture(&mut root, &first, 1).unwrap();
        let second_claim = store.claim_fixture(&mut root, &second, 1).unwrap();
        let first_path = &root.claims[&first_claim].worktree;
        let second_path = &root.claims[&second_claim].worktree;
        let canonical_root = workspace.path().canonicalize().unwrap();
        assert!(first_path.starts_with(&canonical_root));
        assert!(second_path.starts_with(&canonical_root));
        assert_ne!(first_path, second_path);
        assert!(first_path.file_name().unwrap().to_string_lossy().len() <= 65);
        assert!(second_path.file_name().unwrap().to_string_lossy().len() <= 65);
    }

    #[test]
    fn hook_receipts_are_bounded_and_cleanup_retries_remain_inspectable() {
        let repository = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let profile = profile(workspace.path());
        let digest = crate::canonical_sha256(&serde_json::to_value(&profile).unwrap()).unwrap();
        let (store, mut root) = AutomationRunStore::start(AutomationRunStart {
            repository: repository.path(),
            owner_thread_id: "task-hooks",
            source_revision: "abc123",
            profile: &profile,
            profile_digest: &digest,
        })
        .unwrap();
        let claim_id = store.claim_fixture(&mut root, &issue(), 1).unwrap();
        let output = crate::CommandOutcome {
            exit_code: 7,
            stdout: "o".repeat(2_000),
            stderr: "e".repeat(2_000),
        };
        let receipt = store
            .record_hook_receipt(
                &mut root,
                &claim_id,
                AutomationHookKind::BeforeRun,
                Some("false"),
                Ok(&output),
            )
            .unwrap();
        assert_eq!(receipt.status, AutomationHookStatus::Failed);
        assert_eq!(receipt.exit_code, Some(7));
        assert!(receipt.stdout_preview.len() <= 512);
        assert!(receipt.stderr_preview.len() <= 512);
        assert_eq!(receipt.command_sha256, Some(sha256(b"false")));
        for _ in 0..33 {
            store
                .record_hook_receipt(
                    &mut root,
                    &claim_id,
                    AutomationHookKind::BeforeRun,
                    Some("false"),
                    Ok(&output),
                )
                .unwrap();
        }
        assert_eq!(root.claims[&claim_id].hook_receipts.len(), 32);
        assert_eq!(
            root.claims[&claim_id]
                .hook_receipts
                .last()
                .unwrap()
                .invocation,
            34
        );

        root.claims.get_mut(&claim_id).unwrap().cleanup.status = AutomationCleanupStatus::Eligible;
        store.save(&mut root).unwrap();
        store
            .record_cleanup_attempt(&mut root, &claim_id, Err(&"x".repeat(2_000)))
            .unwrap();
        assert_eq!(
            root.claims[&claim_id].cleanup.status,
            AutomationCleanupStatus::RetryPending
        );
        assert!(
            root.claims[&claim_id]
                .cleanup
                .last_failure
                .as_ref()
                .unwrap()
                .len()
                <= 512
        );
        store
            .record_cleanup_attempt(&mut root, &claim_id, Ok(()))
            .unwrap();
        assert_eq!(root.claims[&claim_id].cleanup.attempts, 2);
        assert_eq!(
            root.claims[&claim_id].cleanup.status,
            AutomationCleanupStatus::Removed
        );
    }

    #[test]
    fn deterministic_queue_enforces_global_and_per_state_capacity_without_head_of_line_blocking() {
        let repository = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let mut profile = profile(workspace.path());
        profile.tracker.active_states = vec!["Todo".into(), "In Progress".into()];
        profile.agent.max_concurrent_agents = 2;
        profile
            .agent
            .max_concurrent_agents_by_state
            .insert("Todo".into(), 1);
        profile
            .agent
            .max_concurrent_agents_by_state
            .insert("In Progress".into(), 1);
        let digest = crate::canonical_sha256(&serde_json::to_value(&profile).unwrap()).unwrap();
        let (store, mut checkpoint) = AutomationRunStore::start(AutomationRunStart {
            repository: repository.path(),
            owner_thread_id: "task-queue",
            source_revision: "abc123",
            profile: &profile,
            profile_digest: &digest,
        })
        .unwrap();

        let urgent = queued_issue("issue-1", "ORC-1", "Todo", Some(1));
        let later_same_state = queued_issue("issue-2", "ORC-2", "Todo", Some(2));
        let other_state = queued_issue("issue-3", "ORC-3", "In Progress", Some(3));
        let mut blocked = queued_issue("issue-4", "ORC-4", "Todo", Some(0));
        blocked.blocked_by.push(crate::AutomationIssueBlocker {
            id: Some("blocker-1".into()),
            identifier: Some("ORC-99".into()),
            state: Some("Todo".into()),
        });
        let terminal = queued_issue("issue-5", "ORC-5", "Done", Some(1));
        let mut wrong_label = queued_issue("issue-6", "ORC-6", "Todo", Some(1));
        wrong_label.labels = vec!["not-automation".into()];

        let result = store
            .coordinate_fixture(
                &mut checkpoint,
                &profile,
                &[
                    later_same_state.clone(),
                    terminal,
                    other_state,
                    urgent,
                    blocked,
                    wrong_label,
                ],
                1,
            )
            .unwrap();
        let claimed = result
            .dispatched_claim_ids
            .iter()
            .map(|claim_id| checkpoint.claims[claim_id].issue_identifier.as_str())
            .collect::<Vec<_>>();
        assert_eq!(claimed, ["ORC-1", "ORC-3"]);
        assert_eq!(
            result.counts,
            AutomationQueueCounts {
                queued: 1,
                running: 2,
                blocked: 1,
                waiting_gate: 0,
                handoff: 0,
                terminal: 1,
            }
        );
        assert!(!checkpoint.queue.contains_key("issue-6"));
        assert_eq!(
            store
                .queue_page(&checkpoint, AutomationQueueCategory::Queued, 0, 1)
                .items[0]
                .issue_identifier,
            "ORC-2"
        );

        let mut externally_done = queued_issue("issue-1", "ORC-1", "Done", Some(1));
        externally_done.updated_at = Some("2026-07-17T00:00:00Z".into());
        let reconciled = store
            .coordinate_fixture(
                &mut checkpoint,
                &profile,
                &[externally_done, later_same_state],
                1,
            )
            .unwrap();
        assert!(reconciled.dispatched_claim_ids.is_empty());
        let urgent_claim = checkpoint
            .claims
            .values()
            .find(|claim| claim.issue_id == "issue-1")
            .unwrap();
        assert_eq!(urgent_claim.tracker_state, "Done");
        assert_eq!(
            urgent_claim.next_action,
            "reconcile externally terminal tracker state before dispatch"
        );
        assert_eq!(
            checkpoint
                .claims
                .values()
                .filter(|claim| claim.issue_id == "issue-1")
                .count(),
            1
        );
    }

    #[test]
    fn queue_pages_are_bounded_and_derive_waiting_gate_handoff_and_terminal_claims() {
        let repository = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let profile = profile(workspace.path());
        let digest = crate::canonical_sha256(&serde_json::to_value(&profile).unwrap()).unwrap();
        let (store, mut checkpoint) = AutomationRunStore::start(AutomationRunStart {
            repository: repository.path(),
            owner_thread_id: "task-pages",
            source_revision: "abc123",
            profile: &profile,
            profile_digest: &digest,
        })
        .unwrap();
        for index in 0..55 {
            let issue = queued_issue(
                &format!("issue-{index}"),
                &format!("ORC-{index:02}"),
                "Todo",
                None,
            );
            checkpoint.queue.insert(
                issue.id.clone(),
                queue_item(
                    &issue,
                    AutomationQueueStatus::Queued,
                    "waiting for capacity",
                ),
            );
        }
        let first = store.queue_page(&checkpoint, AutomationQueueCategory::Queued, 0, 100);
        assert_eq!(first.total, 55);
        assert_eq!(first.items.len(), 50);
        assert_eq!(first.next_offset, Some(50));
        let second = store.queue_page(
            &checkpoint,
            AutomationQueueCategory::Queued,
            first.next_offset.unwrap(),
            50,
        );
        assert_eq!(second.items.len(), 5);
        assert_eq!(second.next_offset, None);

        let waiting_id = store.claim_fixture(&mut checkpoint, &issue(), 1).unwrap();
        let waiting = checkpoint.claims.get_mut(&waiting_id).unwrap();
        waiting.status = AutomationClaimStatus::Suspended;
        waiting.effects.push(AutomationEffectReceipt {
            effect_id: "effect-wait".into(),
            idempotency_key: "idem-wait".into(),
            kind: AutomationEffect::TrackerComment,
            claim_id: waiting_id.clone(),
            tracker_project_slug: "orchestra".into(),
            issue_id: waiting.issue_id.clone(),
            request_sha256: "request".into(),
            body_preview: "preview".into(),
            gate_policy: AutomationGatePolicy::AskHuman,
            status: AutomationEffectStatus::WaitingGate,
            provider_receipt: None,
            failure: None,
        });
        assert_eq!(automation_queue_counts(&checkpoint).waiting_gate, 1);
        assert_eq!(
            store
                .queue_page(&checkpoint, AutomationQueueCategory::WaitingGate, 0, 8)
                .items[0]
                .claim_id
                .as_deref(),
            Some(waiting_id.as_str())
        );
    }

    #[test]
    fn normalized_linear_pages_feed_the_same_deterministic_coordinator() {
        let repository = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let mut profile = profile(workspace.path());
        profile.agent.max_concurrent_agents = 1;
        let digest = crate::canonical_sha256(&serde_json::to_value(&profile).unwrap()).unwrap();
        let (store, mut checkpoint) = AutomationRunStore::start(AutomationRunStart {
            repository: repository.path(),
            owner_thread_id: "task-pages-to-queue",
            source_revision: "abc123",
            profile: &profile,
            profile_digest: &digest,
        })
        .unwrap();
        let raw_issue = |id: &str, identifier: &str, priority: i64| {
            json!({
                "id": id,
                "identifier": identifier,
                "title": format!("Issue {identifier}"),
                "priority": priority,
                "state": {"name": "Todo"},
                "labels": {"nodes": [{"name": "automation"}]},
                "createdAt": "2026-07-01T00:00:00Z",
                "updatedAt": "2026-07-16T00:00:00Z"
            })
        };
        let first = crate::normalize_linear_issue_page(&json!({"data": {"project": {"issues": {
            "nodes": [raw_issue("issue-low", "ORC-LOW", 4)],
            "pageInfo": {"hasNextPage": true, "endCursor": "cursor-1"}
        }}}}))
        .unwrap();
        let second = crate::normalize_linear_issue_page(&json!({"data": {"project": {"issues": {
            "nodes": [raw_issue("issue-urgent", "ORC-URGENT", 1)],
            "pageInfo": {"hasNextPage": false, "endCursor": null}
        }}}}))
        .unwrap();
        assert!(first.has_next_page);
        assert_eq!(first.end_cursor.as_deref(), Some("cursor-1"));
        let issues = first
            .issues
            .into_iter()
            .chain(second.issues)
            .collect::<Vec<_>>();
        let result = store
            .coordinate_fixture(&mut checkpoint, &profile, &issues, 1)
            .unwrap();
        assert_eq!(result.dispatched_claim_ids.len(), 1);
        assert_eq!(
            checkpoint.claims[&result.dispatched_claim_ids[0]].issue_identifier,
            "ORC-URGENT"
        );
        assert_eq!(result.counts.queued, 1);
    }

    #[test]
    fn tracker_comment_gate_and_receipt_are_claim_scoped_and_idempotent() {
        let repository = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let profile = profile(workspace.path());
        let digest = crate::canonical_sha256(&serde_json::to_value(&profile).unwrap()).unwrap();
        let (store, mut checkpoint) = AutomationRunStore::start(AutomationRunStart {
            repository: repository.path(),
            owner_thread_id: "task-1",
            source_revision: "abc123",
            profile: &profile,
            profile_digest: &digest,
        })
        .unwrap();
        let claim_id = store.claim_fixture(&mut checkpoint, &issue(), 1).unwrap();
        let executions = std::cell::Cell::new(0);
        let committed = store
            .resolve_tracker_comment(
                &mut checkpoint,
                &claim_id,
                &profile,
                "Implemented and verified.",
                AutomationGatePolicy::AutoAccept,
                |request| {
                    executions.set(executions.get() + 1);
                    assert_eq!(request.claim_id, claim_id);
                    assert_eq!(request.issue_id, "issue-33");
                    AutomationEffectExecution::Committed {
                        provider_receipt: "fixture-comment-1".into(),
                    }
                },
            )
            .unwrap();
        assert_eq!(committed.status, AutomationEffectStatus::Committed);
        assert_eq!(executions.get(), 1);

        let duplicate = store
            .resolve_tracker_comment(
                &mut checkpoint,
                &claim_id,
                &profile,
                "Implemented and verified.",
                AutomationGatePolicy::AutoAccept,
                |_| panic!("a committed idempotency key must not execute twice"),
            )
            .unwrap();
        assert_eq!(duplicate, committed);

        checkpoint.claims.get_mut(&claim_id).unwrap().status = AutomationClaimStatus::Completed;
        let completed_replay = store
            .resolve_tracker_comment(
                &mut checkpoint,
                &claim_id,
                &profile,
                "Implemented and verified.",
                AutomationGatePolicy::AutoAccept,
                |_| panic!("a completed claim must replay its durable receipt"),
            )
            .unwrap();
        assert_eq!(completed_replay, committed);
        assert!(matches!(
            store.resolve_tracker_comment(
                &mut checkpoint,
                &claim_id,
                &profile,
                "A new mutation after completion",
                AutomationGatePolicy::AutoAccept,
                |_| unreachable!(),
            ),
            Err(AutomationRunError::InactiveClaim(_))
        ));
        checkpoint.claims.get_mut(&claim_id).unwrap().status = AutomationClaimStatus::Claimed;
        assert!(matches!(
            store.resolve_tracker_comment(
                &mut checkpoint,
                "claim-from-another-issue",
                &profile,
                "Cross-claim mutation",
                AutomationGatePolicy::AutoAccept,
                |_| unreachable!(),
            ),
            Err(AutomationRunError::MissingClaim(_))
        ));

        let rejected = store
            .resolve_tracker_comment(
                &mut checkpoint,
                &claim_id,
                &profile,
                "Do not publish this variant.",
                AutomationGatePolicy::AutoReject,
                |_| panic!("a rejected gate must precede mutation"),
            )
            .unwrap();
        assert_eq!(rejected.status, AutomationEffectStatus::Rejected);
        let paused = store
            .resolve_tracker_comment(
                &mut checkpoint,
                &claim_id,
                &profile,
                "Ask before publishing this variant.",
                AutomationGatePolicy::AskHuman,
                |_| panic!("a waiting gate must precede mutation"),
            )
            .unwrap();
        assert_eq!(paused.status, AutomationEffectStatus::WaitingGate);
    }

    #[test]
    fn transition_and_pull_request_effects_are_scoped_normalized_and_idempotent() {
        let repository = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let mut profile = profile(workspace.path());
        profile.orchestra.effects = vec![
            AutomationEffect::TrackerTransition,
            AutomationEffect::TrackerLinkPullRequest,
        ];
        let digest = crate::canonical_sha256(&serde_json::to_value(&profile).unwrap()).unwrap();
        let (store, mut root) = AutomationRunStore::start(AutomationRunStart {
            repository: repository.path(),
            owner_thread_id: "task-effects",
            source_revision: "abc123",
            profile: &profile,
            profile_digest: &digest,
        })
        .unwrap();
        let claim_id = store.claim_fixture(&mut root, &issue(), 1).unwrap();

        let already_applied = store
            .resolve_tracker_transition(
                &mut root,
                &claim_id,
                &profile,
                "Todo",
                "Todo",
                AutomationGatePolicy::AutoAccept,
                |_| panic!("an already-applied transition must not execute"),
            )
            .unwrap();
        assert_eq!(already_applied.status, AutomationEffectStatus::Committed);
        assert_eq!(
            already_applied.provider_receipt.as_deref(),
            Some("already-applied:Todo")
        );

        assert!(matches!(
            store.resolve_tracker_transition(
                &mut root,
                &claim_id,
                &profile,
                "Todo",
                "Unknown",
                AutomationGatePolicy::AutoAccept,
                |_| unreachable!(),
            ),
            Err(AutomationRunError::InvalidTransition)
        ));
        let transitioned = store
            .resolve_tracker_transition(
                &mut root,
                &claim_id,
                &profile,
                "Todo",
                "done",
                AutomationGatePolicy::AutoAccept,
                |request| {
                    assert_eq!(request.issue_id, "issue-33");
                    assert_eq!(request.claim_id, claim_id);
                    assert_eq!(request.tracker_project_slug, "orchestra");
                    assert_eq!(request.expected_state, "Todo");
                    assert_eq!(request.target_state, "Done");
                    AutomationEffectExecution::Committed {
                        provider_receipt: "linear-transition-1".into(),
                    }
                },
            )
            .unwrap();
        assert_eq!(transitioned.status, AutomationEffectStatus::Committed);
        assert_eq!(transitioned.kind, AutomationEffect::TrackerTransition);
        assert_eq!(root.claims[&claim_id].tracker_state, "Done");
        let duplicate = store
            .resolve_tracker_transition(
                &mut root,
                &claim_id,
                &profile,
                "Todo",
                "Done",
                AutomationGatePolicy::AutoAccept,
                |_| panic!("idempotent transition must not execute twice"),
            )
            .unwrap();
        assert_eq!(duplicate, transitioned);
        assert!(matches!(
            store.resolve_tracker_transition(
                &mut root,
                &claim_id,
                &profile,
                "Done",
                "Todo",
                AutomationGatePolicy::AutoAccept,
                |_| unreachable!(),
            ),
            Err(AutomationRunError::InactiveClaim(_))
        ));

        let linked = store
            .resolve_tracker_pull_request_link(
                &mut root,
                &claim_id,
                &profile,
                " https://github.com/edgefloor/codex-orchestra/pull/00043/?utm=x#discussion ",
                AutomationGatePolicy::AutoAccept,
                |request| {
                    assert_eq!(request.issue_id, "issue-33");
                    assert_eq!(
                        request.pull_request_url,
                        "https://github.com/edgefloor/codex-orchestra/pull/43"
                    );
                    AutomationEffectExecution::Committed {
                        provider_receipt: "linear-link-1".into(),
                    }
                },
            )
            .unwrap();
        assert_eq!(linked.status, AutomationEffectStatus::Committed);
        assert_eq!(linked.kind, AutomationEffect::TrackerLinkPullRequest);
        assert!(matches!(
            store.resolve_tracker_pull_request_link(
                &mut root,
                &claim_id,
                &profile,
                "https://example.com/pull/43",
                AutomationGatePolicy::AutoAccept,
                |_| unreachable!(),
            ),
            Err(AutomationRunError::InvalidPullRequestLink)
        ));
    }

    #[test]
    fn missing_authority_and_interrupted_or_ambiguous_effects_fail_closed() {
        let repository = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let mut profile = profile(workspace.path());
        let digest = crate::canonical_sha256(&serde_json::to_value(&profile).unwrap()).unwrap();
        let (store, mut checkpoint) = AutomationRunStore::start(AutomationRunStart {
            repository: repository.path(),
            owner_thread_id: "task-1",
            source_revision: "abc123",
            profile: &profile,
            profile_digest: &digest,
        })
        .unwrap();
        let claim_id = store.claim_fixture(&mut checkpoint, &issue(), 1).unwrap();
        profile.orchestra.effects.clear();
        assert!(matches!(
            store.resolve_tracker_comment(
                &mut checkpoint,
                &claim_id,
                &profile,
                "Unauthorized",
                AutomationGatePolicy::AutoAccept,
                |_| unreachable!(),
            ),
            Err(AutomationRunError::MissingEffectAuthority)
        ));
        profile
            .orchestra
            .effects
            .push(AutomationEffect::TrackerComment);
        let ambiguous = store
            .resolve_tracker_comment(
                &mut checkpoint,
                &claim_id,
                &profile,
                "Maybe published",
                AutomationGatePolicy::AutoAccept,
                |_| AutomationEffectExecution::Ambiguous {
                    message: "provider timed out after accepting bytes".into(),
                },
            )
            .unwrap();
        assert_eq!(ambiguous.status, AutomationEffectStatus::Ambiguous);

        let crash_body = "crashed after mutation";
        let mut interrupted = store
            .resolve_tracker_comment(
                &mut checkpoint,
                &claim_id,
                &profile,
                crash_body,
                AutomationGatePolicy::AskHuman,
                |_| unreachable!(),
            )
            .unwrap();
        interrupted.status = AutomationEffectStatus::Executing;
        let effect = checkpoint
            .claims
            .get_mut(&claim_id)
            .unwrap()
            .effects
            .iter_mut()
            .find(|receipt| receipt.idempotency_key == interrupted.idempotency_key)
            .unwrap();
        *effect = interrupted;
        store.save(&mut checkpoint).unwrap();
        let recovered = store
            .resolve_tracker_comment(
                &mut checkpoint,
                &claim_id,
                &profile,
                crash_body,
                AutomationGatePolicy::AutoAccept,
                |_| panic!("interrupted execution must reconcile before retry"),
            )
            .unwrap();
        assert_eq!(recovered.status, AutomationEffectStatus::Ambiguous);
    }

    #[test]
    fn pause_advances_the_epoch_before_descendants_and_fences_stale_provider_results() {
        let repository = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let profile = profile(workspace.path());
        let digest = crate::canonical_sha256(&serde_json::to_value(&profile).unwrap()).unwrap();
        let (store, mut root) = AutomationRunStore::start(AutomationRunStart {
            repository: repository.path(),
            owner_thread_id: "task-pause",
            source_revision: "abc123",
            profile: &profile,
            profile_digest: &digest,
        })
        .unwrap();
        let claim_id = store.claim_fixture(&mut root, &issue(), 1).unwrap();
        let mut stale = root.clone();
        let initial_epoch = root.lease_epoch;
        store.pause(&mut root, "host shutdown").unwrap();
        assert_eq!(root.lease_epoch, initial_epoch + 1);
        assert_eq!(root.status, AutomationRootStatus::Suspended);
        assert_eq!(
            root.reconciliation,
            AutomationReconciliationStatus::Required
        );
        assert_eq!(
            root.claims[&claim_id].status,
            AutomationClaimStatus::Suspended
        );
        assert!(matches!(
            store.update_claim(&mut stale, &claim_id, |_| {}),
            Err(AutomationRunError::StaleLease { .. })
        ));

        store.begin_reconciliation(&mut root).unwrap();
        store
            .reconcile(
                &mut root,
                &profile,
                &[issue()],
                &[AutomationClaimReconciliation {
                    claim_id: claim_id.clone(),
                    issue_task_active: false,
                    descendants_cancelled: false,
                    tracker_terminal: false,
                    workflow_status: None,
                }],
            )
            .unwrap();
        store
            .update_claim(&mut root, &claim_id, |claim| {
                claim.status = AutomationClaimStatus::Running;
            })
            .unwrap();
        let provider_result = store.resolve_tracker_comment(
            &mut root,
            &claim_id,
            &profile,
            "Provider accepted this before shutdown.",
            AutomationGatePolicy::AutoAccept,
            |_| {
                let mut authoritative = store.load().unwrap();
                store.pause(&mut authoritative, "host shutdown").unwrap();
                AutomationEffectExecution::Committed {
                    provider_receipt: "late-provider-receipt".into(),
                }
            },
        );
        assert!(matches!(
            provider_result,
            Err(AutomationRunError::StaleLease { .. })
        ));
        let durable = store.load().unwrap();
        assert_eq!(durable.status, AutomationRootStatus::Suspended);
        let effect = &durable.claims[&claim_id].effects[0];
        assert_eq!(effect.status, AutomationEffectStatus::Ambiguous);
        assert_eq!(effect.provider_receipt, None);
    }

    #[test]
    fn issue_cancellation_fences_late_effects_and_waits_for_descendants_and_reconciliation() {
        let repository = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let profile = profile(workspace.path());
        let digest = crate::canonical_sha256(&serde_json::to_value(&profile).unwrap()).unwrap();
        let (store, mut root) = AutomationRunStore::start(AutomationRunStart {
            repository: repository.path(),
            owner_thread_id: "task-cancel-issue",
            source_revision: "abc123",
            profile: &profile,
            profile_digest: &digest,
        })
        .unwrap();
        let claim_id = store.claim_fixture(&mut root, &issue(), 1).unwrap();
        let (_, request) = store
            .prepare_tracker_comment(
                &mut root,
                &claim_id,
                &profile,
                "provider mutation in flight",
                AutomationGatePolicy::AutoAccept,
            )
            .unwrap();
        let request = request.unwrap();
        let mut stale = root.clone();

        store
            .begin_claim_cancellation(&mut root, &claim_id)
            .unwrap();
        assert_eq!(
            root.claims[&claim_id].status,
            AutomationClaimStatus::Suspended
        );
        assert_eq!(
            root.claims[&claim_id].effects[0].status,
            AutomationEffectStatus::Ambiguous
        );
        assert!(matches!(
            store.complete_tracker_effect(
                &mut stale,
                &claim_id,
                &request.idempotency_key,
                AutomationEffectExecution::Committed {
                    provider_receipt: "late-provider-result".into()
                },
            ),
            Err(AutomationRunError::StaleLease { .. })
        ));

        store
            .complete_claim_cancellation(&mut root, &claim_id, false)
            .unwrap();
        assert!(
            root.claims[&claim_id]
                .next_action
                .contains("descendant cancellation")
        );
        store
            .complete_claim_cancellation(&mut root, &claim_id, true)
            .unwrap();
        assert!(
            root.claims[&claim_id]
                .next_action
                .contains("ambiguous Tracker effects")
        );

        let effect = &mut root.claims.get_mut(&claim_id).unwrap().effects[0];
        effect.status = AutomationEffectStatus::Committed;
        effect.failure = None;
        effect.provider_receipt = Some("reconciled-provider-result".into());
        store
            .complete_claim_cancellation(&mut root, &claim_id, true)
            .unwrap();
        assert_eq!(
            root.claims[&claim_id].status,
            AutomationClaimStatus::Cancelled
        );
        assert_eq!(
            root.claims[&claim_id].cleanup.status,
            AutomationCleanupStatus::Eligible
        );
    }

    #[test]
    fn resume_reconciles_retained_identities_and_terminal_tracker_state_before_dispatch() {
        let repository = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let profile = profile(workspace.path());
        let digest = crate::canonical_sha256(&serde_json::to_value(&profile).unwrap()).unwrap();
        let (store, mut root) = AutomationRunStore::start(AutomationRunStart {
            repository: repository.path(),
            owner_thread_id: "task-resume",
            source_revision: "abc123",
            profile: &profile,
            profile_digest: &digest,
        })
        .unwrap();
        let claim_id = store.claim_fixture(&mut root, &issue(), 1).unwrap();
        let retained_worktree = root.claims[&claim_id].worktree.clone();
        fs::create_dir_all(&retained_worktree).unwrap();
        let retained_run_id = "child-run-1".to_owned();
        let retained_task = AgentHandle {
            thread_id: "issue-task-1".into(),
            task_path: "/root/issue-task-1".into(),
            parent_thread_id: "task-resume".into(),
        };
        let claim = root.claims.get_mut(&claim_id).unwrap();
        claim.issue_task = Some(retained_task.clone());
        claim.workflow_run_id = Some(retained_run_id.clone());
        store.save(&mut root).unwrap();

        store.pause(&mut root, "desktop pause").unwrap();
        store.begin_reconciliation(&mut root).unwrap();
        store
            .reconcile(
                &mut root,
                &profile,
                &[issue()],
                &[AutomationClaimReconciliation {
                    claim_id: claim_id.clone(),
                    issue_task_active: true,
                    descendants_cancelled: false,
                    tracker_terminal: false,
                    workflow_status: Some(RunStatus::Running),
                }],
            )
            .unwrap();
        assert_eq!(root.status, AutomationRootStatus::Running);
        assert_eq!(
            root.reconciliation,
            AutomationReconciliationStatus::Complete
        );
        assert_eq!(root.claims[&claim_id].worktree, retained_worktree);
        assert_eq!(root.claims[&claim_id].issue_task, Some(retained_task));
        assert_eq!(
            root.claims[&claim_id].workflow_run_id.as_deref(),
            Some(retained_run_id.as_str())
        );

        store.pause(&mut root, "tracker refresh").unwrap();
        store.begin_reconciliation(&mut root).unwrap();
        let mut terminal = issue();
        terminal.state = "Done".into();
        let blocked = store.reconcile(
            &mut root,
            &profile,
            &[terminal.clone()],
            &[AutomationClaimReconciliation {
                claim_id: claim_id.clone(),
                issue_task_active: false,
                descendants_cancelled: false,
                tracker_terminal: true,
                workflow_status: Some(RunStatus::Cancelled),
            }],
        );
        assert!(matches!(
            blocked,
            Err(AutomationRunError::ReconciliationBlocked(_))
        ));
        assert_eq!(root.status, AutomationRootStatus::Suspended);
        assert_eq!(root.reconciliation, AutomationReconciliationStatus::Blocked);

        store.begin_reconciliation(&mut root).unwrap();
        store
            .reconcile(
                &mut root,
                &profile,
                &[terminal],
                &[AutomationClaimReconciliation {
                    claim_id: claim_id.clone(),
                    issue_task_active: false,
                    descendants_cancelled: true,
                    tracker_terminal: true,
                    workflow_status: Some(RunStatus::Cancelled),
                }],
            )
            .unwrap();
        assert_eq!(
            root.claims[&claim_id].status,
            AutomationClaimStatus::Cancelled
        );
        assert_eq!(
            root.claims[&claim_id].cleanup.status,
            AutomationCleanupStatus::Eligible
        );
        assert!(
            root.claims[&claim_id]
                .next_action
                .contains("cleanup eligible")
        );
    }

    #[test]
    fn retry_schedule_is_deterministic_capped_and_recovered_from_checkpoint() {
        let repository = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let mut profile = profile(workspace.path());
        profile.polling.interval_ms = 1_000;
        profile.agent.max_retry_backoff_ms = 4_000;
        let digest = crate::canonical_sha256(&serde_json::to_value(&profile).unwrap()).unwrap();
        let (store, mut root) = AutomationRunStore::start(AutomationRunStart {
            repository: repository.path(),
            owner_thread_id: "task-retry",
            source_revision: "abc123",
            profile: &profile,
            profile_digest: &digest,
        })
        .unwrap();
        let claim_id = store.claim_fixture(&mut root, &issue(), 1).unwrap();
        let identity = root.claims[&claim_id].clone();

        let first = store
            .schedule_claim_retry(
                &mut root,
                &claim_id,
                &profile,
                10_000,
                "provider unavailable",
            )
            .unwrap();
        assert_eq!(first.delay_ms, 1_000);
        assert_eq!(first.ready_at_ms, 11_000);
        assert!(matches!(
            store.dispatch_due_claim_work(&mut root, &claim_id, true, 10_999),
            Err(AutomationRunError::RetryNotReady(_))
        ));

        let recovered = store.load().unwrap();
        assert_eq!(recovered.claims[&claim_id].retry, Some(first.clone()));
        store
            .dispatch_due_claim_work(&mut root, &claim_id, true, 11_000)
            .unwrap();
        for (attempt, expected_delay) in [(2, 2_000), (3, 4_000), (4, 4_000)] {
            let scheduled = store
                .schedule_claim_retry(
                    &mut root,
                    &claim_id,
                    &profile,
                    20_000 + u64::from(attempt),
                    "transient failure",
                )
                .unwrap();
            assert_eq!(scheduled.attempt, attempt);
            assert_eq!(scheduled.delay_ms, expected_delay);
            store
                .dispatch_due_claim_work(&mut root, &claim_id, true, scheduled.ready_at_ms)
                .unwrap();
        }
        let claim = &root.claims[&claim_id];
        assert_eq!(claim.claim_id, identity.claim_id);
        assert_eq!(claim.issue_task, identity.issue_task);
        assert_eq!(claim.worktree, identity.worktree);
    }

    #[test]
    fn profile_reload_pins_active_claims_and_activates_only_on_future_dispatch() {
        let repository = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let profile = profile(workspace.path());
        let digest = crate::canonical_sha256(&serde_json::to_value(&profile).unwrap()).unwrap();
        let (store, mut root) = AutomationRunStore::start(AutomationRunStart {
            repository: repository.path(),
            owner_thread_id: "task-profile-reload",
            source_revision: "abc123",
            profile: &profile,
            profile_digest: &digest,
        })
        .unwrap();
        let first_issue = issue();
        let first_claim = store
            .coordinate_fixture(&mut root, &profile, std::slice::from_ref(&first_issue), 1)
            .unwrap()
            .dispatched_claim_ids[0]
            .clone();

        let mut candidate = profile.clone();
        candidate.agent.max_turns = 7;
        let candidate_digest =
            crate::canonical_sha256(&serde_json::to_value(&candidate).unwrap()).unwrap();
        store
            .stage_profile_revision(&mut root, &candidate, &candidate_digest)
            .unwrap();
        assert_eq!(
            root.profile_revision.status,
            AutomationProfileRevisionStatus::PendingValid
        );
        assert_eq!(root.profile_digest, digest);
        assert_eq!(root.claims[&first_claim].profile_digest, digest);
        assert_eq!(root.claims[&first_claim].profile_revision, 1);

        let second_issue = queued_issue("issue-39", "ORC-39", "Todo", Some(1));
        let saturated = store
            .coordinate_fixture(
                &mut root,
                &candidate,
                std::slice::from_ref(&second_issue),
                1,
            )
            .unwrap();
        assert!(saturated.dispatched_claim_ids.is_empty());
        assert_eq!(
            root.profile_revision.status,
            AutomationProfileRevisionStatus::PendingValid
        );

        store
            .update_claim(&mut root, &first_claim, |claim| {
                claim.status = AutomationClaimStatus::Completed;
            })
            .unwrap();
        let second_claim = store
            .coordinate_fixture(
                &mut root,
                &candidate,
                std::slice::from_ref(&second_issue),
                1,
            )
            .unwrap()
            .dispatched_claim_ids[0]
            .clone();
        assert_eq!(root.profile_digest, candidate_digest);
        assert_eq!(root.profile_revision.revision, 2);
        assert_eq!(
            root.profile_revision.status,
            AutomationProfileRevisionStatus::Active
        );
        assert_eq!(root.claims[&first_claim].profile_digest, digest);
        assert_eq!(root.claims[&second_claim].profile_digest, candidate_digest);
        assert_eq!(root.claims[&second_claim].profile_revision, 2);
        assert_eq!(
            store
                .load_profile_revision(&digest)
                .unwrap()
                .agent
                .max_turns,
            20
        );
        assert_eq!(store.load_profile().unwrap().agent.max_turns, 7);

        store
            .reject_profile_revision(
                &mut root,
                Some("rejected-digest"),
                &["agent.max_turns: must be positive".into()],
            )
            .unwrap();
        assert_eq!(root.profile_digest, candidate_digest);
        assert_eq!(
            root.profile_revision.status,
            AutomationProfileRevisionStatus::Rejected
        );
        assert_eq!(root.profile_revision.diagnostics.len(), 1);
        assert_eq!(store.load_profile().unwrap().agent.max_turns, 7);
    }

    #[test]
    fn max_turns_yields_to_continuation_without_replacing_claim_resources() {
        let repository = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let mut profile = profile(workspace.path());
        profile.agent.max_turns = 2;
        profile.polling.interval_ms = 500;
        let digest = crate::canonical_sha256(&serde_json::to_value(&profile).unwrap()).unwrap();
        let (store, mut root) = AutomationRunStore::start(AutomationRunStart {
            repository: repository.path(),
            owner_thread_id: "task-continuation",
            source_revision: "abc123",
            profile: &profile,
            profile_digest: &digest,
        })
        .unwrap();
        let claim_id = store.claim_fixture(&mut root, &issue(), 1).unwrap();
        let retained_worktree = root.claims[&claim_id].worktree.clone();

        let first = store
            .record_completed_invocation(&mut root, &claim_id, &profile, true, 1_000)
            .unwrap()
            .unwrap();
        assert!(!first.reset_turn_window);
        store
            .dispatch_due_claim_work(&mut root, &claim_id, true, first.ready_at_ms)
            .unwrap();
        let exhausted = store
            .record_completed_invocation(&mut root, &claim_id, &profile, true, 2_000)
            .unwrap()
            .unwrap();
        assert!(exhausted.reset_turn_window);
        assert!(root.claims[&claim_id].next_action.contains("max_turns"));

        store
            .dispatch_due_claim_work(&mut root, &claim_id, true, exhausted.ready_at_ms)
            .unwrap();
        let claim = &root.claims[&claim_id];
        assert_eq!(claim.workflow_invocations, 2);
        assert_eq!(claim.turns_in_window, 0);
        assert_eq!(claim.continuation_count, 1);
        assert_eq!(claim.worktree, retained_worktree);
        assert_eq!(claim.claim_id, claim_id);

        let terminal = store
            .record_completed_invocation(&mut root, &claim_id, &profile, false, 3_000)
            .unwrap();
        assert_eq!(terminal, None);
        assert_eq!(
            root.claims[&claim_id].status,
            AutomationClaimStatus::Completed
        );
        assert!(root.claims[&claim_id].retry.is_none());
    }

    #[test]
    fn liveness_distinguishes_progress_gate_retry_stall_and_cancellation() {
        let repository = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let mut profile = profile(workspace.path());
        profile.codex.stall_timeout_ms = 1_000;
        let digest = crate::canonical_sha256(&serde_json::to_value(&profile).unwrap()).unwrap();
        let (store, mut root) = AutomationRunStore::start(AutomationRunStart {
            repository: repository.path(),
            owner_thread_id: "task-liveness",
            source_revision: "abc123",
            profile: &profile,
            profile_digest: &digest,
        })
        .unwrap();
        let claim_id = store.claim_fixture(&mut root, &issue(), 1).unwrap();
        store
            .record_claim_progress(&mut root, &claim_id, 1_000)
            .unwrap();
        assert_eq!(
            automation_claim_liveness(&root.claims[&claim_id], &profile, 1_999),
            AutomationClaimLiveness::Active
        );
        assert_eq!(
            automation_claim_liveness(&root.claims[&claim_id], &profile, 2_000),
            AutomationClaimLiveness::Stalled
        );

        store
            .resolve_tracker_comment(
                &mut root,
                &claim_id,
                &profile,
                "Waiting for approval.",
                AutomationGatePolicy::AskHuman,
                |_| unreachable!(),
            )
            .unwrap();
        assert_eq!(
            automation_claim_liveness(&root.claims[&claim_id], &profile, 20_000),
            AutomationClaimLiveness::WaitingGate
        );
        root.claims.get_mut(&claim_id).unwrap().effects.clear();
        store
            .schedule_claim_retry(&mut root, &claim_id, &profile, 2_000, "retry")
            .unwrap();
        assert_eq!(
            automation_claim_liveness(&root.claims[&claim_id], &profile, 20_000),
            AutomationClaimLiveness::WaitingRetry
        );
        assert!(matches!(
            store.dispatch_due_claim_work(&mut root, &claim_id, false, 20_000),
            Err(AutomationRunError::InactiveClaim(_))
        ));
        assert!(root.claims[&claim_id].retry.is_none());
        store.cancel(&mut root).unwrap();
        assert_eq!(
            automation_claim_liveness(&root.claims[&claim_id], &profile, 20_000),
            AutomationClaimLiveness::Terminal
        );
        assert!(matches!(
            store.dispatch_due_claim_work(&mut root, &claim_id, true, u64::MAX),
            Err(AutomationRunError::InactiveClaim(_))
        ));
    }
}
