use crate::ExecutionPlan;
use crate::RepeatPolicy;
use crate::Step;
use crate::WorktreePolicy;
use crate::validate_plan;
use serde_json::Map;
use serde_json::Number;
use serde_json::Value;
use std::collections::BTreeSet;
use thiserror::Error;

const SDK: &str = "@codex-orchestra/workflow";
const CALLS: &[&str] = &[
    "workflow",
    "defineWorkflow",
    "agent",
    "parallel",
    "pipeline",
    "check",
    "approval",
    "worktree",
    "repeat",
    "ref",
];

#[derive(Clone, Debug, Error, PartialEq)]
#[error("workflow compile error at byte {offset}: {message}")]
pub struct CompileError {
    pub offset: usize,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq)]
enum Ast {
    Null,
    Bool(bool),
    Number(Number),
    String(String),
    Array(Vec<Ast>),
    Object(Vec<(String, Ast)>),
    Call(String, Vec<Ast>),
}

#[derive(Clone, Debug, PartialEq)]
enum TokenKind {
    Ident(String),
    String(String),
    Template(String),
    Number(Number),
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    LParen,
    RParen,
    Colon,
    Comma,
    Semi,
}

#[derive(Clone, Debug, PartialEq)]
struct Token {
    offset: usize,
    kind: TokenKind,
}

/// Compile restricted `.workflow.ts` source without executing TypeScript.
pub fn compile_workflow(source: &str) -> Result<ExecutionPlan, CompileError> {
    let tokens = lex(source)?;
    let mut parser = Parser {
        tokens,
        cursor: 0,
        imports: BTreeSet::new(),
    };
    parser.parse_import()?;
    parser.ident("export")?;
    parser.ident("default")?;
    let ast = parser.expression()?;
    parser.eat(&TokenKind::Semi);
    if parser.cursor != parser.tokens.len() {
        return parser.error("side effects or trailing statements are not allowed");
    }
    let plan = lower_workflow(ast)?;
    let errors = validate_plan(&plan);
    if let Some(error) = errors.first() {
        return Err(CompileError {
            offset: 0,
            message: error.to_string(),
        });
    }
    Ok(plan)
}

struct Parser {
    tokens: Vec<Token>,
    cursor: usize,
    imports: BTreeSet<String>,
}

impl Parser {
    fn parse_import(&mut self) -> Result<(), CompileError> {
        self.ident("import")?;
        self.expect(TokenKind::LBrace)?;
        loop {
            let name = self.take_ident()?;
            if !CALLS.contains(&name.as_str()) {
                return self.error(&format!("`{name}` is not an approved Orchestra DSL import"));
            }
            if self.peek_ident("as") {
                return self.error("import aliases are not allowed");
            }
            self.imports.insert(name);
            if self.eat(&TokenKind::Comma) {
                continue;
            }
            break;
        }
        self.expect(TokenKind::RBrace)?;
        self.ident("from")?;
        let module = self.take_string()?;
        if module != SDK {
            return self.error("workflow may import only `@codex-orchestra/workflow`");
        }
        self.eat(&TokenKind::Semi);
        Ok(())
    }

    fn expression(&mut self) -> Result<Ast, CompileError> {
        let Some(token) = self.tokens.get(self.cursor).cloned() else {
            return self.error("expected expression");
        };
        self.cursor += 1;
        match token.kind {
            TokenKind::String(value) => Ok(Ast::String(value)),
            TokenKind::Template(value) => {
                validate_template(&value, token.offset)?;
                Ok(Ast::String(value))
            }
            TokenKind::Number(value) => Ok(Ast::Number(value)),
            TokenKind::Ident(value) if value == "true" => Ok(Ast::Bool(true)),
            TokenKind::Ident(value) if value == "false" => Ok(Ast::Bool(false)),
            TokenKind::Ident(value) if value == "null" => Ok(Ast::Null),
            TokenKind::Ident(name) => {
                if !self.imports.contains(&name) {
                    return Err(CompileError {
                        offset: token.offset,
                        message: format!(
                            "reference to non-Orchestra identifier `{name}` is not allowed"
                        ),
                    });
                }
                self.expect(TokenKind::LParen)?;
                let mut args = Vec::new();
                if !self.eat(&TokenKind::RParen) {
                    loop {
                        args.push(self.expression()?);
                        if self.eat(&TokenKind::Comma) {
                            if self.eat(&TokenKind::RParen) {
                                break;
                            }
                            continue;
                        }
                        self.expect(TokenKind::RParen)?;
                        break;
                    }
                }
                Ok(Ast::Call(name, args))
            }
            TokenKind::LBracket => {
                let mut values = Vec::new();
                if !self.eat(&TokenKind::RBracket) {
                    loop {
                        values.push(self.expression()?);
                        if self.eat(&TokenKind::Comma) {
                            if self.eat(&TokenKind::RBracket) {
                                break;
                            }
                            continue;
                        }
                        self.expect(TokenKind::RBracket)?;
                        break;
                    }
                }
                Ok(Ast::Array(values))
            }
            TokenKind::LBrace => {
                let mut entries = Vec::new();
                if !self.eat(&TokenKind::RBrace) {
                    loop {
                        let key = match self.tokens.get(self.cursor).cloned() {
                            Some(Token {
                                kind: TokenKind::Ident(key) | TokenKind::String(key),
                                ..
                            }) => {
                                self.cursor += 1;
                                key
                            }
                            _ => {
                                return self
                                    .error("object keys must be static identifiers or strings");
                            }
                        };
                        self.expect(TokenKind::Colon)?;
                        entries.push((key, self.expression()?));
                        if self.eat(&TokenKind::Comma) {
                            if self.eat(&TokenKind::RBrace) {
                                break;
                            }
                            continue;
                        }
                        self.expect(TokenKind::RBrace)?;
                        break;
                    }
                }
                Ok(Ast::Object(entries))
            }
            _ => Err(CompileError {
                offset: token.offset,
                message: "unsupported TypeScript expression".into(),
            }),
        }
    }

    fn ident(&mut self, expected: &str) -> Result<(), CompileError> {
        match self.tokens.get(self.cursor) {
            Some(Token {
                kind: TokenKind::Ident(value),
                ..
            }) if value == expected => {
                self.cursor += 1;
                Ok(())
            }
            _ => self.error(&format!("expected `{expected}`")),
        }
    }
    fn peek_ident(&self, value: &str) -> bool {
        matches!(self.tokens.get(self.cursor), Some(Token { kind: TokenKind::Ident(v), .. }) if v == value)
    }
    fn take_ident(&mut self) -> Result<String, CompileError> {
        match self.tokens.get(self.cursor).cloned() {
            Some(Token {
                kind: TokenKind::Ident(v),
                ..
            }) => {
                self.cursor += 1;
                Ok(v)
            }
            _ => self.error("expected identifier"),
        }
    }
    fn take_string(&mut self) -> Result<String, CompileError> {
        match self.tokens.get(self.cursor).cloned() {
            Some(Token {
                kind: TokenKind::String(v),
                ..
            }) => {
                self.cursor += 1;
                Ok(v)
            }
            _ => self.error("expected string"),
        }
    }
    fn expect(&mut self, kind: TokenKind) -> Result<(), CompileError> {
        if self.eat(&kind) {
            Ok(())
        } else {
            self.error(&format!("expected {kind:?}"))
        }
    }
    fn eat(&mut self, kind: &TokenKind) -> bool {
        if self
            .tokens
            .get(self.cursor)
            .is_some_and(|t| &t.kind == kind)
        {
            self.cursor += 1;
            true
        } else {
            false
        }
    }
    fn error<T>(&self, message: &str) -> Result<T, CompileError> {
        Err(CompileError {
            offset: self.tokens.get(self.cursor).map_or(0, |t| t.offset),
            message: message.into(),
        })
    }
}

fn lex(source: &str) -> Result<Vec<Token>, CompileError> {
    let bytes = source.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    while i < bytes.len() {
        if bytes[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if bytes[i..].starts_with(b"//") {
            i += 2;
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if bytes[i..].starts_with(b"/*") {
            let start = i;
            i += 2;
            while i + 1 < bytes.len() && !bytes[i..].starts_with(b"*/") {
                i += 1;
            }
            if i + 1 >= bytes.len() {
                return Err(CompileError {
                    offset: start,
                    message: "unterminated comment".into(),
                });
            }
            i += 2;
            continue;
        }
        let offset = i;
        let single = match bytes[i] {
            b'{' => Some(TokenKind::LBrace),
            b'}' => Some(TokenKind::RBrace),
            b'[' => Some(TokenKind::LBracket),
            b']' => Some(TokenKind::RBracket),
            b'(' => Some(TokenKind::LParen),
            b')' => Some(TokenKind::RParen),
            b':' => Some(TokenKind::Colon),
            b',' => Some(TokenKind::Comma),
            b';' => Some(TokenKind::Semi),
            _ => None,
        };
        if let Some(kind) = single {
            out.push(Token { offset, kind });
            i += 1;
            continue;
        }
        if matches!(bytes[i], b'\'' | b'"' | b'`') {
            let quote = bytes[i];
            i += 1;
            let mut value = String::new();
            while i < bytes.len() && bytes[i] != quote {
                if bytes[i] == b'\\' {
                    i += 1;
                    if i >= bytes.len() {
                        break;
                    }
                    value.push(match bytes[i] {
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        other => other as char,
                    });
                    i += 1;
                } else {
                    value.push(bytes[i] as char);
                    i += 1;
                }
            }
            if i >= bytes.len() {
                return Err(CompileError {
                    offset,
                    message: "unterminated string".into(),
                });
            }
            i += 1;
            out.push(Token {
                offset,
                kind: if quote == b'`' {
                    TokenKind::Template(value)
                } else {
                    TokenKind::String(value)
                },
            });
            continue;
        }
        if bytes[i].is_ascii_digit()
            || (bytes[i] == b'-' && bytes.get(i + 1).is_some_and(u8::is_ascii_digit))
        {
            let start = i;
            i += 1;
            while i < bytes.len()
                && (bytes[i].is_ascii_digit()
                    || matches!(bytes[i], b'.' | b'e' | b'E' | b'+' | b'-'))
            {
                i += 1;
            }
            let text = &source[start..i];
            let number = if !text.contains(['.', 'e', 'E']) {
                text.parse::<i64>().ok().map(Number::from)
            } else {
                text.parse::<f64>().ok().and_then(Number::from_f64)
            }
            .ok_or_else(|| CompileError {
                offset,
                message: "invalid number".into(),
            })?;
            out.push(Token {
                offset,
                kind: TokenKind::Number(number),
            });
            continue;
        }
        if bytes[i].is_ascii_alphabetic() || matches!(bytes[i], b'_' | b'$') {
            let start = i;
            i += 1;
            while i < bytes.len()
                && (bytes[i].is_ascii_alphanumeric() || matches!(bytes[i], b'_' | b'$' | b'-'))
            {
                i += 1;
            }
            out.push(Token {
                offset,
                kind: TokenKind::Ident(source[start..i].into()),
            });
            continue;
        }
        return Err(CompileError {
            offset,
            message: format!("token `{}` is not allowed", bytes[i] as char),
        });
    }
    Ok(out)
}

fn validate_template(value: &str, offset: usize) -> Result<(), CompileError> {
    let mut rest = value;
    while let Some(start) = rest.find("${") {
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            return Err(CompileError {
                offset,
                message: "unterminated template reference".into(),
            });
        };
        let reference = &after[..end];
        if !valid_reference(reference) {
            return Err(CompileError {
                offset,
                message:
                    "templates may reference only `inputs.<name>` or `steps.<id>.outputs.<name>`"
                        .into(),
            });
        }
        rest = &after[end + 1..];
    }
    Ok(())
}

fn valid_reference(reference: &str) -> bool {
    let parts: Vec<_> = reference.split('.').collect();
    matches!(parts.as_slice(), ["inputs", name] if !name.is_empty())
        || matches!(
            parts.as_slice(),
            ["steps", step, "outputs", output] if !step.is_empty() && !output.is_empty()
        )
}

fn lower_workflow(ast: Ast) -> Result<ExecutionPlan, CompileError> {
    let Ast::Call(name, mut args) = ast else {
        return lower_error("default export must call `workflow` or `defineWorkflow`");
    };
    if !matches!(name.as_str(), "workflow" | "defineWorkflow") || args.len() != 1 {
        return lower_error("default export must be one workflow call with one object argument");
    }
    let Ast::Object(mut entries) = args.remove(0) else {
        return lower_error("workflow argument must be an object");
    };
    let steps_ast =
        take_entry(&mut entries, "steps").ok_or_else(|| lower("workflow requires `steps`"))?;
    let Ast::Array(nodes) = steps_ast else {
        return lower_error("workflow `steps` must be an array");
    };
    let mut steps = Vec::new();
    for node in nodes {
        steps.extend(lower_node(node)?);
    }
    let mut value = object_to_json(entries)?;
    value.insert(
        "steps".into(),
        serde_json::to_value(steps).expect("steps serialize"),
    );
    serde_json::from_value(Value::Object(value))
        .map_err(|error| lower(format!("invalid workflow: {error}")))
}

fn lower_node(ast: Ast) -> Result<Vec<Step>, CompileError> {
    let Ast::Call(name, mut args) = ast else {
        return lower_error("steps must be Orchestra DSL calls");
    };
    match name.as_str() {
        "agent" | "check" | "approval" => {
            if args.len() != 1 {
                return lower_error(&format!("`{name}` expects one object"));
            }
            let Ast::Object(entries) = args.remove(0) else {
                return lower_error(&format!("`{name}` expects an object"));
            };
            let mut value = object_to_json(entries)?;
            value.insert("kind".into(), Value::String(name));
            let step: Step = serde_json::from_value(Value::Object(value))
                .map_err(|error| lower(format!("invalid step: {error}")))?;
            Ok(vec![step])
        }
        "parallel" | "pipeline" => {
            if args.len() != 1 {
                return lower_error(&format!("`{name}` expects one array"));
            }
            let Ast::Array(nodes) = args.remove(0) else {
                return lower_error(&format!("`{name}` expects an array"));
            };
            let mut groups = Vec::new();
            for node in nodes {
                groups.push(lower_node(node)?);
            }
            if name == "pipeline" {
                for index in 1..groups.len() {
                    let prior: Vec<_> = groups[index - 1].iter().map(|s| s.id.clone()).collect();
                    for step in &mut groups[index] {
                        for dependency in &prior {
                            if !step.needs.contains(dependency) {
                                step.needs.push(dependency.clone());
                            }
                        }
                    }
                }
            }
            Ok(groups.into_iter().flatten().collect())
        }
        "worktree" => {
            if args.len() != 2 {
                return lower_error("`worktree` expects a step and `shared` or `isolated`");
            }
            let policy = match args.pop() {
                Some(Ast::String(v)) if v == "isolated" => WorktreePolicy::Isolated,
                Some(Ast::String(v)) if v == "shared" => WorktreePolicy::Shared,
                _ => return lower_error("invalid worktree policy"),
            };
            let mut steps = lower_node(args.remove(0))?;
            for step in &mut steps {
                step.worktree = policy.clone();
            }
            Ok(steps)
        }
        "repeat" => {
            if args.len() != 2 {
                return lower_error("`repeat` expects a step and policy object");
            }
            let policy = ast_to_json(args.pop().unwrap())?;
            let repeat: RepeatPolicy = serde_json::from_value(policy)
                .map_err(|error| lower(format!("invalid repeat policy: {error}")))?;
            let mut steps = lower_node(args.remove(0))?;
            for step in &mut steps {
                step.repeat = Some(repeat.clone());
            }
            Ok(steps)
        }
        _ => lower_error(&format!("`{name}` is not valid in a step list")),
    }
}

fn object_to_json(entries: Vec<(String, Ast)>) -> Result<Map<String, Value>, CompileError> {
    let mut out = Map::new();
    for (key, value) in entries {
        if out.insert(key.clone(), ast_to_json(value)?).is_some() {
            return lower_error(&format!("duplicate object key `{key}`"));
        }
    }
    Ok(out)
}

fn ast_to_json(ast: Ast) -> Result<Value, CompileError> {
    match ast {
        Ast::Null => Ok(Value::Null),
        Ast::Bool(v) => Ok(Value::Bool(v)),
        Ast::Number(v) => Ok(Value::Number(v)),
        Ast::String(v) => Ok(Value::String(v)),
        Ast::Array(values) => values
            .into_iter()
            .map(ast_to_json)
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array),
        Ast::Object(entries) => object_to_json(entries).map(Value::Object),
        Ast::Call(name, mut args) if name == "ref" && args.len() == 1 => match args.remove(0) {
            Ast::String(v) if valid_reference(&v) => Ok(Value::String(format!("${{{v}}}"))),
            Ast::String(_) => lower_error(
                "`ref` may reference only `inputs.<name>` or `steps.<id>.outputs.<name>`",
            ),
            _ => lower_error("`ref` expects one string"),
        },
        Ast::Call(name, _) => lower_error(&format!("DSL call `{name}` is not valid in this value")),
    }
}

fn take_entry(entries: &mut Vec<(String, Ast)>, key: &str) -> Option<Ast> {
    entries
        .iter()
        .position(|(name, _)| name == key)
        .map(|i| entries.remove(i).1)
}
fn lower(message: impl Into<String>) -> CompileError {
    CompileError {
        offset: 0,
        message: message.into(),
    }
}
fn lower_error<T>(message: &str) -> Result<T, CompileError> {
    Err(lower(message))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Action;

    fn valid() -> String {
        r#"import { workflow, agent, pipeline, parallel, check, approval, worktree, repeat } from "@codex-orchestra/workflow";
export default workflow({ name: "slice", max_parallel: 2, steps: [pipeline([
  agent({ id: "plan", prompt: "Plan", model: "gpt-5.4", outputs: ["ok"] }),
  parallel([
    agent({ id: "write", prompt: `Use ${steps.plan.outputs.ok}`, model: "gpt-5.4", reasoning_effort: "high", write_scope: ["src/"], worktree: "isolated" }),
    check({ id: "lint", command: ["cargo", "fmt", "--check"] })
  ]),
  approval({ id: "accept", prompt: "Accept?", choices: ["yes", "no"] })
]) ] });"#.into()
    }

    #[test]
    fn compiles_pipeline_and_defaults_fork_to_none() {
        let plan = compile_workflow(&valid()).unwrap();
        assert_eq!(plan.steps[1].needs, vec!["plan"]);
        assert_eq!(plan.steps[2].needs, vec!["plan"]);
        let Action::Agent(agent) = &plan.steps[0].action else {
            panic!()
        };
        assert_eq!(agent.fork_turns, crate::ForkTurns::None);
        assert!(!agent.allow_delegation);
    }

    #[test]
    fn rejects_arbitrary_typescript_and_side_effects() {
        for source in [
            "import { workflow } from '@codex-orchestra/workflow'; export default (() => workflow({name:'x',steps:[]}))();",
            "import fs from 'node:fs'; export default fs.readFileSync('/etc/passwd');",
            "import { workflow } from '@codex-orchestra/workflow'; process.exit(); export default workflow({name:'x',steps:[]});",
            "import { workflow } from '@codex-orchestra/workflow'; export default workflow({name:'x',steps:[]}); fetch('https://x');",
            "import { workflow } from '@codex-orchestra/workflow'; export default eval('x');",
            "import { workflow } from '@codex-orchestra/workflow'; export default import('./x');",
            "import { workflow } from '@codex-orchestra/workflow'; export default workflow({name:'x',steps:[function(){}]});",
        ] {
            assert!(compile_workflow(source).is_err(), "accepted {source}");
        }
    }

    #[test]
    fn rejects_computed_template_expressions() {
        let source = valid().replace("steps.plan.outputs.ok", "process.env.SECRET");
        assert!(
            compile_workflow(&source)
                .unwrap_err()
                .message
                .contains("templates may reference only")
        );
    }

    #[test]
    fn compiles_repository_vertical_slice() {
        let source = include_str!("../fixtures/native-vertical-slice.workflow.ts");
        let plan = compile_workflow(source).unwrap();
        assert_eq!(plan.name, "native-vertical-slice");
        assert_eq!(plan.steps.len(), 5);
    }

    #[test]
    fn compiles_task_owned_automation_issue_fixture() {
        let source = include_str!("../fixtures/automation-issue.workflow.ts");
        let plan = compile_workflow(source).unwrap();
        assert_eq!(plan.name, "automation-issue");
        assert!(plan.inputs["issue"].required);
        assert!(plan.inputs["task_prompt"].required);
        assert!(plan.inputs["automation"].required);
        assert_eq!(plan.steps.len(), 1);
    }

    #[test]
    fn compiles_typed_inputs_defaults_and_input_references_as_data() {
        let source = r#"import { workflow, agent, ref } from "@codex-orchestra/workflow";
export default workflow({
  name: "inputs",
  inputs: {
    ticket: { type: "string" },
    base: { type: "string", required: false, default: "main" },
    payload: { type: "json", default: null }
  },
  steps: [agent({ id: "work", prompt: ref("inputs.ticket"), model: "gpt-5.4" })]
});"#;
        let plan = compile_workflow(source).unwrap();
        assert!(plan.inputs["ticket"].required);
        assert_eq!(
            plan.inputs["base"].default,
            crate::InputDefault::Value(Value::String("main".into()))
        );
        assert_eq!(
            plan.inputs["payload"].default,
            crate::InputDefault::Value(Value::Null)
        );
        let Action::Agent(agent) = &plan.steps[0].action else {
            panic!()
        };
        assert_eq!(agent.prompt, "${inputs.ticket}");
    }

    #[test]
    fn compiles_explicit_skill_requirement_closure_as_data() {
        let source = r#"import { workflow, agent } from "@codex-orchestra/workflow";
export default workflow({
  name: "skills",
  steps: [agent({
    id: "work",
    prompt: "Implement",
    model: "gpt-5.4",
    skills: [
      { name: "implement", requires: ["tdd"] },
      { name: "tdd", resources: ["references/testing.md"] }
    ]
  })]
});"#;
        let plan = compile_workflow(source).unwrap();
        let Action::Agent(agent) = &plan.steps[0].action else {
            panic!()
        };
        assert_eq!(agent.skills[0].requires, ["tdd"]);
        assert_eq!(agent.skills[1].resources, ["references/testing.md"]);
    }
}
