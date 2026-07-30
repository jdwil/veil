//! Domain types.

#![allow(unused_imports)]

use crate::domain::messages::*;
use crate::ports::{DomainError, ValidationError};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

/// Store: ConnectionStore
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConnectionStore {
    pub status: String,
    pub url: String,
}

impl ConnectionStore {
    pub fn new(status: String, url: String) -> Self {
        Self { status, url }
    }
}

/// Component: Sidebar
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sidebar {}

/// Component: StatusBar
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatusBar {
    pub status: String,
}

impl StatusBar {
    pub fn new(status: String) -> Self {
        Self { status }
    }
}

impl From<StatusBar> for String {
    fn from(v: StatusBar) -> String {
        v.status
    }
}

impl From<String> for StatusBar {
    fn from(s: String) -> Self {
        Self { status: s }
    }
}

/// Component: StatCard
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatCard {
    pub value: String,
    pub label: String,
}

impl StatCard {
    pub fn new(value: String, label: String) -> Self {
        Self { value, label }
    }
}

/// Component: DashboardView
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DashboardView {
    pub repos: Vec<serde_json::Value>,
    pub loading: bool,
    pub error: String,
    pub project_href_tpl: String,
}

impl DashboardView {
    pub fn new(error: String, project_href_tpl: String) -> Self {
        Self {
            repos: Vec::new(),
            loading: false,
            error,
            project_href_tpl,
        }
    }
}

/// Component: ProjectsView
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectsView {
    pub repos: Vec<serde_json::Value>,
    pub loading: bool,
    pub error: String,
    pub project_href_tpl: String,
    pub delete_open: bool,
    pub deleting_id: String,
    pub delete_name: String,
    pub delete_busy: bool,
}

impl ProjectsView {
    pub fn new(
        error: String,
        project_href_tpl: String,
        deleting_id: String,
        delete_name: String,
    ) -> Self {
        Self {
            repos: Vec::new(),
            loading: false,
            error,
            project_href_tpl,
            delete_open: false,
            deleting_id,
            delete_name,
            delete_busy: false,
        }
    }
}

/// Component: ProjectCreateView
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectCreateView {
    pub name: String,
    pub description: String,
    pub submitting: bool,
    pub error: String,
}

impl ProjectCreateView {
    pub fn new(name: String, description: String, error: String) -> Self {
        Self {
            name,
            description,
            submitting: false,
            error,
        }
    }
}

/// Component: ProjectDetailView
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectDetailView {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub description: String,
    pub default_branch: String,
    pub created_at: String,
    pub updated_at: String,
    pub loading: bool,
    pub error: String,
    pub loaded: bool,
    pub delete_open: bool,
    pub delete_busy: bool,
    pub environment: String,
    pub env_options: Vec<String>,
    pub env_role_hint: String,
    pub infra: serde_json::Value,
    pub units: Vec<serde_json::Value>,
    pub has_deploy: bool,
    pub has_toml: bool,
    pub region: String,
    pub project_prefix: String,
    pub network_vpc: String,
    pub toml_path: String,
    pub provision_busy: bool,
    pub provision_msg: String,
    pub provision_job_id: String,
    pub provision_percent: i64,
    pub provision_status: String,
    pub provision_steps: Vec<serde_json::Value>,
    pub plan_open: bool,
    pub plan_loading: bool,
    pub plan_summary: String,
    pub plan_mock: bool,
    pub plan_resources: Vec<serde_json::Value>,
    pub plan_steps: Vec<serde_json::Value>,
    pub plan_notes: Vec<String>,
    pub plan_diff: serde_json::Value,
    pub plan_cta: String,
    pub aws_inventory: Vec<serde_json::Value>,
    pub stack_base: String,
    pub provision_cta: String,
    pub any_provisioned: bool,
    pub infra_status: String,
    pub infra_status_variant: String,
}

impl ProjectDetailView {
    pub fn new(
        id: String,
        name: String,
        slug: String,
        description: String,
        default_branch: String,
        error: String,
        environment: String,
        env_role_hint: String,
        region: String,
        project_prefix: String,
        network_vpc: String,
        toml_path: String,
        provision_msg: String,
        provision_job_id: String,
        provision_status: String,
        plan_summary: String,
        plan_cta: String,
        stack_base: String,
        provision_cta: String,
        infra_status: String,
        infra_status_variant: String,
    ) -> Self {
        Self {
            id,
            name,
            slug,
            description,
            default_branch,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            loading: false,
            error,
            loaded: false,
            delete_open: false,
            delete_busy: false,
            environment,
            env_options: Vec::new(),
            env_role_hint,
            infra: serde_json::json!({}),
            units: Vec::new(),
            has_deploy: false,
            has_toml: false,
            region,
            project_prefix,
            network_vpc,
            toml_path,
            provision_busy: false,
            provision_msg,
            provision_job_id,
            provision_percent: 0,
            provision_status,
            provision_steps: Vec::new(),
            plan_open: false,
            plan_loading: false,
            plan_summary,
            plan_mock: false,
            plan_resources: Vec::new(),
            plan_steps: Vec::new(),
            plan_notes: Vec::new(),
            plan_diff: serde_json::json!({}),
            plan_cta,
            aws_inventory: Vec::new(),
            stack_base,
            provision_cta,
            any_provisioned: false,
            infra_status,
            infra_status_variant,
        }
    }
}

impl ProjectDetailView {
    pub fn select_environment(
        &mut self,
        next: String,
    ) -> Result<Vec<ProjectDetailViewEvent>, DomainError> {
        let mut events: Vec<ProjectDetailViewEvent> = Vec::new();
        if next != "".to_string() {
            if next != &self.environment {
                self.environment = next;
                self.provision_msg = "".to_string();
                self.loaded = false;
            };
        };
        Ok(events)
    }

    pub fn load_provision_plan(&mut self) -> Result<Vec<ProjectDetailViewEvent>, DomainError> {
        let mut events: Vec<ProjectDetailViewEvent> = Vec::new();
        self.plan_loading = true;
        self.plan_open = true;
        self.plan_summary = "".to_string();
        self.plan_resources = vec![];
        self.plan_steps = vec![];
        self.plan_notes = vec![];
        self.plan_diff = serde_json::json!({});
        self.plan_mock = false;
        self.error = "".to_string();
        let mut plan = api_client_mutate(
            "/api/plan-provision".to_string(),
            serde_json::json!({ "project_slug": self.slug.clone(), "environment": self.environment.clone(), "repo_id": self.id.clone(), "branch": self.default_branch.clone() }),
        );
        apply_plan(plan.clone());
        self.plan_loading = false;
        Ok(events)
    }
}

/// Component: DeployView
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeployView {
    pub artifact_id: String,
    pub target_type: String,
    pub deploying: bool,
    pub error: String,
}

impl DeployView {
    pub fn new(artifact_id: String, target_type: String, error: String) -> Self {
        Self {
            artifact_id,
            target_type,
            deploying: false,
            error,
        }
    }
}

impl DeployView {
    pub fn trigger_deploy(&mut self) -> Result<Vec<DeployViewEvent>, DomainError> {
        let mut events: Vec<DeployViewEvent> = Vec::new();
        self.deploying = true;
        self.error = "".to_string();
        api_client_mutate(
            "/api/deploy".to_string(),
            serde_json::json!({ "artifact_id": self.artifact_id.clone(), "target": self.target_type.clone() }),
        );
        self.deploying = false;
        Ok(events)
    }
}

/// Component: RegistryView
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RegistryView {
    pub layers: Vec<serde_json::Value>,
    pub stubs: Vec<serde_json::Value>,
    pub loading: bool,
    pub error: String,
}

impl RegistryView {
    pub fn new(error: String) -> Self {
        Self {
            layers: Vec::new(),
            stubs: Vec::new(),
            loading: false,
            error,
        }
    }
}

/// Component: BusView
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BusView {
    pub test_message: String,
    pub result: Option<String>,
    pub sending: bool,
    pub error: String,
}

impl BusView {
    pub fn new(test_message: String, error: String) -> Self {
        Self {
            test_message,
            result: None,
            sending: false,
            error,
        }
    }
}

impl BusView {
    pub fn send_test(&mut self) -> Result<Vec<BusViewEvent>, DomainError> {
        let mut events: Vec<BusViewEvent> = Vec::new();
        self.sending = true;
        self.error = "".to_string();
        let resp = api_client_mutate(
            "/api/bus/invoke".to_string(),
            serde_json::json!({ "message": self.test_message.clone() }),
        );
        self.result = resp.result;
        self.sending = false;
        Ok(events)
    }
}

/// Component: AgentsView
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentsView {
    pub messages: Vec<serde_json::Value>,
    pub user_input: String,
    pub sending: bool,
    pub error: String,
}

impl AgentsView {
    pub fn new(user_input: String, error: String) -> Self {
        Self {
            messages: Vec::new(),
            user_input,
            sending: false,
            error,
        }
    }
}

impl AgentsView {
    pub fn send_message(&mut self) -> Result<Vec<AgentsViewEvent>, DomainError> {
        let mut events: Vec<AgentsViewEvent> = Vec::new();
        self.sending = true;
        self.error = "".to_string();
        self.messages = self.messages.concat(vec![
            serde_json::json!({ "role": "user".to_string(), "content": self.user_input.clone() }),
        ]);
        let resp = api_client_mutate(
            "/api/agent".to_string(),
            serde_json::json!({ "message": self.user_input.clone() }),
        );
        self.messages = self.messages.concat(vec![serde_json::json!({ "role": "assistant".to_string(), "content": serde_json::json!(resp.clone())["message"].clone() })]);
        self.user_input = "".to_string();
        self.sending = false;
        Ok(events)
    }
}

/// Component: ConfigView
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfigView {
    pub aws_region: String,
    pub s3_bucket: String,
    pub ddb_table: String,
    pub llm_model: String,
    pub saving: bool,
    pub error: String,
}

impl ConfigView {
    pub fn new(
        aws_region: String,
        s3_bucket: String,
        ddb_table: String,
        llm_model: String,
        error: String,
    ) -> Self {
        Self {
            aws_region,
            s3_bucket,
            ddb_table,
            llm_model,
            saving: false,
            error,
        }
    }
}

impl ConfigView {
    pub fn save(&mut self) -> Result<Vec<ConfigViewEvent>, DomainError> {
        let mut events: Vec<ConfigViewEvent> = Vec::new();
        self.saving = true;
        self.error = "".to_string();
        api_client_mutate(
            "/api/config".to_string(),
            serde_json::json!({ "aws_region": self.aws_region.clone(), "s3_bucket": self.s3_bucket.clone(), "ddb_table": self.ddb_table.clone(), "llm_model": self.llm_model.clone() }),
        );
        self.saving = false;
        Ok(events)
    }
}

/// Layout: AppLayout
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppLayout {}

/// Page: Dashboard
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Dashboard {}

/// Page: Projects
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Projects {}

/// Page: ProjectCreate
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectCreate {}

/// Page: ProjectDetail
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectDetail {}

/// Page: Deploy
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Deploy {}

/// Page: Registry
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Registry {}

/// Page: Bus
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Bus {}

/// Page: Agents
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Agents {}

/// Page: Config
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Config {}
