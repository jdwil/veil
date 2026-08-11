//! Named coding plans as data (host-owned, backend-agnostic).
//!
//! Steps are either **host tools** (gates, resolve, open/submit PR) or
//! **agent instructions** (LLM judgment: rewrite source). The model does not
//! own SOP order — this module does.
//!
//! Plans: `coding.slice`, `coding.fix_diagnostics`, `coding.finish_task`.

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
                tool: None,
                instruction: "Repeat coding.slice per diagnostic class until target clean or budget",
            },
            PlanStepSpec {
                id: "finish",
                owner: "host",
                tool: Some("run_coding_plan"),
                instruction: "When task done: run coding.finish_task (open/submit PR)",
            },
        ],
        PlanId::FinishTask => &[
            PlanStepSpec {
                id: "ensure_commits",
                owner: "host",
                tool: None,
                instruction: "Refuse empty PR if no commits and clean tree (soft warn + create if forced)",
            },
            PlanStepSpec {
                id: "open_or_reuse_pr",
                owner: "host",
                tool: Some("create_change"),
                instruction: "Reuse active_change_id or open PR (product name: pull request)",
            },
            PlanStepSpec {
                id: "submit_pr",
                owner: "host",
                tool: Some("submit_change"),
                instruction: "Submit for PR Wizard; surface host_check MUST_ACKNOWLEDGE_ERRORS",
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

/// Agent-facing playbook after host resolve (injected into tool result).
pub fn agent_playbook(id: PlanId, resolve: &Value) -> String {
    let decision = resolve
        .get("decision")
        .and_then(|v| v.as_str())
        .unwrap_or("new");
    let mut lines = vec![
        format!("## Coding plan `{}` (host-owned)", id.as_str()),
        format!("Resolve decision: **{decision}**"),
    ];
    if let Some(pr) = resolve.get("pr_id").and_then(|v| v.as_str()) {
        lines.push(format!("Bound PR id: `{pr}` — reuse; do not open a second PR."));
    }
    if decision == "needs_choice" {
        lines.push(
            "Operator must pick a PR (Present modal). After ACK, call \
             resolve_coding_target({choice}) then continue."
                .into(),
        );
    }
    lines.push("### Required tool order".into());
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
         never merge unless operator asks."
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
}
