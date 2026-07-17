use codex_orchestra_core::Evaluator;
use codex_orchestra_core::EvaluatorFailure;
use codex_orchestra_core::EvaluatorLimits;
use codex_orchestra_core::ValidationOutcome;
use codex_orchestra_core::ValidationRequest;
use serde_json::Value;
use serde_json::json;
use sha2::Digest;
use sha2::Sha256;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use tempfile::tempdir;

const REVISION: &str = "bun-1.3.14-zod-4.4.3-mvp-1";

fn worker() -> PathBuf {
    std::env::var_os("ORCHESTRA_EVALUATOR_BIN")
        .map(PathBuf::from)
        .expect("ORCHESTRA_EVALUATOR_BIN must point to the compiled Product worker")
}

fn hash(source: &str) -> String {
    format!("{:x}", Sha256::digest(source.as_bytes()))
}

fn request(source: &str, schema_id: &str, value: Value) -> ValidationRequest {
    ValidationRequest {
        bundle_source: source.into(),
        bundle_hash: hash(source),
        schema_id: schema_id.into(),
        value,
    }
}

#[tokio::test]
#[ignore = "run through scripts/evaluator-test.sh with the pinned Product worker"]
async fn exact_zod_transform_refine_is_deterministic_across_fresh_workers() {
    let source = r#"({
      output: z.object({
        count: z.number().int().min(0).transform((value) => value + 1),
        label: z.string().trim().min(1)
      }).refine((value) => value.count < 10, { path: ["count"], message: "count is too large" })
    })"#;
    let evaluator = Evaluator::new(worker(), REVISION, EvaluatorLimits::default());
    let mut outcomes = Vec::new();
    for _ in 0..5 {
        outcomes.push(
            evaluator
                .validate(request(
                    source,
                    "output",
                    json!({"label": " result ", "count": 2}),
                ))
                .await
                .unwrap(),
        );
    }
    assert!(outcomes.windows(2).all(|pair| pair[0] == pair[1]));
    let ValidationOutcome::Accepted {
        transformed_canonical,
        ..
    } = &outcomes[0]
    else {
        panic!("expected accepted transform")
    };
    assert_eq!(transformed_canonical, r#"{"count":3,"label":"result"}"#);
}

#[tokio::test]
#[ignore = "run through scripts/evaluator-test.sh with the pinned Product worker"]
async fn ordinary_rejection_is_stable_sorted_and_bounded() {
    let source = r#"({ output: z.object({ z: z.string().min(2), a: z.number().min(0) }) })"#;
    let evaluator = Evaluator::new(worker(), REVISION, EvaluatorLimits::default());
    let outcome = evaluator
        .validate(request(source, "output", json!({"z": "", "a": -1})))
        .await
        .unwrap();
    let ValidationOutcome::Rejected { issues, .. } = outcome else {
        panic!("expected ordinary schema rejection")
    };
    assert_eq!(issues.len(), 2);
    assert_eq!(issues[0].path[0].to_string(), "a");
    assert_eq!(issues[1].path[0].to_string(), "z");
}

#[tokio::test]
#[ignore = "run through scripts/evaluator-test.sh with the pinned Product worker"]
async fn async_and_noncanonical_transforms_are_infrastructure_failures() {
    let evaluator = Evaluator::new(worker(), REVISION, EvaluatorLimits::default());
    let async_source = "({ output: z.string().refine(async () => true) })";
    let async_error = evaluator
        .validate(request(async_source, "output", json!("ok")))
        .await
        .unwrap_err();
    assert_eq!(async_error.kind, EvaluatorFailure::WorkerFailure);

    let noncanonical = "({ output: z.string().transform(() => undefined) })";
    let noncanonical_error = evaluator
        .validate(request(noncanonical, "output", json!("ok")))
        .await
        .unwrap_err();
    assert_eq!(noncanonical_error.kind, EvaluatorFailure::WorkerFailure);
}

#[tokio::test]
#[ignore = "run through scripts/evaluator-test.sh with the pinned Product worker"]
async fn evaluator_revision_is_intrinsic_worker_provenance() {
    let source = "({ output: z.string() })";
    let error = Evaluator::new(
        worker(),
        "different-evaluator-revision",
        EvaluatorLimits::default(),
    )
    .validate(request(source, "output", json!("ok")))
    .await
    .unwrap_err();
    assert_eq!(error.kind, EvaluatorFailure::WorkerFailure);
}

#[tokio::test]
#[ignore = "run through scripts/evaluator-test.sh with the pinned Product worker"]
async fn oversized_timeout_and_crash_have_distinct_failures() {
    let source = "({ output: z.string() })";
    let small = EvaluatorLimits {
        canonical_value_bytes: 8,
        ..EvaluatorLimits::default()
    };
    let oversized = Evaluator::new(worker(), REVISION, small)
        .validate(request(source, "output", json!("too long for limit")))
        .await
        .unwrap_err();
    assert_eq!(oversized.kind, EvaluatorFailure::ValueTooLarge);

    let burn = "({ output: z.string().refine(() => { for (;;) {} }) })";
    let brief = EvaluatorLimits {
        wall_time_ms: 100,
        ..EvaluatorLimits::default()
    };
    let timeout = Evaluator::new(worker(), REVISION, brief)
        .validate(request(burn, "output", json!("ok")))
        .await
        .unwrap_err();
    assert_eq!(timeout.kind, EvaluatorFailure::Timeout);

    let temp = tempdir().unwrap();
    let crashing_worker = temp.path().join("crash-worker");
    fs::write(&crashing_worker, "#!/bin/sh\nkill -KILL $$\n").unwrap();
    let mut permissions = fs::metadata(&crashing_worker).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&crashing_worker, permissions).unwrap();
    let crash_limits = EvaluatorLimits {
        // This assertion distinguishes a terminated process from a timeout. Give the
        // shell enough startup margin when the ignored product suite runs in parallel.
        wall_time_ms: 5_000,
        ..EvaluatorLimits::default()
    };
    let crash = Evaluator::new(crashing_worker, REVISION, crash_limits)
        .validate(request(source, "output", json!("ok")))
        .await
        .unwrap_err();
    assert_eq!(crash.kind, EvaluatorFailure::Crash);
}

trait SegmentDisplay {
    fn to_string(&self) -> String;
}

impl SegmentDisplay for codex_orchestra_core::ValuePathSegment {
    fn to_string(&self) -> String {
        match self {
            Self::Key(value) => value.clone(),
            Self::Index(value) => value.to_string(),
        }
    }
}
