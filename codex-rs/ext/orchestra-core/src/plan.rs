use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionPlan {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub inputs: BTreeMap<String, InputDefinition>,
    #[serde(default = "default_parallel")]
    pub max_parallel: usize,
    pub steps: Vec<Step>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InputDefinition {
    #[serde(rename = "type")]
    pub kind: InputKind,
    #[serde(default = "default_true")]
    pub required: bool,
    #[serde(default, skip_serializing_if = "InputDefault::is_missing")]
    pub default: InputDefault,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum InputKind {
    String,
    Number,
    Boolean,
    Object,
    Array,
    Json,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum InputDefault {
    #[default]
    Missing,
    Value(Value),
}

impl InputDefault {
    pub fn is_missing(&self) -> bool {
        matches!(self, Self::Missing)
    }

    pub fn value(&self) -> Option<&Value> {
        match self {
            Self::Missing => None,
            Self::Value(value) => Some(value),
        }
    }
}

impl<'de> Deserialize<'de> for InputDefault {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Value::deserialize(deserializer).map(Self::Value)
    }
}

impl Serialize for InputDefault {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Missing => serializer.serialize_unit(),
            Self::Value(value) => value.serialize(serializer),
        }
    }
}

fn default_parallel() -> usize {
    4
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Step {
    pub id: String,
    #[serde(default)]
    pub needs: Vec<String>,
    #[serde(default = "default_attempts")]
    pub max_attempts: u32,
    #[serde(default)]
    pub repeat: Option<RepeatPolicy>,
    #[serde(default)]
    pub worktree: WorktreePolicy,
    #[serde(default)]
    pub write_scope: Vec<String>,
    #[serde(flatten)]
    pub action: Action,
}

fn default_attempts() -> u32 {
    1
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Action {
    Agent(AgentStep),
    Check(CheckStep),
    Approval(ApprovalStep),
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentStep {
    pub prompt: String,
    pub model: String,
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    #[serde(default)]
    pub service_tier: Option<String>,
    #[serde(default)]
    pub fork_turns: ForkTurns,
    #[serde(default)]
    pub context: Vec<ContextSource>,
    #[serde(default)]
    pub skills: Vec<SkillRequirement>,
    #[serde(default)]
    pub outputs: Vec<String>,
    #[serde(default)]
    pub allow_delegation: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SkillRequirement {
    pub name: String,
    #[serde(default)]
    pub requires: Vec<String>,
    #[serde(default)]
    pub resources: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CheckStep {
    pub command: Vec<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
}

fn default_timeout() -> u64 {
    120_000
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalStep {
    pub prompt: String,
    #[serde(default)]
    pub choices: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorktreePolicy {
    #[default]
    Shared,
    Isolated,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ForkTurns {
    #[default]
    None,
    All,
    Last(usize),
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RepeatPolicy {
    pub max_rounds: u32,
    pub until_output: String,
    #[serde(default)]
    pub equals: Value,
    #[serde(default = "default_true")]
    pub stop_on_no_progress: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContextSource {
    File {
        path: String,
    },
    Range {
        path: String,
        start: usize,
        end: usize,
    },
    Diff {
        from: String,
        to: String,
        #[serde(default)]
        paths: Vec<String>,
    },
    Revision {
        revision: String,
        path: String,
    },
    DependencyOutput {
        step: String,
        output: String,
    },
    Input {
        input: String,
    },
}

pub type StepOutputs = BTreeMap<String, Value>;
