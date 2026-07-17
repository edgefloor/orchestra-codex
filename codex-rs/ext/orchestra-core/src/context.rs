use crate::ContextSource;
use crate::RunInputs;
use crate::StepOutputs;
use crate::inputs::render_value;
use serde::Deserialize;
use serde::Serialize;
use sha2::Digest;
use sha2::Sha256;
use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use thiserror::Error;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ContextBundle {
    pub sha256: String,
    pub content: String,
    pub sources: Vec<String>,
}

#[derive(Debug, Error)]
pub enum ContextError {
    #[error("context path escapes repository: {0}")]
    PathEscape(String),
    #[error("failed to read context: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid line range for {path}: {start}..{end}")]
    Range {
        path: String,
        start: usize,
        end: usize,
    },
    #[error("git context command failed: {0}")]
    Git(String),
    #[error("missing dependency output {step}.{output}")]
    MissingOutput { step: String, output: String },
    #[error("missing run input {0}")]
    MissingInput(String),
}

pub fn materialize_context(
    repository: &Path,
    sources: &[ContextSource],
    dependency_outputs: &BTreeMap<String, StepOutputs>,
) -> Result<ContextBundle, ContextError> {
    materialize_context_with_inputs(repository, sources, dependency_outputs, &RunInputs::new())
}

pub fn materialize_context_with_inputs(
    repository: &Path,
    sources: &[ContextSource],
    dependency_outputs: &BTreeMap<String, StepOutputs>,
    inputs: &RunInputs,
) -> Result<ContextBundle, ContextError> {
    let root = repository.canonicalize()?;
    let mut content = String::new();
    let mut labels = Vec::new();
    for source in sources {
        let (label, body) = match source {
            ContextSource::File { path } => {
                let file = safe_path(&root, path)?;
                (format!("file:{path}"), std::fs::read_to_string(file)?)
            }
            ContextSource::Range { path, start, end } => {
                let file = safe_path(&root, path)?;
                let text = std::fs::read_to_string(file)?;
                let lines: Vec<_> = text.lines().collect();
                if *start == 0 || start > end || *end > lines.len() {
                    return Err(ContextError::Range {
                        path: path.clone(),
                        start: *start,
                        end: *end,
                    });
                }
                (
                    format!("range:{path}:{start}-{end}"),
                    lines[start - 1..*end].join("\n") + "\n",
                )
            }
            ContextSource::Diff { from, to, paths } => {
                let mut args = vec![
                    "diff".into(),
                    "--no-ext-diff".into(),
                    "--binary".into(),
                    from.clone(),
                    to.clone(),
                    "--".into(),
                ];
                args.extend(paths.iter().cloned());
                (
                    format!("diff:{from}..{to}:{}", paths.join(",")),
                    git(&root, &args)?,
                )
            }
            ContextSource::Revision { revision, path } => {
                if path.starts_with('-') || revision.starts_with('-') {
                    return Err(ContextError::Git(
                        "revision and path must not start with `-`".into(),
                    ));
                }
                (
                    format!("revision:{revision}:{path}"),
                    git(&root, &["show".into(), format!("{revision}:{path}")])?,
                )
            }
            ContextSource::DependencyOutput { step, output } => {
                let value = dependency_outputs
                    .get(step)
                    .and_then(|values| values.get(output))
                    .ok_or_else(|| ContextError::MissingOutput {
                        step: step.clone(),
                        output: output.clone(),
                    })?;
                (
                    format!("output:{step}.{output}"),
                    serde_json::to_string_pretty(value).expect("JSON value serializes"),
                )
            }
            ContextSource::Input { input } => {
                let value = inputs
                    .get(input)
                    .ok_or_else(|| ContextError::MissingInput(input.clone()))?;
                (format!("input:{input}"), render_value(value))
            }
        };
        labels.push(label.clone());
        content.push_str(&format!("<<< ORCHESTRA CONTEXT {label} >>>\n{body}"));
        if !content.ends_with('\n') {
            content.push('\n');
        }
        content.push_str(&format!("<<< END ORCHESTRA CONTEXT {label} >>>\n"));
    }
    let sha256 = format!("{:x}", Sha256::digest(content.as_bytes()));
    Ok(ContextBundle {
        sha256,
        content,
        sources: labels,
    })
}

fn safe_path(root: &Path, relative: &str) -> Result<PathBuf, ContextError> {
    let candidate = root.join(relative);
    let canonical = candidate.canonicalize()?;
    if !canonical.starts_with(root) {
        return Err(ContextError::PathEscape(relative.into()));
    }
    Ok(canonical)
}

fn git(repository: &Path, args: &[String]) -> Result<String, ContextError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(args)
        .output()?;
    if !output.status.success() {
        return Err(ContextError::Git(
            String::from_utf8_lossy(&output.stderr).trim().into(),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use tempfile::tempdir;

    #[test]
    fn ranges_and_dependency_outputs_are_exact_and_hashed() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "one\ntwo\nthree\n").unwrap();
        let mut outputs = BTreeMap::new();
        outputs.insert(
            "plan".into(),
            BTreeMap::from([("answer".into(), serde_json::json!(42))]),
        );
        let sources = vec![
            ContextSource::Range {
                path: "a.txt".into(),
                start: 2,
                end: 3,
            },
            ContextSource::DependencyOutput {
                step: "plan".into(),
                output: "answer".into(),
            },
        ];
        let one = materialize_context(dir.path(), &sources, &outputs).unwrap();
        let two = materialize_context(dir.path(), &sources, &outputs).unwrap();
        assert_eq!(one.sha256, two.sha256);
        assert!(one.content.contains("two\nthree"));
        assert!(!one.content.contains("one\n"));
    }

    #[test]
    fn run_inputs_are_materialized_as_exact_context() {
        let dir = tempdir().unwrap();
        let inputs = RunInputs::from([
            ("ticket".into(), Value::String("#3".into())),
            ("metadata".into(), serde_json::json!({"kind":"issue"})),
        ]);
        let context = materialize_context_with_inputs(
            dir.path(),
            &[
                ContextSource::Input {
                    input: "ticket".into(),
                },
                ContextSource::Input {
                    input: "metadata".into(),
                },
            ],
            &BTreeMap::new(),
            &inputs,
        )
        .unwrap();
        assert!(
            context
                .content
                .contains("<<< ORCHESTRA CONTEXT input:ticket >>>\n#3")
        );
        assert!(context.content.contains(r#"{"kind":"issue"}"#));
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_cannot_escape_repository() {
        use std::os::unix::fs::symlink;
        let dir = tempdir().unwrap();
        symlink("/etc/passwd", dir.path().join("escape")).unwrap();
        assert!(matches!(
            materialize_context(
                dir.path(),
                &[ContextSource::File {
                    path: "escape".into()
                }],
                &BTreeMap::new()
            ),
            Err(ContextError::PathEscape(_))
        ));
    }
}
