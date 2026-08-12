//! Named coding plans as data + in-process step runner (host-owned).
//!
//! Steps are either **host tools** (resolve, open/submit PR) or **agent**
//! tools (write, check, commit). The runner advances a cursor; host steps are
//! executed by `run_coding_plan`, agent steps return a concrete next action.
//!
//! Plans: `coding.slice`, `coding.fix_diagnostics`, `coding.finish_task`.

use std::collections::HashMap;
use std::sync::Mutex;

use serde_json::{json, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanId {
    Slice,
    FixDiagnostics,
    FinishTask,
}

impl PlanId {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "coding.slice" | "slice" => Some(Self::Slice),
            "coding.fix_diagnostics" | "fix_diagnostics" | "fix" | "fix_all" => {
                Some(Self::FixDiagnostics)
            }
            "coding.finish_task" | "finish_task" | "finish" | "open_pr" => Some(Self::FinishTask),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Slice => "coding.slice",
            Self::FixDiagnostics => "coding.fix_diagnostics",
            Self::FinishTask => "coding.finish_task",
        }
    }
}

#[derive(Debug, Clone)]
pub struct PlanStepSpec {
    pub id: &'static str,
    /// `host` = orchestrator/tool runs it; `agent` = LLM must perform.
    pub owner: &'static str,
    pub tool: Option<&'static str>,
    pub instruction: &'static str,
}

pub fn plan_steps(id: PlanId) -> &'static [PlanStepSpec] {
    match id {
        PlanId::Slice => &[
            PlanStepSpec {
                id: "resolve",
                owner: "host",
                tool: Some("resolve_coding_target"),
                instruction: "Match task to open unmerged PR or new work line",
            },
            PlanStepSpec {
                id: "check_baseline",
                owner: "agent",
                tool: Some("veil_check"),
                instruction: "Baseline host check before edit",
            },
            PlanStepSpec {
                id: "write",
                owner: "agent",
                tool: Some("write_source"),
                instruction: "Apply minimal correct fix for one slice",
            },
            PlanStepSpec {
                id: "check_after",
                owner: "agent",
                tool: Some("veil_check"),
                instruction: "Re-check; fix new diags same turn if introduced",
            },
            PlanStepSpec {
                id: "commit",
                owner: "agent",
                tool: Some("session_commit"),
                instruction: "Commit successful slice (host rejects empty commits)",
            },
        ],
        PlanId::FixDiagnostics => &[
            PlanStepSpec {
                id: "resolve",
                owner: "host",
                tool: Some("resolve_coding_target"),
                instruction: "Bind open PR when scope matches; else new line",
            },
            PlanStepSpec {
                id: "status",
                owner: "agent",
                tool: Some("session_status"),
                instruction: "Confirm branch / uncommitted / host_check",
            },
            PlanStepSpec {
                id: "branch",
                owner: "agent",
                tool: Some("create_branch"),
                instruction: "If multi-step and on main: create_branch (skip if already on feature)",
            },
            PlanStepSpec {
                id: "loop_slices",
                owner: "agent",
                tool: Some("run_coding_plan"),
                instruction:
                    "Run coding.slice (or write→check→commit) per diagnostic class until clean",
            },
            PlanStepSpec {
                id: "finish",
                owner: "host",
                tool: Some("run_coding_plan"),
                instruction: "When task done: coding.finish_task (open/submit PR)",
            },
        ],
        PlanId::FinishTask => &[
            PlanStepSpec {
                id: "ensure_commits",
                owner: "host",
                tool: None,
                instruction: "Warn if no commits / clean tree before open PR",
            },
            PlanStepSpec {
                id: "open_or_reuse_pr",
                owner: "host",
                tool: Some("create_pr"),
                instruction: "Reuse active_pr_id or open PR",
            },
            PlanStepSpec {
                id: "submit_pr",
                owner: "host",
                tool: Some("submit_pr"),
                instruction: "Submit for PR Wizard; surface host_check",
            },
        ],
    }
}

pub fn plan_json(id: PlanId) -> Value {
    let steps: Vec<Value> = plan_steps(id)
        .iter()
        .map(|s| {
            json!({
                "id": s.id,
                "owner": s.owner,
                "tool": s.tool,
                "instruction": s.instruction,
            })
        })
        .collect();
    json!({
        "plan": id.as_str(),
        "steps": steps,
        "rules": {
            "commit_per_slice": true,
            "pr_when_task_done": true,
            "never_auto_merge": true,
            "product_name": "pull_request",
            "not_ticket": true
        }
    })
}

/// Live run cursor for a project (process-local; survives turns in same host).
#[derive(Debug, Clone)]
pub struct PlanRun {
    pub plan: PlanId,
    /// Index into plan_steps.
    pub cursor: usize,
    pub request: String,
    pub project: Option<String>,
    pub completed: Vec<String>,
    pub resolve: Value,
    pub started_ms: u128,
}

fn runs() -> &'static Mutex<HashMap<String, PlanRun>> {
    static RUNS: std::sync::OnceLock<Mutex<HashMap<String, PlanRun>>> = std::sync::OnceLock::new();
    RUNS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn run_key(project: Option<&str>, plan: PlanId) -> String {
    format!("{}::{}", project.unwrap_or("_"), plan.as_str())
}

fn now_ms() -> u128 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

pub fn get_run(project: Option<&str>, plan: PlanId) -> Option<PlanRun> {
    runs()
        .lock()
        .ok()?
        .get(&run_key(project, plan))
        .cloned()
}

pub fn put_run(run: PlanRun) {
    if let Ok(mut g) = runs().lock() {
        let k = run_key(run.project.as_deref(), run.plan);
        g.insert(k, run);
    }
}

pub fn clear_run(project: Option<&str>, plan: PlanId) {
    if let Ok(mut g) = runs().lock() {
        g.remove(&run_key(project, plan));
    }
}

pub fn start_run(plan: PlanId, request: String, project: Option<String>, resolve: Value) -> PlanRun {
    let run = PlanRun {
        plan,
        cursor: 0,
        request,
        project: project.clone(),
        completed: vec![],
        resolve,
        started_ms: now_ms(),
    };
    put_run(run.clone());
    run
}

/// Mark current step done and advance cursor. Returns None if plan finished.
pub fn advance_run(project: Option<&str>, plan: PlanId) -> Option<PlanRun> {
    let mut run = get_run(project, plan)?;
    let steps = plan_steps(run.plan);
    if run.cursor < steps.len() {
        run.completed.push(steps[run.cursor].id.to_string());
        run.cursor += 1;
    }
    if run.cursor >= steps.len() {
        clear_run(project, plan);
        return Some(run);
    }
    put_run(run.clone());
    Some(run)
}

pub fn current_step(run: &PlanRun) -> Option<&'static PlanStepSpec> {
    plan_steps(run.plan).get(run.cursor)
}

/// Skip host steps that were already satisfied at start (e.g. resolve done).
pub fn skip_completed_host_prefix(run: &mut PlanRun, already: &[&str]) {
    let steps = plan_steps(run.plan);
    while run.cursor < steps.len() {
        let s = &steps[run.cursor];
        if s.owner == "host" && already.contains(&s.id) {
            run.completed.push(s.id.to_string());
            run.cursor += 1;
        } else {
            break;
        }
    }
    put_run(run.clone());
}

pub fn run_status_json(run: &PlanRun) -> Value {
    let steps = plan_steps(run.plan);
    let cur = current_step(run);
    json!({
        "plan": run.plan.as_str(),
        "cursor": run.cursor,
        "total_steps": steps.len(),
        "completed": run.completed,
        "done": run.cursor >= steps.len(),
        "current": cur.map(|s| json!({
            "id": s.id,
            "owner": s.owner,
            "tool": s.tool,
            "instruction": s.instruction,
        })),
        "request": run.request,
        "project": run.project,
        "resolve": run.resolve,
        "started_ms": run.started_ms,
    })
}

/// Next action payload for the agent (or host).
pub fn next_action_json(run: &PlanRun) -> Value {
    match current_step(run) {
        None => json!({
            "phase": "done",
            "summary": format!("Plan {} complete", run.plan.as_str()),
            "run": run_status_json(run),
        }),
        Some(s) if s.owner == "agent" => json!({
            "phase": "agent_step",
            "step_id": s.id,
            "tool": s.tool,
            "instruction": s.instruction,
            "must_call": s.tool,
            "after_success": "run_coding_plan({ plan, action: \"next\" })",
            "run": run_status_json(run),
            "playbook": agent_playbook(run.plan, &run.resolve),
        }),
        Some(s) => json!({
            "phase": "host_step",
            "step_id": s.id,
            "tool": s.tool,
            "instruction": s.instruction,
            "run": run_status_json(run),
        }),
    }
}

/// Agent-facing playbook after host resolve (injected into tool result).
pub fn agent_playbook(id: PlanId, resolve: &Value) -> String {
    let decision = resolve
        .get("decision")
        .and_then(|v| v.as_str())
        .unwrap_or("new");
    let mut lines = vec![
        format!("## Coding plan `{}` (host-owned step runner)", id.as_str()),
        format!("Resolve decision: **{decision}**"),
    ];
    if let Some(pr) = resolve.get("pr_id").and_then(|v| v.as_str()) {
        lines.push(format!("Bound PR id: `{pr}` — reuse; do not open a second PR."));
    }
    if decision == "needs_choice" {
        lines.push(
            "Operator must pick a PR (Present modal). After ACK, call \
             resolve_coding_target({choice}) then run_coding_plan again."
                .into(),
        );
    }
    lines.push("### Steps (call run_coding_plan action=next after each agent step)".into());
    for (i, s) in plan_steps(id).iter().enumerate() {
        lines.push(format!(
            "{}. [{}] {}{}",
            i + 1,
            s.owner,
            s.instruction,
            s.tool.map(|t| format!(" (`{t}`)")).unwrap_or_default()
        ));
    }
    lines.push(
        "Host enforces: empty session_commit rejected; submit surfaces host_check; \
         VEIL_STRICT_SUBMIT=1 hard-refuses submit on host errors; never merge unless asked."
            .into(),
    );
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_plan_ids() {
        assert_eq!(PlanId::parse("fix_all"), Some(PlanId::FixDiagnostics));
        assert_eq!(PlanId::parse("coding.finish_task"), Some(PlanId::FinishTask));
        assert_eq!(PlanId::parse("slice"), Some(PlanId::Slice));
        assert!(PlanId::parse("nope").is_none());
    }

    #[test]
    fn fix_plan_has_resolve_and_finish() {
        let steps = plan_steps(PlanId::FixDiagnostics);
        assert!(steps.iter().any(|s| s.id == "resolve"));
        assert!(steps.iter().any(|s| s.id == "finish"));
    }

    #[test]
    fn step_runner_advances() {
        let mut run = start_run(
            PlanId::Slice,
            "fix x".into(),
            Some("demo".into()),
            json!({ "decision": "new" }),
        );
        skip_completed_host_prefix(&mut run, &["resolve"]);
        assert_eq!(current_step(&run).unwrap().id, "check_baseline");
        let run = advance_run(Some("demo"), PlanId::Slice).unwrap();
        assert_eq!(current_step(&run).unwrap().id, "write");
        clear_run(Some("demo"), PlanId::Slice);
    }
}
