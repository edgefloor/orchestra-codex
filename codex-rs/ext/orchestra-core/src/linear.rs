use crate::AutomationIssue;
use crate::AutomationIssueBlocker;
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeSet;
use thiserror::Error;

#[derive(Clone, Debug, PartialEq)]
pub struct LinearIssuePage {
    pub issues: Vec<AutomationIssue>,
    pub has_next_page: bool,
    pub end_cursor: Option<String>,
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum LinearReadError {
    #[error("Linear returned GraphQL errors: {0}")]
    GraphQl(String),
    #[error("Linear response did not contain the expected issue data")]
    MissingData,
    #[error("Linear response could not be normalized: {0}")]
    InvalidData(String),
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawIssue {
    id: String,
    identifier: String,
    title: String,
    description: Option<String>,
    priority: Option<i64>,
    state: RawState,
    branch_name: Option<String>,
    url: Option<String>,
    labels: Option<RawConnection<RawLabel>>,
    relations: Option<RawConnection<RawRelation>>,
    inverse_relations: Option<RawConnection<RawRelation>>,
    created_at: Option<String>,
    updated_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawState {
    name: String,
}

#[derive(Debug, Deserialize)]
struct RawLabel {
    name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawRelation {
    #[serde(rename = "type")]
    kind: String,
    related_issue: Option<RawBlocker>,
    issue: Option<RawBlocker>,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd)]
struct RawBlocker {
    id: Option<String>,
    identifier: Option<String>,
    state: Option<RawStateName>,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd)]
struct RawStateName {
    name: String,
}

#[derive(Debug, Deserialize)]
struct RawConnection<T> {
    nodes: Vec<T>,
}

impl<T> Default for RawConnection<T> {
    fn default() -> Self {
        Self { nodes: Vec::new() }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawPageInfo {
    has_next_page: bool,
    end_cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawIssuePage {
    nodes: Vec<RawIssue>,
    page_info: RawPageInfo,
}

pub fn normalize_linear_issue_page(response: &Value) -> Result<LinearIssuePage, LinearReadError> {
    reject_graphql_errors(response)?;
    let connection = response
        .pointer("/data/project/issues")
        .or_else(|| response.pointer("/data/issues"))
        .ok_or(LinearReadError::MissingData)?;
    let raw: RawIssuePage = serde_json::from_value(connection.clone())
        .map_err(|error| LinearReadError::InvalidData(error.to_string()))?;
    let mut issues = raw
        .nodes
        .into_iter()
        .map(normalize_issue)
        .collect::<Result<Vec<_>, _>>()?;
    issues.sort_by(|left, right| left.identifier.cmp(&right.identifier));
    Ok(LinearIssuePage {
        issues,
        has_next_page: raw.page_info.has_next_page,
        end_cursor: raw.page_info.end_cursor.filter(|value| !value.is_empty()),
    })
}

pub fn normalize_linear_issue(response: &Value) -> Result<AutomationIssue, LinearReadError> {
    reject_graphql_errors(response)?;
    let issue = response
        .pointer("/data/issue")
        .ok_or(LinearReadError::MissingData)?;
    let raw: RawIssue = serde_json::from_value(issue.clone())
        .map_err(|error| LinearReadError::InvalidData(error.to_string()))?;
    normalize_issue(raw)
}

fn reject_graphql_errors(response: &Value) -> Result<(), LinearReadError> {
    let Some(errors) = response.get("errors").and_then(Value::as_array) else {
        return Ok(());
    };
    if errors.is_empty() {
        return Ok(());
    }
    let messages = errors
        .iter()
        .filter_map(|error| error.get("message").and_then(Value::as_str))
        .take(4)
        .collect::<Vec<_>>()
        .join("; ");
    Err(LinearReadError::GraphQl(if messages.is_empty() {
        "unknown GraphQL error".into()
    } else {
        messages
    }))
}

fn normalize_issue(raw: RawIssue) -> Result<AutomationIssue, LinearReadError> {
    let id = nonempty(raw.id, "id")?;
    let identifier = nonempty(raw.identifier, "identifier")?;
    let title = nonempty(raw.title, "title")?;
    let state = nonempty(raw.state.name, "state.name")?;
    let labels = raw
        .labels
        .unwrap_or_default()
        .nodes
        .into_iter()
        .filter_map(|label| {
            let label = label.name.trim().to_ascii_lowercase();
            (!label.is_empty()).then_some(label)
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut blockers = BTreeSet::new();
    for relation in raw.relations.unwrap_or_default().nodes {
        if matches!(relation.kind.as_str(), "blocked_by" | "blockedBy")
            && let Some(issue) = relation.related_issue.or(relation.issue)
        {
            blockers.insert(issue);
        }
    }
    for relation in raw.inverse_relations.unwrap_or_default().nodes {
        if relation.kind == "blocks"
            && let Some(issue) = relation.issue.or(relation.related_issue)
        {
            blockers.insert(issue);
        }
    }
    Ok(AutomationIssue {
        id,
        identifier,
        title,
        description: trimmed_optional(raw.description),
        priority: raw.priority.filter(|priority| *priority > 0),
        state,
        branch_name: trimmed_optional(raw.branch_name),
        url: trimmed_optional(raw.url),
        labels,
        blocked_by: blockers
            .into_iter()
            .map(|blocker| AutomationIssueBlocker {
                id: blocker.id.and_then(nonempty_optional),
                identifier: blocker.identifier.and_then(nonempty_optional),
                state: blocker
                    .state
                    .map(|state| state.name)
                    .and_then(nonempty_optional),
            })
            .collect(),
        created_at: trimmed_optional(raw.created_at),
        updated_at: trimmed_optional(raw.updated_at),
    })
}

fn nonempty(value: String, field: &str) -> Result<String, LinearReadError> {
    nonempty_optional(value)
        .ok_or_else(|| LinearReadError::InvalidData(format!("`{field}` is empty")))
}

fn nonempty_optional(value: String) -> Option<String> {
    let value = value.trim().to_owned();
    (!value.is_empty()).then_some(value)
}

fn trimmed_optional(value: Option<String>) -> Option<String> {
    value.and_then(nonempty_optional)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn issue(identifier: &str, state: &str) -> Value {
        json!({
            "id": format!("id-{identifier}"),
            "identifier": identifier,
            "title": format!("  {identifier} title  "),
            "description": " description ",
            "priority": 0,
            "state": {"name": state},
            "branchName": " branch ",
            "url": format!("https://linear.app/issue/{identifier}"),
            "labels": {"nodes": [{"name": " Automation "}, {"name": "bug"}, {"name": "BUG"}]},
            "relations": {"nodes": [{
                "type": "blocked_by",
                "relatedIssue": {"id": "blocker-2", "identifier": "ORC-2", "state": {"name": "Todo"}}
            }]},
            "inverseRelations": {"nodes": [{
                "type": "blocks",
                "issue": {"id": "blocker-1", "identifier": "ORC-1", "state": {"name": "Done"}}
            }]},
            "createdAt": "2026-07-01T00:00:00.000Z",
            "updatedAt": "2026-07-16T00:00:00.000Z"
        })
    }

    #[test]
    fn paginated_linear_values_normalize_stably_without_raw_graphql() {
        let response = json!({"data": {"project": {"issues": {
            "nodes": [issue("ORC-35", "In Progress"), issue("ORC-12", "Done")],
            "pageInfo": {"hasNextPage": true, "endCursor": "cursor-2"}
        }}}});
        let page = normalize_linear_issue_page(&response).unwrap();
        assert_eq!(page.issues[0].identifier, "ORC-12");
        assert_eq!(page.issues[1].labels, vec!["automation", "bug"]);
        assert_eq!(page.issues[1].priority, None);
        assert_eq!(
            page.issues[1]
                .blocked_by
                .iter()
                .map(|blocker| blocker.identifier.as_deref().unwrap())
                .collect::<Vec<_>>(),
            vec!["ORC-1", "ORC-2"]
        );
        assert!(page.has_next_page);
        assert_eq!(page.end_cursor.as_deref(), Some("cursor-2"));
    }

    #[test]
    fn refresh_and_graphql_failures_are_typed() {
        let refreshed = normalize_linear_issue(&json!({
            "data": {"issue": issue("ORC-35", "Done")}
        }))
        .unwrap();
        assert_eq!(refreshed.state, "Done");
        assert_eq!(refreshed.title, "ORC-35 title");
        assert_eq!(
            normalize_linear_issue_page(&json!({"errors": [{"message": "rate limited"}]})),
            Err(LinearReadError::GraphQl("rate limited".into()))
        );
    }
}
