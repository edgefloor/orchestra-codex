use super::*;
use async_trait::async_trait;
use codex_orchestra_core::AgentHandle;
use codex_orchestra_core::AgentOutcome;
use codex_orchestra_core::AgentStatus;
use codex_orchestra_core::AutomationEffectExecution;
use codex_orchestra_core::AutomationGatePolicy;
use codex_orchestra_core::AutomationRunStart;
use codex_orchestra_core::AutomationRunStore;
use codex_orchestra_core::CommandOutcome;
use codex_orchestra_core::InheritedCodexPolicy;
use codex_orchestra_core::NativeHost;
use codex_orchestra_core::OrchestraRuntime;
use codex_orchestra_core::RunOutcome;
use codex_orchestra_core::SpawnRequest;
use codex_orchestra_core::WorktreePolicy;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;
use tempfile::tempdir;

const RAW_CHILD_DETAIL: &str = "RAW_CHILD_DETAIL_MUST_STAY_IN_THE_NATIVE_WORKFLOW_RUN";

#[test]
fn queue_blockers_preserve_order_and_bound_each_projected_identity() {
    let oversized = "é".repeat(MAX_PROJECTION_TEXT_BYTES);
    let projected = project_automation_queue_item(core::AutomationQueueProjectionItem {
        issue_id: "issue-1".into(),
        issue_identifier: "ORC-1".into(),
        issue_title: "Blocked issue".into(),
        state: "Todo".into(),
        priority: None,
        claim_id: None,
        category: core::AutomationQueueCategory::Blocked,
        next_action: "inspect blockers".into(),
        blocked_by: vec![
            core::AutomationQueueBlocker {
                id: Some(oversized),
                identifier: Some("ORC-2".into()),
                state: Some("In Progress".into()),
            },
            core::AutomationQueueBlocker {
                id: Some("blocker-2".into()),
                identifier: Some("ORC-3".into()),
                state: None,
            },
        ],
    });

    assert_eq!(projected.blocked_by.len(), 2);
    let first_id = projected.blocked_by[0].id.as_ref().unwrap();
    assert!(first_id.truncated);
    assert!(first_id.text.len() <= MAX_PROJECTION_TEXT_BYTES + '…'.len_utf8());
    assert_eq!(
        projected.blocked_by[0]
            .identifier
            .as_ref()
            .map(|text| text.text.as_str()),
        Some("ORC-2")
    );
    assert_eq!(
        projected.blocked_by[1]
            .identifier
            .as_ref()
            .map(|text| text.text.as_str()),
        Some("ORC-3")
    );
}

struct AcceptanceHost;

#[async_trait]
impl NativeHost for AcceptanceHost {
    async fn spawn(&self, request: SpawnRequest) -> Result<AgentHandle, String> {
        Ok(AgentHandle {
            thread_id: "workflow-child-42".into(),
            task_path: request.task_name,
            parent_thread_id: request.parent_thread_id,
        })
    }

    async fn status(&self, _: &AgentHandle) -> Result<AgentStatus, String> {
        Ok(AgentStatus::Running)
    }

    async fn wait(&self, _: &AgentHandle) -> Result<AgentOutcome, String> {
        Ok(AgentOutcome {
            status: AgentStatus::Completed,
            final_response: Some(
                json!({
                    "summary": RAW_CHILD_DETAIL,
                    "tracker_comment": {"body": "Issue ORC-42 implemented and verified."}
                })
                .to_string(),
            ),
        })
    }

    async fn cancel(&self, _: &AgentHandle) -> Result<(), String> {
        Ok(())
    }

    async fn run_command(
        &self,
        _: &str,
        _: &Path,
        _: &[String],
        _: Option<&Path>,
        _: u64,
    ) -> Result<CommandOutcome, String> {
        Ok(CommandOutcome {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        })
    }

    async fn create_worktree(
        &self,
        _: &str,
        repository: &Path,
        run_id: &str,
        step_id: &str,
        _: &WorktreePolicy,
        _: &str,
    ) -> Result<PathBuf, String> {
        let path = repository
            .join(".codex/orchestra/worktrees")
            .join(format!("{run_id}-{step_id}"));
        std::fs::create_dir_all(&path).map_err(|error| error.to_string())?;
        Ok(path)
    }

    async fn remove_worktree(&self, _: &str, _: &Path, path: &Path) -> Result<(), String> {
        if path.exists() {
            std::fs::remove_dir_all(path).map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    async fn request_approval(
        &self,
        _: &str,
        _: &str,
        _: &[String],
    ) -> Result<Option<String>, String> {
        Ok(None)
    }

    async fn emit_activity(&self, _: &str, _: &str) {}
}

fn initialize_repository(repository: &Path) {
    std::process::Command::new("git")
        .arg("init")
        .arg("-q")
        .arg(repository)
        .status()
        .unwrap();
    std::fs::write(repository.join("README.md"), "acceptance fixture\n").unwrap();
    assert!(
        std::process::Command::new("git")
            .arg("-C")
            .arg(repository)
            .args(["add", "."])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        std::process::Command::new("git")
            .arg("-C")
            .arg(repository)
            .args([
                "-c",
                "user.name=Orchestra Acceptance",
                "-c",
                "user.email=orchestra@example.invalid",
                "commit",
                "-q",
                "-m",
                "fixture",
            ])
            .status()
            .unwrap()
            .success()
    );
}

#[tokio::test]
async fn workflow_md_linear_fixture_reaches_bounded_desktop_projection() {
    let repository = tempdir().unwrap();
    initialize_repository(repository.path());
    let workflow_source = r#"
        import { agent, pipeline, workflow } from "@codex-orchestra/workflow";
        export default workflow({
          name: "automation-issue",
          max_parallel: 1,
          inputs: {
            issue: { type: "object" },
            task_prompt: { type: "string" },
            automation: { type: "object" },
          },
          steps: [pipeline([agent({
            id: "implement",
            prompt: "{{inputs.task_prompt}}",
            model: "gpt-5.4",
            outputs: ["summary", "tracker_comment"],
            write_scope: ["."],
          })])],
        });
    "#;
    std::fs::write(repository.path().join("issue.workflow.ts"), workflow_source).unwrap();
    std::fs::write(
        repository.path().join("WORKFLOW.md"),
        r#"---
tracker:
  kind: linear
  project_slug: orchestra
  api_key: $LINEAR_API_KEY
  required_labels: [automation]
  active_states: [Todo, In Progress]
  terminal_states: [Done, Cancelled]
workspace:
  root: .codex/orchestra/automation-worktrees
agent:
  max_concurrent_agents: 1
orchestra:
  workflow: issue.workflow.ts
  effects: [tracker.comment]
---
Implement {{ issue.identifier }}: {{ issue.title }}
"#,
    )
    .unwrap();

    let linear_fixture = json!({"data": {"issue": {
        "id": "linear-42",
        "identifier": "ORC-42",
        "title": "Prove the Automation acceptance path",
        "description": "Use the native Issue task and typed Workflow.",
        "priority": 1,
        "state": {"name": "Todo"},
        "branchName": "orc-42-acceptance",
        "url": "https://linear.app/orchestra/issue/ORC-42",
        "labels": {"nodes": [{"name": "automation"}]},
        "relations": {"nodes": []},
        "inverseRelations": {"nodes": []},
        "createdAt": "2026-07-17T00:00:00.000Z",
        "updatedAt": "2026-07-17T00:00:00.000Z"
    }}});
    let issue = core::normalize_linear_issue(&linear_fixture).unwrap();
    let validation = core::validate_automation_profile(core::AutomationValidationRequest {
        workflow_md_path: repository.path().join("WORKFLOW.md"),
        repository_root: repository.path().to_path_buf(),
        fixture_issue: issue.clone(),
        attempt: Some(1),
        environment: BTreeMap::new(),
        home_dir: None,
        inherited_policy: InheritedCodexPolicy::default(),
    });
    assert!(validation.valid, "{:?}", validation.diagnostics);
    assert!(validation.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == core::AutomationDiagnosticCode::MissingSecret
            && diagnostic.severity == core::AutomationValidationSeverity::Warning
    }));
    let profile = validation.profile.unwrap();
    let profile_digest = validation.profile_digest.unwrap();
    let plan = core::compile_workflow(workflow_source).unwrap();

    let (store, mut root) = AutomationRunStore::start(AutomationRunStart {
        repository: repository.path(),
        owner_thread_id: "automation-task-42",
        source_revision: "fixture-revision-42",
        profile: &profile,
        profile_digest: &profile_digest,
    })
    .unwrap();
    let task_prompt = validation
        .preview
        .as_ref()
        .and_then(|preview| preview.rendered_prompt.clone())
        .unwrap();
    let coordination_plan = core::plan_coordination_page(
        &root,
        &profile,
        core::AutomationCoordinationPage::Ready {
            expected_scan_revision: 0,
            input_cursor: None,
            output_cursor: None,
            issues: std::slice::from_ref(&issue),
        },
        1,
        40,
    )
    .unwrap()
    .with_task_prompt(&task_prompt);
    let intent = store
        .commit_coordination_plan(&mut root, &profile, coordination_plan)
        .unwrap();
    let intent = intent.dispatch_intent.unwrap();
    let claim_id = intent.claim_id.clone();
    store
        .start_dispatch_intent(&mut root, &intent.intent_id, true, 41)
        .unwrap();
    let issue_task = AgentHandle {
        thread_id: "issue-task-42".into(),
        task_path: "automation_orc_42".into(),
        parent_thread_id: "automation-task-42".into(),
    };
    let issue_worktree = root.claims[&claim_id].worktree.clone();
    std::fs::create_dir_all(&issue_worktree).unwrap();
    initialize_repository(&issue_worktree);
    store
        .update_claim(&mut root, &claim_id, |claim| {
            claim.issue_task = Some(issue_task.clone());
            claim.status = core::AutomationClaimStatus::Running;
        })
        .unwrap();
    let first_steering = store
        .prepare_issue_steering(
            &mut root,
            &claim_id,
            "automation-task-42",
            "FIRST_STEERING_MUST_NOT_REACH_THE_BOUNDED_PROJECTION",
            40,
        )
        .unwrap();
    store
        .complete_issue_steering(
            &mut root,
            &claim_id,
            first_steering.sequence,
            Ok("submission-1"),
        )
        .unwrap();
    let latest_steering = store
        .prepare_issue_steering(
            &mut root,
            &claim_id,
            "automation-task-42",
            "Focus on the recovery test.",
            42,
        )
        .unwrap();
    store
        .complete_issue_steering(
            &mut root,
            &claim_id,
            latest_steering.sequence,
            Ok("submission-2"),
        )
        .unwrap();

    let workflow = OrchestraRuntime::new(AcceptanceHost)
        .run_with_inputs(
            &issue_worktree,
            &issue_task.thread_id,
            plan,
            Some(&json!({
                "issue": issue,
                "task_prompt": root.claims[&claim_id].task_prompt,
                "automation": {
                    "root_run_id": root.run_id,
                    "claim_id": claim_id,
                    "attempt": 1
                }
            })),
        )
        .await
        .unwrap();
    let workflow_checkpoint = match workflow {
        RunOutcome::Completed(checkpoint) => checkpoint,
        outcome => panic!("unexpected workflow outcome: {outcome:?}"),
    };
    let tracker_body = workflow_checkpoint.steps["implement"].outputs["tracker_comment"]["body"]
        .as_str()
        .unwrap()
        .to_owned();
    store
        .update_claim(&mut root, &claim_id, |claim| {
            claim.workflow_run_id = Some(workflow_checkpoint.run_id.clone());
            claim.workflow_status = Some(workflow_checkpoint.status.clone());
        })
        .unwrap();
    let receipt = store
        .resolve_tracker_comment(
            &mut root,
            &claim_id,
            &profile,
            &tracker_body,
            AutomationGatePolicy::AutoAccept,
            |_| AutomationEffectExecution::Committed {
                provider_receipt: "linear-comment-42".into(),
            },
        )
        .unwrap();
    assert_eq!(receipt.status, core::AutomationEffectStatus::Committed);
    store
        .record_completed_invocation(&mut root, &claim_id, &profile, false, 42)
        .unwrap();
    store
        .complete_dispatch_intent(&mut root, &intent.intent_id)
        .unwrap();

    let mut focused_root = root.clone();
    let seed_claim = focused_root.claims[&claim_id].clone();
    for index in 0..25 {
        let mut filler = seed_claim.clone();
        filler.claim_id = format!("{index:05}");
        filler.issue_id = format!("filler-issue-{index:02}");
        filler.issue_identifier = format!("ORC-FILLER-{index:02}");
        focused_root.claims.insert(filler.claim_id.clone(), filler);
    }
    let projection = project_automation_run(root, None);
    let claim = &projection.claims[0];
    assert_eq!(projection.owner_thread_id, "automation-task-42");
    assert_eq!(projection.coordination.scan_revision, 1);
    assert_eq!(
        projection
            .coordination
            .dispatch_intent
            .as_ref()
            .unwrap()
            .status,
        protocol::AutomationDispatchIntentStatus::Completed
    );
    assert_eq!(claim.workflow_invocations, 1);
    assert_eq!(
        claim.issue_task.as_ref().unwrap().thread_id,
        "issue-task-42"
    );
    assert_eq!(
        claim.workflow_run_id.as_deref(),
        Some(workflow_checkpoint.run_id.as_str())
    );
    assert_eq!(
        claim.effects[0].provider_receipt.as_deref(),
        Some("linear-comment-42")
    );
    let steering = claim.latest_steering_receipt.as_ref().unwrap();
    assert_eq!(steering.sequence, 2);
    assert_eq!(steering.input_preview, "Focus on the recovery test.");
    assert_eq!(steering.provider_receipt.as_deref(), Some("submission-2"));
    assert!(claim.worktree.contains("orc-42"));
    let desktop_payload = serde_json::to_string(&projection).unwrap();
    assert!(!desktop_payload.contains(RAW_CHILD_DETAIL));
    assert!(!desktop_payload.contains("FIRST_STEERING_MUST_NOT_REACH"));
    assert!(!desktop_payload.contains("tracker_comment"));

    let unfocused = project_automation_run(focused_root.clone(), None);
    assert_eq!(unfocused.claims_total, 26);
    assert_eq!(unfocused.claims.len(), 25);
    assert!(
        unfocused
            .claims
            .iter()
            .all(|claim| claim.issue_id != "linear-42")
    );
    let missing_focus = project_automation_run(
        focused_root.clone(),
        Some(AutomationClaimFocus::IssueId("missing-issue")),
    );
    assert_eq!(missing_focus.claims, unfocused.claims);
    let focused = project_automation_run(
        focused_root.clone(),
        Some(AutomationClaimFocus::IssueId("linear-42")),
    );
    assert_eq!(focused.claims.len(), 25);
    assert_eq!(
        focused
            .claims
            .iter()
            .filter(|claim| claim.issue_id == "linear-42")
            .count(),
        1
    );
    assert_eq!(
        focused
            .claims
            .iter()
            .find(|claim| claim.issue_id == "linear-42")
            .and_then(|claim| claim.issue_url.as_deref()),
        Some("https://linear.app/orchestra/issue/ORC-42")
    );

    let steering_receipt = focused_root.claims[&claim_id]
        .steering_receipts
        .iter()
        .max_by_key(|receipt| receipt.sequence)
        .cloned()
        .unwrap();
    let response =
        project_automation_steer_issue_response(focused_root, steering_receipt, &claim_id);
    let steered = response.run;
    assert_eq!(steered.claims.len(), 25);
    assert_eq!(
        steered
            .claims
            .iter()
            .filter(|claim| claim.claim_id == claim_id)
            .count(),
        1
    );
    assert_eq!(
        steered
            .claims
            .iter()
            .find(|claim| claim.claim_id == claim_id)
            .and_then(|claim| claim.issue_url.as_deref()),
        Some("https://linear.app/orchestra/issue/ORC-42")
    );
    assert_eq!(response.receipt.sequence, 2);
    assert_eq!(
        response.receipt.input_preview,
        "Focus on the recovery test."
    );
}
