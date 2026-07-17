use crate::AgentHandle;
use crate::PromotionStatus;
use crate::RunCheckpoint;
use crate::RunStatus;
use crate::StepStatus;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use sha2::Digest;
use sha2::Sha256;
use std::cmp::Ordering;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use thiserror::Error;

/// Hard limits shared by every adapter over the execution query service.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionQueryLimits {
    pub max_page_items: usize,
    pub max_response_bytes: usize,
    pub max_checkpoint_bytes: u64,
    pub max_inline_value_bytes: usize,
    pub max_evidence_hash_bytes: u64,
    pub max_evidence_content_bytes: u64,
    pub max_text_bytes: usize,
    pub max_identity_bytes: usize,
    pub max_digest_bytes: usize,
}

impl Default for ExecutionQueryLimits {
    fn default() -> Self {
        Self {
            max_page_items: 100,
            max_response_bytes: 64 * 1024,
            max_checkpoint_bytes: 16 * 1024 * 1024,
            max_inline_value_bytes: 16 * 1024,
            max_evidence_hash_bytes: 32 * 1024 * 1024,
            max_evidence_content_bytes: 32 * 1024,
            max_text_bytes: 2 * 1024,
            max_identity_bytes: 512,
            max_digest_bytes: 8 * 1024,
        }
    }
}

/// An adapter-selected budget. It may narrow, but never widen, service limits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExecutionQueryBudget {
    pub max_items: usize,
    pub max_bytes: usize,
}

impl Default for ExecutionQueryBudget {
    fn default() -> Self {
        Self {
            max_items: 50,
            max_bytes: 32 * 1024,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExecutionSelector {
    Run,
    Steps {
        after: Option<String>,
    },
    Outputs {
        step_id: Option<String>,
        after: Option<String>,
    },
    Evidence {
        step_id: Option<String>,
        after: Option<String>,
    },
    EvidenceContent {
        evidence_id: String,
    },
    History {
        after: Option<HistoryCursor>,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryCursor {
    pub sequence: u64,
    pub item_id: String,
    pub revision: u64,
}

impl Ord for HistoryCursor {
    fn cmp(&self, other: &Self) -> Ordering {
        (&self.sequence, &self.item_id, &self.revision).cmp(&(
            &other.sequence,
            &other.item_id,
            &other.revision,
        ))
    }
}

impl PartialOrd for HistoryCursor {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionHistoryRecord {
    pub sequence: u64,
    pub item_id: String,
    pub revision: u64,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_id: Option<String>,
    pub summary: String,
}

impl ExecutionHistoryRecord {
    fn cursor(&self) -> HistoryCursor {
        HistoryCursor {
            sequence: self.sequence,
            item_id: self.item_id.clone(),
            revision: self.revision,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryReadRequest {
    pub parent_thread_id: String,
    pub run_id: String,
    pub after: Option<HistoryCursor>,
    pub limit: usize,
}

/// Implemented by the Codex rollout adapter. Repository state is deliberately
/// not made into a second semantic-history store.
#[async_trait::async_trait]
pub trait ExecutionHistorySource: Send + Sync + 'static {
    async fn read(
        &self,
        request: &HistoryReadRequest,
    ) -> Result<Vec<ExecutionHistoryRecord>, String>;
}

#[derive(Clone, Debug, Default)]
pub struct NoExecutionHistory;

#[async_trait::async_trait]
impl ExecutionHistorySource for NoExecutionHistory {
    async fn read(
        &self,
        _request: &HistoryReadRequest,
    ) -> Result<Vec<ExecutionHistoryRecord>, String> {
        Ok(Vec::new())
    }
}

#[derive(Clone)]
pub struct ExecutionQueryService {
    limits: ExecutionQueryLimits,
    history: Arc<dyn ExecutionHistorySource>,
}

impl Default for ExecutionQueryService {
    fn default() -> Self {
        Self::new(ExecutionQueryLimits::default())
    }
}

impl ExecutionQueryService {
    pub fn new(limits: ExecutionQueryLimits) -> Self {
        Self {
            limits,
            history: Arc::new(NoExecutionHistory),
        }
    }

    pub fn with_history_source(
        limits: ExecutionQueryLimits,
        history: Arc<dyn ExecutionHistorySource>,
    ) -> Self {
        Self { limits, history }
    }

    pub fn limits(&self) -> &ExecutionQueryLimits {
        &self.limits
    }

    pub async fn query(
        &self,
        repository: &Path,
        parent_thread_id: &str,
        run_id: &str,
        selector: ExecutionSelector,
        budget: ExecutionQueryBudget,
    ) -> Result<ExecutionQueryResult, ExecutionQueryError> {
        let budget = self.checked_budget(budget)?;
        let checkpoint = self.authorized_checkpoint(repository, parent_thread_id, run_id)?;
        let result = match selector {
            ExecutionSelector::Run => ExecutionQueryResult::Run(self.run_projection(&checkpoint)),
            ExecutionSelector::Steps { after } => ExecutionQueryResult::Steps(self.steps_page(
                &checkpoint,
                after.as_deref(),
                budget,
            )?),
            ExecutionSelector::Outputs { step_id, after } => ExecutionQueryResult::Outputs(
                self.outputs_page(&checkpoint, step_id.as_deref(), after.as_deref(), budget)?,
            ),
            ExecutionSelector::Evidence { step_id, after } => {
                ExecutionQueryResult::Evidence(self.evidence_page(
                    repository,
                    &checkpoint,
                    run_id,
                    step_id.as_deref(),
                    after.as_deref(),
                    budget,
                )?)
            }
            ExecutionSelector::EvidenceContent { evidence_id } => {
                ExecutionQueryResult::EvidenceContent(self.evidence_content(
                    repository,
                    &checkpoint,
                    run_id,
                    &evidence_id,
                    budget,
                )?)
            }
            ExecutionSelector::History { after } => ExecutionQueryResult::History(
                self.history_page(parent_thread_id, run_id, after, budget)
                    .await?,
            ),
        };
        if serialized_len(&result)? > budget.max_bytes {
            return Err(ExecutionQueryError::BudgetTooSmall);
        }
        Ok(result)
    }

    pub fn digest(
        &self,
        repository: &Path,
        parent_thread_id: &str,
        run_id: &str,
        max_bytes: usize,
    ) -> Result<RunDigest, ExecutionQueryError> {
        if max_bytes == 0 || max_bytes > self.limits.max_digest_bytes {
            return Err(ExecutionQueryError::InvalidBudget);
        }
        let checkpoint = self.authorized_checkpoint(repository, parent_thread_id, run_id)?;
        let state_bytes = serde_json_canonicalizer::to_vec(&checkpoint)
            .map_err(|error| ExecutionQueryError::InvalidCheckpoint(error.to_string()))?;
        let state_sha256 = sha256(&state_bytes);
        let next_action = truncate(&checkpoint.next_action, self.limits.max_text_bytes);
        let header = format!(
            "run {} status {}\nnext {}\n",
            checkpoint.run_id,
            status_name(&checkpoint.status),
            next_action.text
        );
        if header.len() > max_bytes {
            return Err(ExecutionQueryError::BudgetTooSmall);
        }

        let mut lines = checkpoint
            .steps
            .iter()
            .map(|(id, step)| {
                let detail = step
                    .error
                    .as_deref()
                    .or(step.approval_decision.as_deref())
                    .map(|text| truncate(text, self.limits.max_text_bytes.min(160)).text);
                let line = match detail {
                    Some(detail) => format!(
                        "step {id} {} attempts={} rounds={} — {detail}\n",
                        step_status_name(&step.status),
                        step.attempts,
                        step.rounds
                    ),
                    None => format!(
                        "step {id} {} attempts={} rounds={}\n",
                        step_status_name(&step.status),
                        step.attempts,
                        step.rounds
                    ),
                };
                (digest_priority(&step.status), id, line)
            })
            .collect::<Vec<_>>();
        lines.sort_by(|left, right| (left.0, left.1).cmp(&(right.0, right.1)));

        let mut text = header;
        let mut included = 0usize;
        for (_, _, line) in &lines {
            let remaining = lines.len() - included - 1;
            let footer = if remaining > 0 {
                format!("omitted {remaining} steps\n")
            } else {
                String::new()
            };
            if text.len() + line.len() + footer.len() > max_bytes {
                break;
            }
            text.push_str(line);
            included += 1;
        }
        let omitted_steps = lines.len() - included;
        if omitted_steps > 0 {
            let footer = format!("omitted {omitted_steps} steps\n");
            if text.len() + footer.len() > max_bytes {
                return Err(ExecutionQueryError::BudgetTooSmall);
            }
            text.push_str(&footer);
        }
        Ok(RunDigest {
            run_id: checkpoint.run_id,
            state_sha256,
            text,
            omitted_steps,
        })
    }

    fn checked_budget(
        &self,
        budget: ExecutionQueryBudget,
    ) -> Result<ExecutionQueryBudget, ExecutionQueryError> {
        if budget.max_items == 0
            || budget.max_bytes == 0
            || budget.max_items > self.limits.max_page_items
            || budget.max_bytes > self.limits.max_response_bytes
        {
            return Err(ExecutionQueryError::InvalidBudget);
        }
        Ok(budget)
    }

    fn authorized_checkpoint(
        &self,
        repository: &Path,
        parent_thread_id: &str,
        run_id: &str,
    ) -> Result<RunCheckpoint, ExecutionQueryError> {
        validate_identity(run_id, self.limits.max_identity_bytes)?;
        let path = repository
            .join(".codex/orchestra/runs")
            .join(run_id)
            .join("state.json");
        let metadata = fs::metadata(&path).map_err(storage_error)?;
        if metadata.len() > self.limits.max_checkpoint_bytes {
            return Err(ExecutionQueryError::CheckpointTooLarge);
        }
        let checkpoint: RunCheckpoint =
            serde_json::from_slice(&fs::read(&path).map_err(storage_error)?)
                .map_err(|error| ExecutionQueryError::InvalidCheckpoint(error.to_string()))?;
        if checkpoint.run_id != run_id {
            return Err(ExecutionQueryError::InvalidCheckpoint(
                "run id does not match checkpoint path".into(),
            ));
        }
        validate_identity(&checkpoint.run_id, self.limits.max_identity_bytes)?;
        if checkpoint.parent_thread_id != parent_thread_id {
            return Err(ExecutionQueryError::Unauthorized);
        }
        Ok(checkpoint)
    }

    fn run_projection(&self, checkpoint: &RunCheckpoint) -> RunProjection {
        let mut counts = StepCounts::default();
        for step in checkpoint.steps.values() {
            counts.add(&step.status);
        }
        RunProjection {
            schema_version: checkpoint.schema_version,
            run_id: checkpoint.run_id.clone(),
            workflow_sha256: checkpoint.workflow_sha256.clone(),
            source_revision: checkpoint.source_revision.clone(),
            status: checkpoint.status.clone(),
            promotion: checkpoint.promotion.clone(),
            step_counts: counts,
            next_action: truncate(&checkpoint.next_action, self.limits.max_text_bytes),
        }
    }

    fn steps_page(
        &self,
        checkpoint: &RunCheckpoint,
        after: Option<&str>,
        budget: ExecutionQueryBudget,
    ) -> Result<StepsPage, ExecutionQueryError> {
        if let Some(after) = after {
            validate_identity(after, self.limits.max_identity_bytes)?;
        }
        let mut all = checkpoint
            .steps
            .iter()
            .filter(|(id, _)| after.is_none_or(|after| id.as_str() > after))
            .map(|(id, step)| StepProjection {
                id: id.clone(),
                status: step.status.clone(),
                attempts: step.attempts,
                rounds: step.rounds,
                agent: step.agent.as_ref().map(AgentReference::from),
                context_sha256: step.context_sha256.clone(),
                approval_decision: step
                    .approval_decision
                    .as_deref()
                    .map(|text| truncate(text, self.limits.max_text_bytes)),
                error: step
                    .error
                    .as_deref()
                    .map(|text| truncate(text, self.limits.max_text_bytes)),
                output_count: step.outputs.len(),
            })
            .collect::<Vec<_>>();
        let requested_more = all.len() > budget.max_items;
        all.truncate(budget.max_items);
        let (items, next) = fit_page(
            all,
            requested_more,
            budget.max_bytes,
            |item| item.id.clone(),
            |items, next| ExecutionQueryResult::Steps(StepsPage { items, next }),
        )?;
        Ok(StepsPage { items, next })
    }

    fn outputs_page(
        &self,
        checkpoint: &RunCheckpoint,
        step_filter: Option<&str>,
        after: Option<&str>,
        budget: ExecutionQueryBudget,
    ) -> Result<OutputsPage, ExecutionQueryError> {
        if let Some(step_id) = step_filter {
            validate_identity(step_id, self.limits.max_identity_bytes)?;
            if !checkpoint.steps.contains_key(step_id) {
                return Err(ExecutionQueryError::NotFound);
            }
        }
        let inline_limit = self
            .limits
            .max_inline_value_bytes
            .min(budget.max_bytes.saturating_sub(1024) / budget.max_items.max(1));
        let mut all = Vec::new();
        for (step_id, step) in &checkpoint.steps {
            if step_filter.is_some_and(|filter| filter != step_id) {
                continue;
            }
            for (name, value) in &step.outputs {
                let cursor = output_cursor(step_id, name);
                if after.is_some_and(|after| cursor.as_str() <= after) {
                    continue;
                }
                validate_identity(name, self.limits.max_identity_bytes)?;
                let canonical = serde_json_canonicalizer::to_vec(value)
                    .map_err(|error| ExecutionQueryError::InvalidCheckpoint(error.to_string()))?;
                all.push(OutputProjection {
                    step_id: step_id.clone(),
                    name: name.clone(),
                    sha256: sha256(&canonical),
                    canonical_bytes: canonical.len(),
                    value: (canonical.len() <= inline_limit).then(|| value.clone()),
                    cursor,
                });
            }
        }
        all.sort_by(|left, right| left.cursor.cmp(&right.cursor));
        let requested_more = all.len() > budget.max_items;
        all.truncate(budget.max_items);
        let (items, next) = fit_page(
            all,
            requested_more,
            budget.max_bytes,
            |item| item.cursor.clone(),
            |items, next| ExecutionQueryResult::Outputs(OutputsPage { items, next }),
        )?;
        Ok(OutputsPage { items, next })
    }

    fn evidence_page(
        &self,
        repository: &Path,
        checkpoint: &RunCheckpoint,
        run_id: &str,
        step_filter: Option<&str>,
        after: Option<&str>,
        budget: ExecutionQueryBudget,
    ) -> Result<EvidencePage, ExecutionQueryError> {
        if let Some(step_id) = step_filter {
            validate_identity(step_id, self.limits.max_identity_bytes)?;
            if !checkpoint.steps.contains_key(step_id) {
                return Err(ExecutionQueryError::NotFound);
            }
        }
        let root = repository
            .join(".codex/orchestra/runs")
            .join(run_id)
            .join("evidence");
        let mut files = Vec::new();
        collect_evidence(&root, &root, &mut files)?;
        files.sort();
        let mut all = Vec::new();
        for (relative, path) in files {
            if after.is_some_and(|after| relative.as_str() <= after) {
                continue;
            }
            let step_id = evidence_step_id(&relative);
            if step_filter.is_some_and(|filter| step_id.as_deref() != Some(filter)) {
                continue;
            }
            validate_identity(&relative, self.limits.max_identity_bytes)?;
            let metadata = fs::metadata(&path).map_err(storage_error)?;
            let digest = if metadata.len() <= self.limits.max_evidence_hash_bytes {
                Some(sha256(&fs::read(&path).map_err(storage_error)?))
            } else {
                None
            };
            all.push(EvidenceReference {
                evidence_id: evidence_id(&relative),
                name: evidence_name(&relative, self.limits.max_identity_bytes)?,
                path: relative,
                kind: evidence_kind(&path),
                provenance: evidence_provenance(&path),
                step_id,
                bytes: metadata.len(),
                sha256: digest,
                availability: if metadata.len() <= self.limits.max_evidence_content_bytes {
                    EvidenceAvailability::Available
                } else {
                    EvidenceAvailability::ContentTooLarge
                },
            });
        }
        let requested_more = all.len() > budget.max_items;
        all.truncate(budget.max_items);
        let (items, next) = fit_page(
            all,
            requested_more,
            budget.max_bytes,
            |item| item.path.clone(),
            |items, next| ExecutionQueryResult::Evidence(EvidencePage { items, next }),
        )?;
        Ok(EvidencePage { items, next })
    }

    fn evidence_content(
        &self,
        repository: &Path,
        _checkpoint: &RunCheckpoint,
        run_id: &str,
        requested_evidence_id: &str,
        budget: ExecutionQueryBudget,
    ) -> Result<EvidenceContentProjection, ExecutionQueryError> {
        validate_identity(requested_evidence_id, self.limits.max_identity_bytes)?;
        let root = repository
            .join(".codex/orchestra/runs")
            .join(run_id)
            .join("evidence");
        let mut files = Vec::new();
        collect_evidence(&root, &root, &mut files)?;
        let (relative, path) = files
            .into_iter()
            .find(|(relative, _)| evidence_id(relative) == requested_evidence_id)
            .ok_or(ExecutionQueryError::NotFound)?;
        let kind = evidence_kind(&path);
        let provenance = evidence_provenance(&path);
        let name = evidence_name(&relative, self.limits.max_identity_bytes)?;
        let bytes = fs::read(&path).map_err(storage_error)?;
        let content_bytes = u64::try_from(bytes.len())
            .map_err(|_| ExecutionQueryError::Storage("evidence length exceeds u64".into()))?;
        let digest = if content_bytes <= self.limits.max_evidence_hash_bytes {
            Some(sha256(&bytes))
        } else {
            None
        };
        if content_bytes > self.limits.max_evidence_content_bytes {
            return Ok(EvidenceContentProjection {
                evidence_id: requested_evidence_id.into(),
                name,
                kind,
                provenance,
                availability: EvidenceAvailability::ContentTooLarge,
                bytes: content_bytes,
                sha256: digest,
                media_type: evidence_media_type(&path).into(),
                content: None,
            });
        }
        let content = match String::from_utf8(bytes) {
            Ok(content) => content,
            Err(_) => {
                return Ok(EvidenceContentProjection {
                    evidence_id: requested_evidence_id.into(),
                    name,
                    kind,
                    provenance,
                    availability: EvidenceAvailability::Malformed,
                    bytes: content_bytes,
                    sha256: digest,
                    media_type: evidence_media_type(&path).into(),
                    content: None,
                });
            }
        };
        let projection = EvidenceContentProjection {
            evidence_id: requested_evidence_id.into(),
            name,
            kind,
            provenance,
            availability: EvidenceAvailability::Available,
            bytes: content_bytes,
            sha256: digest,
            media_type: evidence_media_type(&path).into(),
            content: Some(content),
        };
        if serialized_len(&ExecutionQueryResult::EvidenceContent(projection.clone()))?
            > budget.max_bytes
        {
            return Ok(EvidenceContentProjection {
                content: None,
                availability: EvidenceAvailability::ContentTooLarge,
                ..projection
            });
        }
        Ok(projection)
    }

    async fn history_page(
        &self,
        parent_thread_id: &str,
        run_id: &str,
        after: Option<HistoryCursor>,
        budget: ExecutionQueryBudget,
    ) -> Result<HistoryPage, ExecutionQueryError> {
        if let Some(after) = &after {
            validate_identity(&after.item_id, self.limits.max_identity_bytes)?;
        }
        let mut items = self
            .history
            .read(&HistoryReadRequest {
                parent_thread_id: parent_thread_id.into(),
                run_id: run_id.into(),
                after: after.clone(),
                limit: budget.max_items.saturating_add(1),
            })
            .await
            .map_err(ExecutionQueryError::History)?;
        for item in &mut items {
            validate_identity(&item.item_id, self.limits.max_identity_bytes)?;
            validate_identity(&item.kind, self.limits.max_identity_bytes)?;
            if let Some(step_id) = &item.step_id {
                validate_identity(step_id, self.limits.max_identity_bytes)?;
            }
            item.summary = truncate(&item.summary, self.limits.max_text_bytes).text;
        }
        items.sort_by_key(ExecutionHistoryRecord::cursor);
        items.retain(|item| after.as_ref().is_none_or(|after| item.cursor() > *after));
        items.dedup_by_key(|item| item.cursor());
        let requested_more = items.len() > budget.max_items;
        items.truncate(budget.max_items);
        let (items, next) = fit_page(
            items,
            requested_more,
            budget.max_bytes,
            ExecutionHistoryRecord::cursor,
            |items, next| ExecutionQueryResult::History(HistoryPage { items, next }),
        )?;
        Ok(HistoryPage { items, next })
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "selector", content = "result", rename_all = "snake_case")]
pub enum ExecutionQueryResult {
    Run(RunProjection),
    Steps(StepsPage),
    Outputs(OutputsPage),
    Evidence(EvidencePage),
    EvidenceContent(EvidenceContentProjection),
    History(HistoryPage),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BoundedText {
    pub text: String,
    pub truncated: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StepCounts {
    pub pending: usize,
    pub running: usize,
    pub retrying: usize,
    pub waiting_approval: usize,
    pub completed: usize,
    pub failed: usize,
    pub cancelled: usize,
}

impl StepCounts {
    fn add(&mut self, status: &StepStatus) {
        match status {
            StepStatus::Pending => self.pending += 1,
            StepStatus::Running => self.running += 1,
            StepStatus::Retrying => self.retrying += 1,
            StepStatus::WaitingApproval => self.waiting_approval += 1,
            StepStatus::Completed => self.completed += 1,
            StepStatus::Failed => self.failed += 1,
            StepStatus::Cancelled => self.cancelled += 1,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunProjection {
    pub schema_version: u32,
    pub run_id: String,
    pub workflow_sha256: String,
    pub source_revision: String,
    pub status: RunStatus,
    pub promotion: PromotionStatus,
    pub step_counts: StepCounts,
    pub next_action: BoundedText,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentReference {
    pub thread_id: String,
    pub task_path: String,
}

impl From<&AgentHandle> for AgentReference {
    fn from(value: &AgentHandle) -> Self {
        Self {
            thread_id: value.thread_id.clone(),
            task_path: value.task_path.clone(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StepProjection {
    pub id: String,
    pub status: StepStatus,
    pub attempts: u32,
    pub rounds: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentReference>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval_decision: Option<BoundedText>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<BoundedText>,
    pub output_count: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StepsPage {
    pub items: Vec<StepProjection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputProjection {
    pub step_id: String,
    pub name: String,
    pub sha256: String,
    pub canonical_bytes: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    #[serde(skip)]
    cursor: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputsPage {
    pub items: Vec<OutputProjection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceReference {
    pub evidence_id: String,
    pub name: String,
    pub path: String,
    pub kind: EvidenceKind,
    pub provenance: EvidenceProvenance,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_id: Option<String>,
    pub bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    pub availability: EvidenceAvailability,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    Check,
    Change,
    Skill,
    Other,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceProvenance {
    RuntimeCheck,
    RuntimeChange,
    SkillSnapshot,
    RuntimeOther,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceAvailability {
    Available,
    ContentTooLarge,
    Malformed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceContentProjection {
    pub evidence_id: String,
    pub name: String,
    pub kind: EvidenceKind,
    pub provenance: EvidenceProvenance,
    pub availability: EvidenceAvailability,
    pub bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    pub media_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidencePage {
    pub items: Vec<EvidenceReference>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryPage {
    pub items: Vec<ExecutionHistoryRecord>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<HistoryCursor>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunDigest {
    pub run_id: String,
    pub state_sha256: String,
    pub text: String,
    pub omitted_steps: usize,
}

#[derive(Debug, Error)]
pub enum ExecutionQueryError {
    #[error("query is not authorized for this task")]
    Unauthorized,
    #[error("execution record was not found")]
    NotFound,
    #[error("query budget is invalid")]
    InvalidBudget,
    #[error("query budget is too small for one projection")]
    BudgetTooSmall,
    #[error("query identity is invalid")]
    InvalidIdentity,
    #[error("checkpoint exceeds the query read limit")]
    CheckpointTooLarge,
    #[error("checkpoint is invalid: {0}")]
    InvalidCheckpoint(String),
    #[error("execution history is unavailable: {0}")]
    History(String),
    #[error("query storage failed: {0}")]
    Storage(String),
}

fn fit_page<T: Clone, C: Clone, F, G>(
    mut items: Vec<T>,
    mut has_more: bool,
    max_bytes: usize,
    cursor: G,
    build: F,
) -> Result<(Vec<T>, Option<C>), ExecutionQueryError>
where
    F: Fn(Vec<T>, Option<C>) -> ExecutionQueryResult,
    G: Fn(&T) -> C,
{
    let started_with_items = !items.is_empty();
    loop {
        let next = if has_more {
            items.last().map(&cursor)
        } else {
            None
        };
        if serialized_len(&build(items.clone(), next.clone()))? <= max_bytes {
            return Ok((items, next));
        }
        if items.pop().is_none() || (started_with_items && items.is_empty()) {
            return Err(ExecutionQueryError::BudgetTooSmall);
        }
        has_more = true;
    }
}

fn serialized_len(value: &ExecutionQueryResult) -> Result<usize, ExecutionQueryError> {
    serde_json::to_vec(value)
        .map(|bytes| bytes.len())
        .map_err(|error| ExecutionQueryError::InvalidCheckpoint(error.to_string()))
}

fn validate_identity(value: &str, max_bytes: usize) -> Result<(), ExecutionQueryError> {
    if value.is_empty()
        || value.len() > max_bytes
        || value.contains("..")
        || value
            .bytes()
            .any(|byte| byte == 0 || byte.is_ascii_control())
    {
        return Err(ExecutionQueryError::InvalidIdentity);
    }
    Ok(())
}

fn output_cursor(step_id: &str, name: &str) -> String {
    format!("{step_id}\u{0}{name}")
}

fn collect_evidence(
    root: &Path,
    current: &Path,
    files: &mut Vec<(String, PathBuf)>,
) -> Result<(), ExecutionQueryError> {
    let entries = match fs::read_dir(current) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(storage_error(error)),
    };
    for entry in entries {
        let entry = entry.map_err(storage_error)?;
        let file_type = entry.file_type().map_err(storage_error)?;
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            collect_evidence(root, &entry.path(), files)?;
        } else if file_type.is_file() {
            let relative = entry
                .path()
                .strip_prefix(root)
                .map_err(|error| ExecutionQueryError::Storage(error.to_string()))?
                .to_string_lossy()
                .replace(std::path::MAIN_SEPARATOR, "/");
            files.push((relative, entry.path()));
        }
    }
    Ok(())
}

fn evidence_kind(path: &Path) -> EvidenceKind {
    match path
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .rev()
        .nth(1)
    {
        Some("checks") => EvidenceKind::Check,
        Some("changes") => EvidenceKind::Change,
        Some("skills") => EvidenceKind::Skill,
        _ => EvidenceKind::Other,
    }
}

fn evidence_provenance(path: &Path) -> EvidenceProvenance {
    match evidence_kind(path) {
        EvidenceKind::Check => EvidenceProvenance::RuntimeCheck,
        EvidenceKind::Change => EvidenceProvenance::RuntimeChange,
        EvidenceKind::Skill => EvidenceProvenance::SkillSnapshot,
        EvidenceKind::Other => EvidenceProvenance::RuntimeOther,
    }
}

fn evidence_id(relative: &str) -> String {
    sha256(relative.as_bytes())
}

fn evidence_name(relative: &str, max_bytes: usize) -> Result<String, ExecutionQueryError> {
    let name = Path::new(relative)
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(ExecutionQueryError::InvalidIdentity)?;
    validate_identity(name, max_bytes)?;
    Ok(name.into())
}

fn evidence_media_type(path: &Path) -> &'static str {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("json") => "application/json",
        Some("patch" | "diff") => "text/x-diff",
        Some("md") => "text/markdown",
        Some("log" | "txt") => "text/plain",
        _ => "text/plain",
    }
}

fn evidence_step_id(path: &str) -> Option<String> {
    let (directory, file) = path.split_once('/')?;
    if !matches!(directory, "checks" | "changes") || file == "promoted.patch" {
        return None;
    }
    let stem = file.rsplit_once('.').map_or(file, |(stem, _)| stem);
    stem.rsplit_once('-').map(|(step, _)| step.to_string())
}

fn truncate(value: &str, max_bytes: usize) -> BoundedText {
    if value.len() <= max_bytes {
        return BoundedText {
            text: value.into(),
            truncated: false,
        };
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    BoundedText {
        text: value[..end].into(),
        truncated: true,
    }
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn status_name(status: &RunStatus) -> &'static str {
    match status {
        RunStatus::Pending => "pending",
        RunStatus::Running => "running",
        RunStatus::WaitingApproval => "waiting_approval",
        RunStatus::Completed => "completed",
        RunStatus::Failed => "failed",
        RunStatus::Cancelled => "cancelled",
    }
}

fn step_status_name(status: &StepStatus) -> &'static str {
    match status {
        StepStatus::Pending => "pending",
        StepStatus::Running => "running",
        StepStatus::Retrying => "retrying",
        StepStatus::WaitingApproval => "waiting_approval",
        StepStatus::Completed => "completed",
        StepStatus::Failed => "failed",
        StepStatus::Cancelled => "cancelled",
    }
}

fn digest_priority(status: &StepStatus) -> u8 {
    match status {
        StepStatus::Failed | StepStatus::WaitingApproval => 0,
        StepStatus::Running | StepStatus::Retrying => 1,
        StepStatus::Pending => 2,
        StepStatus::Cancelled => 3,
        StepStatus::Completed => 4,
    }
}

fn storage_error(error: std::io::Error) -> ExecutionQueryError {
    if error.kind() == std::io::ErrorKind::NotFound {
        ExecutionQueryError::NotFound
    } else {
        ExecutionQueryError::Storage(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PromotionStatus;
    use crate::StepCheckpoint;
    use std::collections::BTreeMap;
    use tempfile::tempdir;

    fn checkpoint(repository: &Path) -> RunCheckpoint {
        let mut steps = BTreeMap::new();
        steps.insert(
            "done".into(),
            StepCheckpoint {
                status: StepStatus::Completed,
                attempts: 1,
                rounds: 1,
                outputs: BTreeMap::from([("result".into(), serde_json::json!({"ok": true}))]),
                ..StepCheckpoint::default()
            },
        );
        steps.insert(
            "failed".into(),
            StepCheckpoint {
                status: StepStatus::Failed,
                attempts: 2,
                error: Some("failure detail ".repeat(200)),
                ..StepCheckpoint::default()
            },
        );
        RunCheckpoint {
            schema_version: 4,
            run_id: "run-1".into(),
            workflow_sha256: "workflow".into(),
            inputs: BTreeMap::new(),
            inputs_sha256: "inputs".into(),
            skills: Default::default(),
            skills_sha256: "skills".into(),
            parent_thread_id: "task-1".into(),
            repository: repository.into(),
            source_revision: "revision".into(),
            status: RunStatus::Failed,
            promotion: PromotionStatus::NotRequired,
            steps,
            next_action: "inspect the failure".into(),
        }
    }

    fn write_run(repository: &Path, checkpoint: &RunCheckpoint) {
        let root = repository.join(".codex/orchestra/runs/run-1");
        fs::create_dir_all(root.join("evidence/checks")).unwrap();
        fs::create_dir_all(root.join("evidence/changes")).unwrap();
        fs::create_dir_all(root.join("evidence/other")).unwrap();
        fs::write(
            root.join("state.json"),
            serde_json::to_vec(checkpoint).unwrap(),
        )
        .unwrap();
        fs::write(root.join("evidence/checks/failed-2.json"), b"check").unwrap();
        fs::write(root.join("evidence/changes/done-1.patch"), b"patch").unwrap();
        fs::write(root.join("evidence/other/empty.txt"), b"").unwrap();
        fs::write(root.join("evidence/other/malformed.bin"), [0xff, 0xfe]).unwrap();
    }

    #[tokio::test]
    async fn authorization_precedes_every_selector() {
        let repository = tempdir().unwrap();
        write_run(repository.path(), &checkpoint(repository.path()));
        let service = ExecutionQueryService::default();
        for selector in [
            ExecutionSelector::Run,
            ExecutionSelector::Steps { after: None },
            ExecutionSelector::Outputs {
                step_id: None,
                after: None,
            },
            ExecutionSelector::Evidence {
                step_id: None,
                after: None,
            },
            ExecutionSelector::EvidenceContent {
                evidence_id: evidence_id("checks/failed-2.json"),
            },
            ExecutionSelector::History { after: None },
        ] {
            assert!(matches!(
                service
                    .query(
                        repository.path(),
                        "another-task",
                        "run-1",
                        selector,
                        ExecutionQueryBudget::default()
                    )
                    .await,
                Err(ExecutionQueryError::Unauthorized)
            ));
        }
    }

    #[tokio::test]
    async fn fixed_selectors_are_typed_paginated_and_bounded() {
        let repository = tempdir().unwrap();
        let mut checkpoint = checkpoint(repository.path());
        checkpoint
            .steps
            .get_mut("done")
            .unwrap()
            .outputs
            .insert("large".into(), Value::String("x".repeat(20_000)));
        write_run(repository.path(), &checkpoint);
        let service = ExecutionQueryService::default();
        let budget = ExecutionQueryBudget {
            max_items: 1,
            max_bytes: 2048,
        };

        let steps = service
            .query(
                repository.path(),
                "task-1",
                "run-1",
                ExecutionSelector::Steps { after: None },
                budget,
            )
            .await
            .unwrap();
        let ExecutionQueryResult::Steps(steps) = steps else {
            panic!("expected steps");
        };
        assert_eq!(steps.items.len(), 1);
        assert!(steps.next.is_some());

        let outputs = service
            .query(
                repository.path(),
                "task-1",
                "run-1",
                ExecutionSelector::Outputs {
                    step_id: Some("done".into()),
                    after: None,
                },
                budget,
            )
            .await
            .unwrap();
        assert!(serialized_len(&outputs).unwrap() <= budget.max_bytes);
        let ExecutionQueryResult::Outputs(outputs) = outputs else {
            panic!("expected outputs");
        };
        assert_eq!(outputs.items.len(), 1);
        assert!(outputs.items[0].value.is_none());

        let evidence = service
            .query(
                repository.path(),
                "task-1",
                "run-1",
                ExecutionSelector::Evidence {
                    step_id: Some("failed".into()),
                    after: None,
                },
                budget,
            )
            .await
            .unwrap();
        let ExecutionQueryResult::Evidence(evidence) = evidence else {
            panic!("expected evidence");
        };
        assert_eq!(evidence.items[0].path, "checks/failed-2.json");
        assert_eq!(evidence.items[0].name, "failed-2.json");
        assert_eq!(
            evidence.items[0].provenance,
            EvidenceProvenance::RuntimeCheck
        );
        assert_eq!(
            evidence.items[0].availability,
            EvidenceAvailability::Available
        );
        assert_eq!(
            evidence.items[0].sha256.as_deref(),
            Some(sha256(b"check").as_str())
        );
    }

    #[tokio::test]
    async fn evidence_content_is_opaque_authorized_bounded_and_explicit() {
        let repository = tempdir().unwrap();
        write_run(repository.path(), &checkpoint(repository.path()));
        let service = ExecutionQueryService::default();
        let result = service
            .query(
                repository.path(),
                "task-1",
                "run-1",
                ExecutionSelector::EvidenceContent {
                    evidence_id: evidence_id("checks/failed-2.json"),
                },
                ExecutionQueryBudget::default(),
            )
            .await
            .unwrap();
        let ExecutionQueryResult::EvidenceContent(content) = result else {
            panic!("expected evidence content");
        };
        assert_eq!(content.name, "failed-2.json");
        assert_eq!(content.content.as_deref(), Some("check"));
        assert_eq!(content.bytes, 5);
        assert_eq!(content.media_type, "application/json");
        assert_eq!(content.sha256.as_deref(), Some(sha256(b"check").as_str()));
        assert!(!serde_json::to_string(&content).unwrap().contains("checks/"));

        let empty = service
            .query(
                repository.path(),
                "task-1",
                "run-1",
                ExecutionSelector::EvidenceContent {
                    evidence_id: evidence_id("other/empty.txt"),
                },
                ExecutionQueryBudget::default(),
            )
            .await
            .unwrap();
        let ExecutionQueryResult::EvidenceContent(empty) = empty else {
            panic!("expected empty evidence projection");
        };
        assert_eq!(empty.availability, EvidenceAvailability::Available);
        assert_eq!(empty.content.as_deref(), Some(""));
        assert_eq!(empty.bytes, 0);
        assert_eq!(empty.sha256.as_deref(), Some(sha256(b"").as_str()));

        let malformed = service
            .query(
                repository.path(),
                "task-1",
                "run-1",
                ExecutionSelector::EvidenceContent {
                    evidence_id: evidence_id("other/malformed.bin"),
                },
                ExecutionQueryBudget::default(),
            )
            .await
            .unwrap();
        let ExecutionQueryResult::EvidenceContent(malformed) = malformed else {
            panic!("expected malformed evidence projection");
        };
        assert_eq!(malformed.availability, EvidenceAvailability::Malformed);
        assert!(malformed.content.is_none());

        let unknown = service
            .query(
                repository.path(),
                "task-1",
                "run-1",
                ExecutionSelector::EvidenceContent {
                    evidence_id: "0".repeat(64),
                },
                ExecutionQueryBudget::default(),
            )
            .await;
        assert!(matches!(unknown, Err(ExecutionQueryError::NotFound)));
    }

    #[derive(Default)]
    struct FakeHistory;

    #[async_trait::async_trait]
    impl ExecutionHistorySource for FakeHistory {
        async fn read(
            &self,
            request: &HistoryReadRequest,
        ) -> Result<Vec<ExecutionHistoryRecord>, String> {
            assert_eq!(request.parent_thread_id, "task-1");
            Ok((1..=3)
                .rev()
                .map(|sequence| ExecutionHistoryRecord {
                    sequence,
                    item_id: format!("item-{sequence}"),
                    revision: 1,
                    kind: "step".into(),
                    step_id: Some("done".into()),
                    summary: "summary".into(),
                })
                .collect())
        }
    }

    #[tokio::test]
    async fn history_uses_injected_rollout_source_with_common_pagination() {
        let repository = tempdir().unwrap();
        write_run(repository.path(), &checkpoint(repository.path()));
        let service = ExecutionQueryService::with_history_source(
            ExecutionQueryLimits::default(),
            Arc::new(FakeHistory),
        );
        let result = service
            .query(
                repository.path(),
                "task-1",
                "run-1",
                ExecutionSelector::History { after: None },
                ExecutionQueryBudget {
                    max_items: 2,
                    max_bytes: 2048,
                },
            )
            .await
            .unwrap();
        let ExecutionQueryResult::History(page) = result else {
            panic!("expected history");
        };
        assert_eq!(
            page.items
                .iter()
                .map(|item| item.sequence)
                .collect::<Vec<_>>(),
            [1, 2]
        );
        assert_eq!(page.next.unwrap().sequence, 2);
    }

    #[test]
    fn digest_is_deterministic_prioritized_and_hard_bounded() {
        let repository = tempdir().unwrap();
        write_run(repository.path(), &checkpoint(repository.path()));
        let service = ExecutionQueryService::default();
        let first = service
            .digest(repository.path(), "task-1", "run-1", 512)
            .unwrap();
        let second = service
            .digest(repository.path(), "task-1", "run-1", 512)
            .unwrap();
        assert_eq!(first, second);
        assert!(first.text.len() <= 512);
        assert!(first.text.contains("step failed failed"));
        assert!(first.text.find("step failed").unwrap() < first.text.find("step done").unwrap());
    }

    #[tokio::test]
    async fn run_ids_cannot_escape_the_authoritative_run_root() {
        let repository = tempdir().unwrap();
        let result = ExecutionQueryService::default()
            .query(
                repository.path(),
                "task-1",
                "../run-1",
                ExecutionSelector::Run,
                ExecutionQueryBudget::default(),
            )
            .await;
        assert!(matches!(result, Err(ExecutionQueryError::InvalidIdentity)));
    }
}
