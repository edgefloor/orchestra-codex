use anyhow::Context;
use anyhow::Result;
use app_test_support::TestAppServer;
use app_test_support::to_response;
use codex_app_server_protocol::AutomationDispatchIntentStatus;
use codex_app_server_protocol::AutomationIssue as ProtocolAutomationIssue;
use codex_app_server_protocol::AutomationReconcileParams;
use codex_app_server_protocol::AutomationRunResponse;
use codex_app_server_protocol::AutomationStartParams;
use codex_app_server_protocol::AutomationStatusParams;
use codex_app_server_protocol::AutomationValidateParams;
use codex_app_server_protocol::AutomationValidateResponse;
use codex_app_server_protocol::JSONRPCResponse;
use codex_app_server_protocol::RequestId;
use codex_app_server_protocol::ThreadStartParams;
use codex_app_server_protocol::ThreadStartResponse;
use codex_orchestra_core::AutomationCoordinationPage;
use codex_orchestra_core::AutomationIssue;
use codex_orchestra_core::AutomationProfile;
use codex_orchestra_core::AutomationRunStart;
use codex_orchestra_core::AutomationRunStore;
use codex_orchestra_core::plan_coordination_page;
use core_test_support::skip_if_remote;
use pretty_assertions::assert_eq;
use std::fs;
use std::process::Command;
use std::time::Duration;
use tempfile::TempDir;
use tokio::time::Instant;
use tokio::time::sleep;
use tokio::time::timeout;

const DEFAULT_READ_TIMEOUT: Duration = Duration::from_secs(10);
const MISSING_LINEAR_KEY: &str = "ORCHESTRA_TEST_MISSING_LINEAR_KEY";
const EXACT_TRACKER_URL: &str =
    "https://linear.app/orchestra/issue/ORC-CYCLE-5/non-derivable-slug?source=tracker";

fn workflow_document() -> String {
    format!(
        r#"---
tracker:
  kind: linear
  api_key: ${MISSING_LINEAR_KEY}
  project_slug: orchestra
  required_labels: [automation]
  active_states: [Todo]
  terminal_states: [Done, Cancelled]
polling:
  interval_ms: 15000
workspace:
  root: .codex/orchestra/worktrees
agent:
  max_concurrent_agents: 1
  max_turns: 3
codex:
  approval_policy: untrusted
  thread_sandbox: read-only
orchestra:
  workflow: issue.workflow.ts
  effects:
    - tracker.comment
---
Work on {{{{ issue.identifier }}}}: {{{{ issue.title }}}}.
"#
    )
}

fn workflow_source() -> &'static str {
    r#"import { workflow, agent } from "@codex-orchestra/workflow";
export default workflow({
  name: "automation-issue",
  inputs: {
    issue: { type: "object" },
    task_prompt: { type: "string" },
    automation: { type: "object" }
  },
  steps: [agent({ id: "implement", prompt: "Implement the issue", model: "gpt-5.4" })]
});"#
}

fn fixture_issue() -> ProtocolAutomationIssue {
    ProtocolAutomationIssue {
        id: "linear-cycle-5".into(),
        identifier: "ORC-CYCLE-5".into(),
        title: "Retain one failed background dispatch".into(),
        description: Some("App Server acceptance fixture".into()),
        priority: Some(1),
        state: "Todo".into(),
        branch_name: None,
        url: Some(EXACT_TRACKER_URL.into()),
        labels: vec!["automation".into()],
        blocked_by: Vec::new(),
        created_at: Some("2026-07-18T00:00:00.000Z".into()),
        updated_at: Some("2026-07-18T00:00:00.000Z".into()),
    }
}

async fn rpc<T: serde::de::DeserializeOwned>(
    app_server: &mut TestAppServer,
    method: &str,
    params: impl serde::Serialize,
) -> Result<T> {
    let request_id = app_server
        .send_raw_request(method, Some(serde_json::to_value(params)?))
        .await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        app_server.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    to_response(response)
}

#[tokio::test]
async fn automation_start_and_refresh_project_one_durable_missing_credential_failure() -> Result<()>
{
    skip_if_remote!(
        Ok(()),
        "Orchestra runtime state is intentionally repository-local"
    );
    let codex_home = TempDir::new()?;
    let mut app_server = TestAppServer::builder()
        .with_codex_home(codex_home.path())
        .with_env_overrides(&[(MISSING_LINEAR_KEY, None)])
        .build()
        .await?;
    timeout(DEFAULT_READ_TIMEOUT, app_server.initialize()).await??;

    let repository = app_server.auto_env()?.selection().cwd.to_path_buf();
    fs::write(repository.join("WORKFLOW.md"), workflow_document())?;
    fs::write(repository.join("issue.workflow.ts"), workflow_source())?;
    let git_init = Command::new("git")
        .arg("init")
        .arg("--quiet")
        .arg(&repository)
        .output()?;
    anyhow::ensure!(
        git_init.status.success(),
        "git init failed: {}",
        String::from_utf8_lossy(&git_init.stderr)
    );

    let environment = app_server.auto_env_params()?;
    let thread: ThreadStartResponse = rpc(
        &mut app_server,
        "thread/start",
        ThreadStartParams {
            cwd: Some(repository.display().to_string()),
            environments: Some(vec![environment]),
            ..Default::default()
        },
    )
    .await?;
    let issue = fixture_issue();
    let validation: AutomationValidateResponse = rpc(
        &mut app_server,
        "automation/validate",
        AutomationValidateParams {
            thread_id: thread.thread.id.clone(),
            profile_path: "WORKFLOW.md".into(),
            fixture_issue: issue.clone(),
            attempt: Some(1),
        },
    )
    .await?;
    anyhow::ensure!(
        validation.valid,
        "fixture profile did not validate: {:#?}",
        validation.diagnostics
    );
    let profile: AutomationProfile = serde_json::from_value(serde_json::to_value(
        validation
            .profile
            .context("valid profile projection is missing")?,
    )?)?;
    let profile_digest = validation
        .profile_digest
        .context("valid profile digest is missing")?;
    let core_issue = AutomationIssue {
        id: "linear-cycle-5".into(),
        identifier: "ORC-CYCLE-5".into(),
        title: "Retain one failed background dispatch".into(),
        description: Some("App Server acceptance fixture".into()),
        priority: Some(1),
        state: "Todo".into(),
        branch_name: None,
        url: Some(EXACT_TRACKER_URL.into()),
        labels: vec!["automation".into()],
        blocked_by: Vec::new(),
        created_at: Some("2026-07-18T00:00:00.000Z".into()),
        updated_at: Some("2026-07-18T00:00:00.000Z".into()),
    };

    let (store, mut root) = AutomationRunStore::start(AutomationRunStart {
        repository: &repository,
        owner_thread_id: &thread.thread.id,
        source_revision: "app-server-acceptance",
        profile: &profile,
        profile_digest: &profile_digest,
    })?;
    let plan = plan_coordination_page(
        &root,
        &profile,
        AutomationCoordinationPage::Ready {
            expected_scan_revision: 0,
            input_cursor: None,
            output_cursor: None,
            issues: std::slice::from_ref(&core_issue),
        },
        1,
        50_000,
    )?;
    let intent = store
        .commit_coordination_plan(&mut root, &profile, plan)?
        .dispatch_intent
        .context("coordination plan did not create a dispatch intent")?;
    let focused_issue_id = core_issue.id.clone();
    let seed_claim = root.claims[&intent.claim_id].clone();
    for index in 0..25 {
        let mut filler = seed_claim.clone();
        filler.claim_id = format!("{index:05}");
        filler.issue_id = format!("filler-issue-{index:02}");
        filler.issue_identifier = format!("ORC-FILLER-{index:02}");
        filler.issue_url = Some(format!(
            "https://linear.app/orchestra/issue/ORC-FILLER-{index:02}"
        ));
        root.claims.insert(filler.claim_id.clone(), filler);
    }
    store.save(&mut root)?;
    let durable = store.load()?;
    assert_eq!(
        durable.claims[&intent.claim_id].issue_url.as_deref(),
        Some(EXACT_TRACKER_URL)
    );

    let started: AutomationRunResponse = rpc(
        &mut app_server,
        "automation/start",
        AutomationStartParams {
            thread_id: thread.thread.id.clone(),
            profile_path: "WORKFLOW.md".into(),
        },
    )
    .await?;
    let started_intent = started
        .run
        .coordination
        .dispatch_intent
        .as_ref()
        .context("automation/start omitted the outstanding intent")?;
    assert_eq!(started.run.run_id, root.run_id);
    assert_eq!(started.run.claims_total, 26);
    assert_eq!(started.run.claims.len(), 25);
    assert!(
        started
            .run
            .claims
            .iter()
            .all(|claim| claim.issue_url.as_deref() != Some(EXACT_TRACKER_URL))
    );
    assert!(
        started
            .run
            .claims
            .iter()
            .all(|claim| claim.issue_id != focused_issue_id)
    );
    assert_eq!(started_intent.intent_id, intent.intent_id);
    assert_eq!(started_intent.claim_id, intent.claim_id);
    assert_eq!(
        started_intent.status,
        AutomationDispatchIntentStatus::Pending
    );

    let deadline = Instant::now() + DEFAULT_READ_TIMEOUT;
    let recovered = loop {
        let status: AutomationRunResponse = rpc(
            &mut app_server,
            "automation/status",
            AutomationStatusParams {
                thread_id: thread.thread.id.clone(),
                run_id: root.run_id.clone(),
                focused_issue_id: Some(focused_issue_id.clone()),
            },
        )
        .await?;
        if status.run.coordination.error.is_some() {
            break status.run;
        }
        anyhow::ensure!(
            Instant::now() < deadline,
            "background dispatch failure was not projected before the deadline"
        );
        sleep(Duration::from_millis(20)).await;
    };

    let recovered_intent = recovered
        .coordination
        .dispatch_intent
        .as_ref()
        .context("durable error displaced the outstanding intent")?;
    assert_eq!(recovered.run_id, started.run.run_id);
    assert_eq!(recovered.claims_total, 26);
    assert_eq!(recovered.claims.len(), 25);
    let focused_claims = recovered
        .claims
        .iter()
        .filter(|claim| claim.issue_id == focused_issue_id)
        .collect::<Vec<_>>();
    assert_eq!(focused_claims.len(), 1);
    assert_eq!(focused_claims[0].claim_id, intent.claim_id);
    assert_eq!(
        focused_claims[0].issue_url.as_deref(),
        Some(EXACT_TRACKER_URL)
    );
    assert_eq!(recovered_intent.intent_id, intent.intent_id);
    assert_eq!(recovered_intent.claim_id, intent.claim_id);
    assert_eq!(
        recovered_intent.status,
        AutomationDispatchIntentStatus::Pending
    );
    assert_eq!(
        recovered
            .coordination
            .error
            .as_ref()
            .map(|error| error.text.as_str()),
        Some("re-resolve the referenced Linear credential, then retry")
    );
    let mut durable = store.load()?;
    assert_eq!(
        durable.coordination.error.as_deref(),
        Some("re-resolve the referenced Linear credential, then retry")
    );
    assert_eq!(durable.coordination.dispatch_intent.as_ref(), Some(&intent));
    assert!(durable.claims.contains_key(&intent.claim_id));
    durable
        .claims
        .retain(|claim_id, _claim| claim_id == &intent.claim_id);
    store.save(&mut durable)?;

    let refreshed: AutomationRunResponse = rpc(
        &mut app_server,
        "automation/refresh",
        AutomationReconcileParams {
            thread_id: thread.thread.id,
            run_id: recovered.run_id.clone(),
            profile_path: "WORKFLOW.md".into(),
        },
    )
    .await?;
    assert_eq!(refreshed.run.run_id, recovered.run_id);
    assert_eq!(refreshed.run.claims_total, 1);
    assert_eq!(refreshed.run.claims.len(), 1);
    assert_eq!(
        refreshed
            .run
            .claims
            .first()
            .map(|claim| (&claim.claim_id, claim.issue_url.as_deref())),
        Some((&intent.claim_id, Some(EXACT_TRACKER_URL)))
    );
    assert_eq!(
        refreshed
            .run
            .coordination
            .dispatch_intent
            .as_ref()
            .map(|intent| (&intent.intent_id, &intent.claim_id)),
        Some((&intent.intent_id, &intent.claim_id))
    );
    let run_directories = fs::read_dir(repository.join(".codex/orchestra/runs"))?
        .collect::<std::io::Result<Vec<_>>>()?;
    assert_eq!(run_directories.len(), 1);

    Ok(())
}
