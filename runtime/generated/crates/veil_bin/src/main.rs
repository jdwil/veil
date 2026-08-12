//! HTTP harness for package `VeilRuntime` (RT-001 / RT-003).
//! Wires adapters + exposes services as REST endpoints.
//! `cargo run -p veil_bin` from the generated workspace root.

use agent::application::{self as agent_app, Deps as agent_Deps};
use axum::{
    Json, Router,
    extract::Query,
    extract::Request,
    extract::State,
    http::{HeaderMap, StatusCode},
    middleware::{Next, from_fn},
    response::Response,
    routing::{get, post, put},
};
use change_management::application::{
    self as change_management_app, Deps as change_management_Deps,
};
use daemon::application::{self as daemon_app, Deps as daemon_Deps};
use deploy::application::{self as deploy_app, Deps as deploy_Deps};
use exec::application::{self as exec_app};
use extensions::application::{self as extensions_app, Deps as extensions_Deps};
use meta_execution::application::{self as meta_execution_app, Deps as meta_execution_Deps};
use serde_json::Value;
use std::sync::Arc;
use storage::application::{self as storage_app, Deps as storage_Deps};
use tools::application::{self as tools_app, Deps as tools_Deps};
use tower_http::cors::{Any, CorsLayer};
use uuid::Uuid;
use veil_shared::*;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(3000);

    let bus = veil_shared::InProcessBus::new();

    // ── context Storage ──
    // stub harness_field S3Client
    let _stub_s3_client = {
        let conf = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        let conf = aws_sdk_s3::config::Builder::from(&conf);
        let conf = if let Ok(ep) = std::env::var("AWS_ENDPOINT_URL") {
            conf.endpoint_url(ep).build()
        } else {
            conf.build()
        };
        aws_sdk_s3::Client::from_conf(conf)
    };

    // stub harness_field DdbClient
    let _stub_ddb_client = {
        let conf = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        let conf = aws_sdk_dynamodb::config::Builder::from(&conf);
        let conf = if let Ok(ep) =
            std::env::var("AWS_ENDPOINT_URL").or_else(|_| std::env::var("DYNAMO_ENDPOINT"))
        {
            conf.endpoint_url(ep).build()
        } else {
            conf.build()
        };
        aws_sdk_dynamodb::Client::from_conf(conf)
    };

    let s3_object_storage_inst: Arc<dyn storage::ports::ObjectStorage + Send + Sync> =
        Arc::new(storage::adapters::S3ObjectStorage {
            bucket: std::env::var("BUCKET").unwrap_or_else(|_| "default".into()),
            client: _stub_s3_client.clone(),
        });
    let ddb_metadata_store_inst: Arc<dyn storage::ports::MetadataStore + Send + Sync> =
        Arc::new(storage::adapters::DdbMetadataStore {
            client: _stub_ddb_client.clone(),
            table: std::env::var("VEIL_DDB_TABLE")
                .or_else(|_| std::env::var("TABLE"))
                .unwrap_or_else(|_| "veil".into()),
        });
    let storage_deps = Arc::new(storage_Deps {
        object_storage: s3_object_storage_inst.clone(),
        metadata_store: ddb_metadata_store_inst.clone(),
    });

    let storage_router = Router::new()
        .route(
            "/api/branches",
            post(storage_create_branch_handler).get(storage_list_branches_handler),
        )
        .route("/api/commit_logs", get(storage_get_commit_log_handler))
        .route("/api/compile", post(storage_compile_handler))
        .route("/api/deploy", post(storage_deploy_handler))
        .route("/api/diffs", get(storage_get_diff_handler))
        .route("/api/files", get(storage_list_files_handler))
        .route(
            "/api/project_infras/{id}",
            get(storage_get_project_infra_handler),
        )
        .route(
            "/api/query-project-modules",
            post(storage_query_project_modules_handler),
        )
        .route("/api/read-file", post(storage_read_file_handler))
        .route(
            "/api/repos",
            post(storage_create_repo_handler).get(storage_list_repos_handler),
        )
        .route(
            "/api/repos/{id}",
            get(storage_get_repo_handler).delete(storage_delete_repo_handler),
        )
        .route(
            "/api/sync-repo-to-object-store",
            post(storage_sync_repo_to_object_store_handler),
        )
        .route("/api/write-file", post(storage_write_file_handler))
        .layer(from_fn(veil_api_key_middleware))
        .layer(veil_cors_layer())
        .with_state(storage_deps.clone());

    // ── context Tools ──
    let tools_deps = Arc::new(tools_Deps {
        bus: Arc::new(bus.clone()),
    });

    let tools_router = Router::new()
        .route("/api/branch_tools", post(tools_create_branch_tool_handler))
        .route("/api/branches_tools", get(tools_list_branches_tool_handler))
        .route("/api/compile-tool", post(tools_compile_tool_handler))
        .route("/api/deploy-tool", post(tools_deploy_tool_handler))
        .route("/api/diff-tool", post(tools_diff_tool_handler))
        .route("/api/files_tools", get(tools_list_files_tool_handler))
        .route("/api/log-tool", post(tools_log_tool_handler))
        .route(
            "/api/propose-reaction-graph-tool",
            post(tools_propose_reaction_graph_tool_handler),
        )
        .route("/api/read-file-tool", post(tools_read_file_tool_handler))
        .route("/api/repo_tools", post(tools_create_repo_tool_handler))
        .route("/api/repos_tools", get(tools_list_repos_tool_handler))
        .route(
            "/api/validate-reaction-palette-tool",
            post(tools_validate_reaction_palette_tool_handler),
        )
        .route("/api/write-file-tool", post(tools_write_file_tool_handler))
        .layer(from_fn(veil_api_key_middleware))
        .layer(veil_cors_layer())
        .with_state(tools_deps.clone());

    // ── context Daemon ──
    let daemon_deps = Arc::new(daemon_Deps {
        bus: Arc::new(bus.clone()),
    });

    let daemon_router = Router::new()
        .route(
            "/api/handle-agent-message",
            post(daemon_handle_agent_message_handler),
        )
        .route(
            "/api/handle-connection",
            post(daemon_handle_connection_handler),
        )
        .route(
            "/api/handle-tool-call",
            post(daemon_handle_tool_call_handler),
        )
        .route("/api/health-check", post(daemon_health_check_handler))
        .route("/api/load-config", post(daemon_load_config_handler))
        .layer(from_fn(veil_api_key_middleware))
        .layer(veil_cors_layer())
        .with_state(daemon_deps.clone());

    // ── context Exec ──
    let bus_auth_adapter_inst: Arc<dyn exec::ports::AuthService + Send + Sync> =
        Arc::new(exec::adapters::BusAuthAdapter {});
    let exec_router = Router::new()
        .route("/api/load-env-config", post(exec_load_env_config_handler))
        .route("/api/parse-manifest", post(exec_parse_manifest_handler))
        .route(
            "/api/read-all-manifests",
            post(exec_read_all_manifests_handler),
        )
        .route(
            "/api/run-security-scan",
            post(exec_run_security_scan_handler),
        )
        .route("/api/start-harness", post(exec_start_harness_handler))
        .route("/api/wire-application", post(exec_wire_application_handler))
        .layer(from_fn(veil_api_key_middleware))
        .layer(veil_cors_layer())
        .with_state(());

    // ── context Extensions ──
    let file_extension_registry_inst: Arc<dyn extensions::ports::ExtensionRegistry + Send + Sync> =
        Arc::new(extensions::adapters::FileExtensionRegistry {
            dir: std::env::var("VEIL_EXTENSIONS_DIR").unwrap_or_else(|_| "default".into()),
        });
    let file_extension_source_store_inst: Arc<
        dyn extensions::ports::ExtensionSourceStore + Send + Sync,
    > = Arc::new(extensions::adapters::FileExtensionSourceStore {
        dir: std::env::var("VEIL_EXTENSIONS_DIR").unwrap_or_else(|_| "default".into()),
    });
    let file_extension_artifact_store_inst: Arc<
        dyn extensions::ports::ExtensionArtifactStore + Send + Sync,
    > = Arc::new(extensions::adapters::FileExtensionArtifactStore {
        dir: std::env::var("VEIL_EXTENSIONS_DIR").unwrap_or_else(|_| "default".into()),
    });
    let file_extension_executor_inst: Arc<dyn extensions::ports::ExtensionExecutor + Send + Sync> =
        Arc::new(extensions::adapters::FileExtensionExecutor {
            dir: std::env::var("VEIL_EXTENSIONS_DIR").unwrap_or_else(|_| "default".into()),
        });
    let extensions_deps = Arc::new(extensions_Deps {
        extension_registry: file_extension_registry_inst.clone(),
        extension_source_store: file_extension_source_store_inst.clone(),
        extension_artifact_store: file_extension_artifact_store_inst.clone(),
        extension_executor: file_extension_executor_inst.clone(),
    });

    let extensions_router = Router::new()
        .route(
            "/api/ensure-stock-catalog",
            post(extensions_ensure_stock_catalog_handler),
        )
        .route(
            "/api/extension_versions",
            get(extensions_list_extension_versions_handler),
        )
        .route(
            "/api/extension_versions/{id}",
            get(extensions_get_extension_version_handler),
        )
        .route(
            "/api/extensions",
            post(extensions_create_extension_handler).get(extensions_list_extensions_handler),
        )
        .route(
            "/api/extensions/{id}",
            get(extensions_get_extension_handler),
        )
        .route(
            "/api/extensions_by_scopes",
            get(extensions_list_extensions_by_scope_handler),
        )
        .route(
            "/api/fork-extension",
            post(extensions_fork_extension_handler),
        )
        .route(
            "/api/invoke-extension",
            post(extensions_invoke_extension_handler),
        )
        .route(
            "/api/mount-ui-extension",
            post(extensions_mount_ui_extension_handler),
        )
        .route(
            "/api/promote-extension",
            post(extensions_promote_extension_handler),
        )
        .route(
            "/api/publish-extension",
            post(extensions_publish_extension_handler),
        )
        .route(
            "/api/save-extension-version",
            post(extensions_save_extension_version_handler),
        )
        .route(
            "/api/stock_extensions",
            get(extensions_list_stock_extensions_handler),
        )
        .route(
            "/api/upsert-stock-extension",
            post(extensions_upsert_stock_extension_handler),
        )
        .route(
            "/api/validate-reaction-palette",
            post(extensions_validate_reaction_palette_handler),
        )
        .layer(from_fn(veil_api_key_middleware))
        .layer(veil_cors_layer())
        .with_state(extensions_deps.clone());

    // ── context MetaExecution ──
    // stub harness_field S3Client
    let _stub_s3_client = {
        let conf = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        let conf = aws_sdk_s3::config::Builder::from(&conf);
        let conf = if let Ok(ep) = std::env::var("AWS_ENDPOINT_URL") {
            conf.endpoint_url(ep).build()
        } else {
            conf.build()
        };
        aws_sdk_s3::Client::from_conf(conf)
    };

    let sandboxed_subprocess_runner_inst: Arc<
        dyn meta_execution::ports::SubprocessRunner + Send + Sync,
    > = Arc::new(meta_execution::adapters::SandboxedSubprocessRunner {});
    let s3_meta_object_store_inst: Arc<dyn meta_execution::ports::ObjectStorage + Send + Sync> =
        Arc::new(meta_execution::adapters::S3MetaObjectStore {
            bucket: std::env::var("VEIL_S3_BUCKET")
                .or_else(|_| std::env::var("BUCKET"))
                .unwrap_or_else(|_| "veil".into()),
            client: _stub_s3_client.clone(),
        });
    let s3_artifact_cache_inst: Arc<dyn meta_execution::ports::MetaArtifactCache + Send + Sync> =
        Arc::new(meta_execution::adapters::S3ArtifactCache {
            bucket: std::env::var("VEIL_S3_BUCKET")
                .or_else(|_| std::env::var("BUCKET"))
                .unwrap_or_else(|_| "veil".into()),
            client: _stub_s3_client.clone(),
        });
    let mock_meta_compiler_inst: Arc<
        dyn meta_execution::ports::MetaCompilationBackend + Send + Sync,
    > = Arc::new(meta_execution::adapters::MockMetaCompiler {});
    let meta_execution_deps = Arc::new(meta_execution_Deps {
        subprocess_runner: sandboxed_subprocess_runner_inst.clone(),
        objects: s3_meta_object_store_inst.clone(),
        cache: s3_artifact_cache_inst.clone(),
        compiler: mock_meta_compiler_inst.clone(),
        bus: Arc::new(bus.clone()),
    });

    let meta_execution_router = Router::new()
        .route(
            "/api/check-warm-status",
            post(meta_execution_check_warm_status_handler),
        )
        .route(
            "/api/ensure-compiled",
            post(meta_execution_ensure_compiled_handler),
        )
        .route(
            "/api/execute-meta-function",
            post(meta_execution_execute_meta_function_handler),
        )
        .route(
            "/api/execute-meta-layer-tool",
            post(meta_execution_execute_meta_layer_tool_handler),
        )
        .route(
            "/api/meta-layer-status-tool",
            post(meta_execution_meta_layer_status_tool_handler),
        )
        .route(
            "/api/resolve-content-hash",
            post(meta_execution_resolve_content_hash_handler),
        )
        .route(
            "/api/warm-function",
            post(meta_execution_warm_function_handler),
        )
        .route(
            "/api/warm-meta-layer-tool",
            post(meta_execution_warm_meta_layer_tool_handler),
        )
        .layer(from_fn(veil_api_key_middleware))
        .layer(veil_cors_layer())
        .with_state(meta_execution_deps.clone());

    // ── context Deploy ──
    // stub harness_field DdbClient
    let _stub_ddb_client = {
        let conf = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        let conf = aws_sdk_dynamodb::config::Builder::from(&conf);
        let conf = if let Ok(ep) =
            std::env::var("AWS_ENDPOINT_URL").or_else(|_| std::env::var("DYNAMO_ENDPOINT"))
        {
            conf.endpoint_url(ep).build()
        } else {
            conf.build()
        };
        aws_sdk_dynamodb::Client::from_conf(conf)
    };

    // stub harness_field S3Client
    let _stub_s3_client = {
        let conf = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        let conf = aws_sdk_s3::config::Builder::from(&conf);
        let conf = if let Ok(ep) = std::env::var("AWS_ENDPOINT_URL") {
            conf.endpoint_url(ep).build()
        } else {
            conf.build()
        };
        aws_sdk_s3::Client::from_conf(conf)
    };

    // stub harness_field LambdaClient
    let _stub_lambda_client = {
        let conf = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        let conf = aws_sdk_lambda::config::Builder::from(&conf);
        let conf = if let Ok(ep) = std::env::var("AWS_ENDPOINT_URL") {
            conf.endpoint_url(ep).build()
        } else {
            conf.build()
        };
        aws_sdk_lambda::Client::from_conf(conf)
    };

    // stub harness_field SqsClient
    let _stub_sqs_client = {
        let conf = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        let conf = aws_sdk_sqs::config::Builder::from(&conf);
        let conf = if let Ok(ep) = std::env::var("AWS_ENDPOINT_URL") {
            conf.endpoint_url(ep).build()
        } else {
            conf.build()
        };
        aws_sdk_sqs::Client::from_conf(conf)
    };

    // stub harness_field SnsClient
    let _stub_sns_client = {
        let conf = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        let conf = aws_sdk_sns::config::Builder::from(&conf);
        let conf = if let Ok(ep) = std::env::var("AWS_ENDPOINT_URL") {
            conf.endpoint_url(ep).build()
        } else {
            conf.build()
        };
        aws_sdk_sns::Client::from_conf(conf)
    };

    // stub harness_field ApigwClient
    let _stub_apigw_client = {
        let conf = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        let conf = aws_sdk_apigatewayv2::config::Builder::from(&conf).build();
        aws_sdk_apigatewayv2::Client::from_conf(conf)
    };

    let ddb_deployment_store_inst: Arc<dyn deploy::ports::DeploymentStateStore + Send + Sync> =
        Arc::new(deploy::adapters::DdbDeploymentStore {
            client: _stub_ddb_client.clone(),
            table: std::env::var("VEIL_DDB_TABLE")
                .or_else(|_| std::env::var("TABLE"))
                .unwrap_or_else(|_| "veil".into()),
        });
    let local_deploy_exec_inst: Arc<dyn deploy::ports::DeployExec + Send + Sync> =
        Arc::new(deploy::adapters::LocalDeployExec {
            apigw: _stub_apigw_client.clone(),
            bucket: std::env::var("BUCKET").unwrap_or_else(|_| "default".into()),
            ddb: _stub_ddb_client.clone(),
            lambda: _stub_lambda_client.clone(),
            s3: _stub_s3_client.clone(),
            sns: _stub_sns_client.clone(),
            sqs: _stub_sqs_client.clone(),
        });
    let mock_action_executor_inst: Arc<dyn deploy::ports::ActionExecutor + Send + Sync> =
        Arc::new(deploy::adapters::MockActionExecutor {});
    let deploy_deps = Arc::new(deploy_Deps {
        store: ddb_deployment_store_inst.clone(),
        exec: local_deploy_exec_inst.clone(),
        executor: mock_action_executor_inst.clone(),
        bus: Arc::new(bus.clone()),
    });

    let deploy_router = Router::new()
        .route(
            "/api/all_deployments",
            get(deploy_list_all_deployments_handler),
        )
        .route("/api/deploy-unit", post(deploy_deploy_unit_handler))
        .route("/api/deploy/deploy-tool", post(deploy_deploy_tool_handler))
        .route(
            "/api/deploy_environments",
            get(deploy_list_deploy_environments_handler),
        )
        .route(
            "/api/deployment-diff-tool",
            post(deploy_deployment_diff_tool_handler),
        )
        .route(
            "/api/deployment-status-tool",
            post(deploy_deployment_status_tool_handler),
        )
        .route(
            "/api/deployment_status",
            get(deploy_get_deployment_status_handler),
        )
        .route(
            "/api/deployments_tools",
            get(deploy_list_deployments_tool_handler),
        )
        .route("/api/plan-provision", post(deploy_plan_provision_handler))
        .route(
            "/api/provision-project",
            post(deploy_provision_project_handler),
        )
        .route("/api/provision_jobs", get(deploy_get_provision_job_handler))
        .route("/api/reconcile", post(deploy_reconcile_handler))
        .route(
            "/api/rollback-deployment",
            post(deploy_rollback_deployment_handler),
        )
        .route("/api/rollback-tool", post(deploy_rollback_tool_handler))
        .route("/api/scale-service", post(deploy_scale_service_handler))
        .route("/api/scale-tool", post(deploy_scale_tool_handler))
        .layer(from_fn(veil_api_key_middleware))
        .layer(veil_cors_layer())
        .with_state(deploy_deps.clone());

    // ── context PullRequestManagement ──
    // stub harness_field DdbClient
    let _stub_ddb_client = {
        let conf = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        let conf = aws_sdk_dynamodb::config::Builder::from(&conf);
        let conf = if let Ok(ep) =
            std::env::var("AWS_ENDPOINT_URL").or_else(|_| std::env::var("DYNAMO_ENDPOINT"))
        {
            conf.endpoint_url(ep).build()
        } else {
            conf.build()
        };
        aws_sdk_dynamodb::Client::from_conf(conf)
    };

    // stub harness_field S3Client
    let _stub_s3_client = {
        let conf = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        let conf = aws_sdk_s3::config::Builder::from(&conf);
        let conf = if let Ok(ep) = std::env::var("AWS_ENDPOINT_URL") {
            conf.endpoint_url(ep).build()
        } else {
            conf.build()
        };
        aws_sdk_s3::Client::from_conf(conf)
    };

    let ddb_pull_request_repo_inst: Arc<
        dyn change_management::ports::PullRequestRepo + Send + Sync,
    > = Arc::new(change_management::adapters::DdbPullRequestRepo {
        client: _stub_ddb_client.clone(),
        table: std::env::var("VEIL_DDB_TABLE")
            .or_else(|_| std::env::var("TABLE"))
            .unwrap_or_else(|_| "veil".into()),
    });
    let ddb_approval_repo_inst: Arc<dyn change_management::ports::ApprovalRepo + Send + Sync> =
        Arc::new(change_management::adapters::DdbApprovalRepo {
            client: _stub_ddb_client.clone(),
            table: std::env::var("VEIL_DDB_TABLE")
                .or_else(|_| std::env::var("TABLE"))
                .unwrap_or_else(|_| "veil".into()),
        });
    let ddb_ci_run_repo_inst: Arc<dyn change_management::ports::CiRunRepo + Send + Sync> =
        Arc::new(change_management::adapters::DdbCiRunRepo {
            client: _stub_ddb_client.clone(),
            table: std::env::var("VEIL_DDB_TABLE")
                .or_else(|_| std::env::var("TABLE"))
                .unwrap_or_else(|_| "veil".into()),
        });
    let ddb_comment_repo_inst: Arc<dyn change_management::ports::CommentRepo + Send + Sync> =
        Arc::new(change_management::adapters::DdbCommentRepo {
            client: _stub_ddb_client.clone(),
            table: std::env::var("VEIL_DDB_TABLE")
                .or_else(|_| std::env::var("TABLE"))
                .unwrap_or_else(|_| "veil".into()),
        });
    let s3_git_service_adapter_inst: Arc<dyn change_management::ports::GitService + Send + Sync> =
        Arc::new(change_management::adapters::S3GitServiceAdapter {
            bucket: std::env::var("BUCKET").unwrap_or_else(|_| "default".into()),
            s3: _stub_s3_client.clone(),
        });
    let change_management_deps = Arc::new(change_management_Deps {
        pr_repo: ddb_pull_request_repo_inst.clone(),
        approval_repo: ddb_approval_repo_inst.clone(),
        ci_repo: ddb_ci_run_repo_inst.clone(),
        comment_repo: ddb_comment_repo_inst.clone(),
        git: s3_git_service_adapter_inst.clone(),
    });

    let change_management_router = Router::new()
        .route(
            "/api/pull_requests",
            post(change_management_create_pull_request_flat_handler)
                .get(change_management_list_all_pull_requests_handler),
        )
        .route(
            "/api/pull_requests/{id}",
            get(change_management_get_pull_request_handler),
        )
        .route(
            "/api/pull_requests/{id}/approve",
            post(change_management_approve_pr_handler),
        )
        .route(
            "/api/pull_requests/{id}/comments",
            post(change_management_add_review_comment_handler),
        )
        .route(
            "/api/pull_requests/{id}/commit",
            post(change_management_commit_to_pr_handler),
        )
        .route(
            "/api/pull_requests/{id}/diff",
            get(change_management_get_structural_diff_handler),
        )
        .route(
            "/api/pull_requests/{id}/merge",
            post(change_management_merge_pr_handler),
        )
        .route(
            "/api/pull_requests/{id}/request-changes",
            post(change_management_request_changes_handler),
        )
        .route(
            "/api/pull_requests/{id}/status",
            put(change_management_update_pull_request_status_handler),
        )
        .route(
            "/api/pull_requests/{id}/submit",
            post(change_management_submit_for_review_handler),
        )
        .route(
            "/api/repos/{id}/pull_requests",
            post(change_management_create_pull_request_handler)
                .get(change_management_list_pull_requests_handler),
        )
        .layer(from_fn(veil_api_key_middleware))
        .layer(veil_cors_layer())
        .with_state(change_management_deps.clone());

    // ── context Agent ──
    let in_memory_acp_registry_inst: Arc<dyn agent::ports::AcpSessionRegistry + Send + Sync> =
        Arc::new(agent::adapters::InMemoryAcpRegistry {
            sessions: Default::default(),
        });
    let agent_deps = Arc::new(agent_Deps {
        registry: in_memory_acp_registry_inst.clone(),
        bus: Arc::new(bus.clone()),
    });

    let agent_router = Router::new()
        .route("/api/agent-status", post(agent_agent_status_handler))
        .route(
            "/api/execute-acp-turn",
            post(agent_execute_acp_turn_handler),
        )
        .route("/api/execute-tool", post(agent_execute_tool_handler))
        .route(
            "/api/resolve-provider",
            post(agent_resolve_provider_handler),
        )
        .route(
            "/api/send-tool-result",
            post(agent_send_tool_result_handler),
        )
        .route("/api/tool_registries", get(agent_get_tool_registry_handler))
        .route("/api/ws-agent-acp", post(agent_ws_agent_acp_handler))
        .route("/api/ws-agent-chat", post(agent_ws_agent_chat_handler))
        .layer(from_fn(veil_api_key_middleware))
        .layer(veil_cors_layer())
        .with_state(agent_deps.clone());

    // ── bus handlers (cross-context invoke / request) ──
    {
        let __deps = storage_deps.clone();
        bus.register("CreateRepo", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = storage_app::create_repo(
                    &__deps,
                    cmd.get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    serde_json::from_value(
                        cmd.get("description")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = storage_deps.clone();
        bus.register("ListRepos", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = storage_app::list_repos(&__deps).await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = storage_deps.clone();
        bus.register("GetRepo", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = storage_app::get_repo(
                    &__deps,
                    cmd.get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = storage_deps.clone();
        bus.register("DeleteRepo", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = storage_app::delete_repo(
                    &__deps,
                    cmd.get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = storage_deps.clone();
        bus.register("GetProjectInfra", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = storage_app::get_project_infra(
                    &__deps,
                    cmd.get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    serde_json::from_value(
                        cmd.get("environment")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = storage_deps.clone();
        bus.register("QueryProjectModules", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = storage_app::query_project_modules(
                    &__deps,
                    cmd.get("module")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("filters_json")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = storage_deps.clone();
        bus.register("SyncRepoToObjectStore", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = storage_app::sync_repo_to_object_store(
                    &__deps,
                    cmd.get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("branch")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = storage_deps.clone();
        bus.register("WriteFile", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = storage_app::write_file(
                    &__deps,
                    serde_json::from_value(
                        cmd.get("repo_id")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                    cmd.get("branch")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("path")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("content")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("message")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = storage_deps.clone();
        bus.register("ReadFile", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = storage_app::read_file(
                    &__deps,
                    serde_json::from_value(
                        cmd.get("repo_id")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                    cmd.get("branch")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("path")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = storage_deps.clone();
        bus.register("ListFiles", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = storage_app::list_files(
                    &__deps,
                    serde_json::from_value(
                        cmd.get("repo_id")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                    cmd.get("branch")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("prefix")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = storage_deps.clone();
        bus.register("CreateBranch", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = storage_app::create_branch(
                    &__deps,
                    serde_json::from_value(
                        cmd.get("repo_id")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                    cmd.get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("from_ref")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = storage_deps.clone();
        bus.register("ListBranches", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = storage_app::list_branches(
                    &__deps,
                    serde_json::from_value(
                        cmd.get("repo_id")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = storage_deps.clone();
        bus.register("GetDiff", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = storage_app::get_diff(
                    &__deps,
                    serde_json::from_value(
                        cmd.get("repo_id")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                    cmd.get("from_ref")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("to_ref")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = storage_deps.clone();
        bus.register("Compile", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = storage_app::compile(
                    &__deps,
                    serde_json::from_value(
                        cmd.get("repo_id")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                    cmd.get("branch")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    serde_json::from_value(
                        cmd.get("target")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = storage_deps.clone();
        bus.register("Deploy", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = storage_app::deploy(
                    &__deps,
                    serde_json::from_value(
                        cmd.get("artifact_id")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                    serde_json::from_value(
                        cmd.get("target")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = storage_deps.clone();
        bus.register("GetCommitLog", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = storage_app::get_commit_log(
                    &__deps,
                    serde_json::from_value(
                        cmd.get("repo_id")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                    serde_json::from_value(
                        cmd.get("branch")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                    cmd.get("limit").and_then(|v| v.as_i64()).unwrap_or(0),
                    cmd.get("offset").and_then(|v| v.as_i64()).unwrap_or(0),
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = tools_deps.clone();
        bus.register("CreateRepoTool", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = tools_app::create_repo_tool(
                    &__deps,
                    cmd.get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    serde_json::from_value(
                        cmd.get("description")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = tools_deps.clone();
        bus.register("WriteFileTool", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = tools_app::write_file_tool(
                    &__deps,
                    cmd.get("repo_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("branch")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("path")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("content")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("message")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = tools_deps.clone();
        bus.register("ReadFileTool", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = tools_app::read_file_tool(
                    &__deps,
                    cmd.get("repo_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("branch")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("path")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = tools_deps.clone();
        bus.register("ListFilesTool", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = tools_app::list_files_tool(
                    &__deps,
                    cmd.get("repo_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("branch")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    serde_json::from_value(
                        cmd.get("prefix")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = tools_deps.clone();
        bus.register("CreateBranchTool", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = tools_app::create_branch_tool(
                    &__deps,
                    cmd.get("repo_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("from_ref")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = tools_deps.clone();
        bus.register("ListBranchesTool", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = tools_app::list_branches_tool(
                    &__deps,
                    cmd.get("repo_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = tools_deps.clone();
        bus.register("DiffTool", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = tools_app::diff_tool(
                    &__deps,
                    cmd.get("repo_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("from_ref")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("to_ref")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = tools_deps.clone();
        bus.register("CompileTool", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = tools_app::compile_tool(
                    &__deps,
                    cmd.get("repo_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("branch")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("target")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = tools_deps.clone();
        bus.register("DeployTool", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = tools_app::deploy_tool(
                    &__deps,
                    cmd.get("artifact_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("target")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    serde_json::from_value(
                        cmd.get("tag").cloned().unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = tools_deps.clone();
        bus.register("ListReposTool", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = tools_app::list_repos_tool(&__deps).await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = tools_deps.clone();
        bus.register("LogTool", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = tools_app::log_tool(
                    &__deps,
                    cmd.get("repo_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    serde_json::from_value(
                        cmd.get("branch")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                    serde_json::from_value(
                        cmd.get("limit").cloned().unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = tools_deps.clone();
        bus.register("ValidateReactionPaletteTool", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = tools_app::validate_reaction_palette_tool(
                    &__deps,
                    serde_json::from_value(
                        cmd.get("node_kinds")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = tools_deps.clone();
        bus.register("ProposeReactionGraphTool", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = tools_app::propose_reaction_graph_tool(
                    &__deps,
                    serde_json::from_value(
                        cmd.get("node_kinds")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        bus.register("LoadConfig", move |cmd| async move {
            let __result = daemon_app::load_config().await?;
            Ok(serde_json::to_value(__result).map_err(|e| DomainError::External(e.to_string()))?)
        });
    }
    {
        let __deps = daemon_deps.clone();
        bus.register("Connection", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = daemon_app::handle_connection(
                    &__deps,
                    cmd.get("msg")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        bus.register("AgentMessage", move |cmd| async move {
            let __result = daemon_app::handle_agent_message(
                cmd.get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
            )
            .await?;
            Ok(serde_json::to_value(__result).map_err(|e| DomainError::External(e.to_string()))?)
        });
    }
    {
        let __deps = daemon_deps.clone();
        bus.register("ToolCall", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = daemon_app::handle_tool_call(
                    &__deps,
                    cmd.get("tool")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("args").cloned().unwrap_or(serde_json::Value::Null),
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        bus.register("HealthCheck", move |cmd| async move {
            let __result = daemon_app::health_check().await?;
            Ok(serde_json::to_value(__result).map_err(|e| DomainError::External(e.to_string()))?)
        });
    }
    {
        bus.register("ParseManifest", move |cmd| async move {
            let __result = exec_app::parse_manifest(
                cmd.get("json")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
            )
            .await?;
            Ok(serde_json::to_value(__result).map_err(|e| DomainError::External(e.to_string()))?)
        });
    }
    {
        bus.register("ReadAllManifests", move |cmd| async move {
            let __result = exec_app::read_all_manifests(
                cmd.get("workspace_dir")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
            )
            .await?;
            Ok(serde_json::to_value(__result).map_err(|e| DomainError::External(e.to_string()))?)
        });
    }
    {
        bus.register("LoadEnvConfig", move |cmd| async move {
            let __result = exec_app::load_env_config(
                serde_json::from_value(
                    cmd.get("manifests")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null),
                )
                .map_err(|e| DomainError::External(e.to_string()))?,
            )
            .await?;
            Ok(serde_json::to_value(__result).map_err(|e| DomainError::External(e.to_string()))?)
        });
    }
    {
        bus.register("WireApplication", move |cmd| async move {
            let __result = exec_app::wire_application(
                serde_json::from_value(
                    cmd.get("manifests")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null),
                )
                .map_err(|e| DomainError::External(e.to_string()))?,
                serde_json::from_value(cmd.get("env").cloned().unwrap_or(serde_json::Value::Null))
                    .map_err(|e| DomainError::External(e.to_string()))?,
            )
            .await?;
            Ok(serde_json::to_value(__result).map_err(|e| DomainError::External(e.to_string()))?)
        });
    }
    {
        bus.register("RunSecurityScan", move |cmd| async move {
            let __result = exec_app::run_security_scan(
                cmd.get("workspace_dir")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                serde_json::from_value(
                    cmd.get("config")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null),
                )
                .map_err(|e| DomainError::External(e.to_string()))?,
            )
            .await?;
            Ok(serde_json::to_value(__result).map_err(|e| DomainError::External(e.to_string()))?)
        });
    }
    {
        bus.register("StartHarness", move |cmd| async move {
            let __result =
                exec_app::start_harness(cmd.get("port").and_then(|v| v.as_i64()).unwrap_or(0))
                    .await?;
            Ok(serde_json::to_value(__result).map_err(|e| DomainError::External(e.to_string()))?)
        });
    }
    {
        let __deps = extensions_deps.clone();
        bus.register("CreateExtension", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = extensions_app::create_extension(
                    &__deps,
                    cmd.get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("kind")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("scope")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("provenance")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    serde_json::from_value(
                        cmd.get("product_id")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                    serde_json::from_value(
                        cmd.get("tenant_id")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                    serde_json::from_value(
                        cmd.get("initiative_id")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                    serde_json::from_value(
                        cmd.get("description")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                    serde_json::from_value(
                        cmd.get("params_schema")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = extensions_deps.clone();
        bus.register("ListExtensions", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = extensions_app::list_extensions(
                    &__deps,
                    serde_json::from_value(
                        cmd.get("scope").cloned().unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                    serde_json::from_value(
                        cmd.get("kind").cloned().unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                    serde_json::from_value(
                        cmd.get("product_id")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                    serde_json::from_value(
                        cmd.get("tenant_id")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = extensions_deps.clone();
        bus.register("GetExtension", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = extensions_app::get_extension(
                    &__deps,
                    serde_json::from_value(
                        cmd.get("id").cloned().unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = extensions_deps.clone();
        bus.register("ListExtensionVersions", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = extensions_app::list_extension_versions(
                    &__deps,
                    serde_json::from_value(
                        cmd.get("id").cloned().unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = extensions_deps.clone();
        bus.register("GetExtensionVersion", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = extensions_app::get_extension_version(
                    &__deps,
                    serde_json::from_value(
                        cmd.get("id").cloned().unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                    cmd.get("version").and_then(|v| v.as_i64()).unwrap_or(0),
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = extensions_deps.clone();
        bus.register("SaveExtensionVersion", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = extensions_app::save_extension_version(
                    &__deps,
                    serde_json::from_value(
                        cmd.get("extension_id")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                    cmd.get("source_commit")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("artifact_uris")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null),
                    serde_json::from_value(
                        cmd.get("changelog")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = extensions_deps.clone();
        bus.register("PublishExtension", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = extensions_app::publish_extension(
                    &__deps,
                    serde_json::from_value(
                        cmd.get("id").cloned().unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = extensions_deps.clone();
        bus.register("InvokeExtension", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = extensions_app::invoke_extension(
                    &__deps,
                    serde_json::from_value(
                        cmd.get("extension_id")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                    cmd.get("version").and_then(|v| v.as_i64()).unwrap_or(0),
                    cmd.get("kind")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("params")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null),
                    cmd.get("context")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null),
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = extensions_deps.clone();
        bus.register("ListStockExtensions", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = extensions_app::list_stock_extensions(
                    &__deps,
                    serde_json::from_value(
                        cmd.get("scope").cloned().unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                    serde_json::from_value(
                        cmd.get("kind").cloned().unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                    serde_json::from_value(
                        cmd.get("product_id")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = extensions_deps.clone();
        bus.register("UpsertStockExtension", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = extensions_app::upsert_stock_extension(
                    &__deps,
                    serde_json::from_value(
                        cmd.get("extension_id")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                    cmd.get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("description")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    serde_json::from_value(
                        cmd.get("product_id")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                    serde_json::from_value(
                        cmd.get("params_schema")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                    serde_json::from_value(
                        cmd.get("capabilities")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                    serde_json::from_value(
                        cmd.get("palette_layer_refs")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = extensions_deps.clone();
        bus.register("EnsureStockCatalog", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = extensions_app::ensure_stock_catalog(
                    &__deps,
                    serde_json::from_value(
                        cmd.get("activate_id")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                    serde_json::from_value(
                        cmd.get("guard_id")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                    serde_json::from_value(
                        cmd.get("product_id")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = extensions_deps.clone();
        bus.register("ForkExtension", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = extensions_app::fork_extension(
                    &__deps,
                    serde_json::from_value(
                        cmd.get("source_id")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                    cmd.get("source_version")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0),
                    cmd.get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    serde_json::from_value(
                        cmd.get("tenant_id")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                    serde_json::from_value(
                        cmd.get("initiative_id")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = extensions_deps.clone();
        bus.register("ListExtensionsByScope", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = extensions_app::list_extensions_by_scope(
                    &__deps,
                    cmd.get("scope")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    serde_json::from_value(
                        cmd.get("kind").cloned().unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                    serde_json::from_value(
                        cmd.get("product_id")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                    serde_json::from_value(
                        cmd.get("tenant_id")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = extensions_deps.clone();
        bus.register("PromoteExtension", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = extensions_app::promote_extension(
                    &__deps,
                    serde_json::from_value(
                        cmd.get("extension_id")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                    cmd.get("target_scope")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("allow_promote")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false),
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        bus.register("ValidateReactionPalette", move |cmd| async move {
            let __result = extensions_app::validate_reaction_palette(
                serde_json::from_value(
                    cmd.get("node_kinds")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null),
                )
                .map_err(|e| DomainError::External(e.to_string()))?,
            )
            .await?;
            Ok(serde_json::to_value(__result).map_err(|e| DomainError::External(e.to_string()))?)
        });
    }
    {
        bus.register("MountUiExtension", move |cmd| async move {
            let __result = extensions_app::mount_ui_extension(
                serde_json::from_value(
                    cmd.get("extension_id")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null),
                )
                .map_err(|e| DomainError::External(e.to_string()))?,
                cmd.get("version").and_then(|v| v.as_i64()).unwrap_or(0),
                cmd.get("slot")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                cmd.get("props").cloned().unwrap_or(serde_json::Value::Null),
            )
            .await?;
            Ok(serde_json::to_value(__result).map_err(|e| DomainError::External(e.to_string()))?)
        });
    }
    {
        let __deps = meta_execution_deps.clone();
        bus.register("ResolveContentHash", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = meta_execution_app::resolve_content_hash(
                    &__deps,
                    serde_json::from_value(
                        cmd.get("function_id")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = meta_execution_deps.clone();
        bus.register("EnsureCompiled", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = meta_execution_app::ensure_compiled(
                    &__deps,
                    serde_json::from_value(
                        cmd.get("function_id")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                    cmd.get("content_hash")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = meta_execution_deps.clone();
        bus.register("ExecuteMetaFunction", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = meta_execution_app::execute_meta_function(
                    &__deps,
                    serde_json::from_value(
                        cmd.get("function_id")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                    cmd.get("input").cloned().unwrap_or(serde_json::Value::Null),
                    serde_json::from_value(
                        cmd.get("capabilities")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                    serde_json::from_value(
                        cmd.get("timeout_ms")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                    serde_json::from_value(
                        cmd.get("idempotency_key")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = meta_execution_deps.clone();
        bus.register("WarmFunction", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = meta_execution_app::warm_function(
                    &__deps,
                    serde_json::from_value(
                        cmd.get("function_id")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = meta_execution_deps.clone();
        bus.register("CheckWarmStatus", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = meta_execution_app::check_warm_status(
                    &__deps,
                    serde_json::from_value(
                        cmd.get("function_id")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = meta_execution_deps.clone();
        bus.register("ExecuteMetaLayerTool", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = meta_execution_app::execute_meta_layer_tool(
                    &__deps,
                    cmd.get("tenant_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("function_path")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    serde_json::from_value(
                        cmd.get("version")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                    cmd.get("input").cloned().unwrap_or(serde_json::Value::Null),
                    serde_json::from_value(
                        cmd.get("timeout_ms")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = meta_execution_deps.clone();
        bus.register("WarmMetaLayerTool", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = meta_execution_app::warm_meta_layer_tool(
                    &__deps,
                    cmd.get("tenant_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("function_path")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    serde_json::from_value(
                        cmd.get("version")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = meta_execution_deps.clone();
        bus.register("MetaLayerStatusTool", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = meta_execution_app::meta_layer_status_tool(
                    &__deps,
                    cmd.get("tenant_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("function_path")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    serde_json::from_value(
                        cmd.get("version")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = deploy_deps.clone();
        bus.register("Reconcile", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = deploy_app::reconcile(
                    &__deps,
                    serde_json::from_value(
                        cmd.get("desired")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                    cmd.get("environment")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    serde_json::from_value(
                        cmd.get("new_artifact_hash")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = deploy_deps.clone();
        bus.register("DeployUnit", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = deploy_app::deploy_unit(
                    &__deps,
                    serde_json::from_value(
                        cmd.get("desired")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                    cmd.get("environment")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("artifact_hash")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("actor")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    serde_json::from_value(
                        cmd.get("message")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = deploy_deps.clone();
        bus.register("RollbackDeployment", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = deploy_app::rollback_deployment(
                    &__deps,
                    cmd.get("environment")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("unit_name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    serde_json::from_value(
                        cmd.get("target_version")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                    cmd.get("actor")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    serde_json::from_value(
                        cmd.get("reason")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = deploy_deps.clone();
        bus.register("GetDeploymentStatus", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = deploy_app::get_deployment_status(
                    &__deps,
                    cmd.get("environment")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("unit_name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = deploy_deps.clone();
        bus.register("ListAllDeployments", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = deploy_app::list_all_deployments(
                    &__deps,
                    serde_json::from_value(
                        cmd.get("environment")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                    serde_json::from_value(
                        cmd.get("project")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = deploy_deps.clone();
        bus.register("ListDeployEnvironments", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = deploy_app::list_deploy_environments(&__deps).await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = deploy_deps.clone();
        bus.register("PlanProvision", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = deploy_app::plan_provision(
                    &__deps,
                    cmd.get("project_slug")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("environment")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("repo_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("branch")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = deploy_deps.clone();
        bus.register("ProvisionProject", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = deploy_app::provision_project(
                    &__deps,
                    cmd.get("project_slug")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("environment")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("repo_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("branch")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = deploy_deps.clone();
        bus.register("GetProvisionJob", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = deploy_app::get_provision_job(
                    &__deps,
                    cmd.get("job_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = deploy_deps.clone();
        bus.register("ScaleService", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = deploy_app::scale_service(
                    &__deps,
                    cmd.get("environment")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("unit_name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("desired_count")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0),
                    cmd.get("actor")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    serde_json::from_value(
                        cmd.get("reason")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = deploy_deps.clone();
        bus.register("RollbackTool", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = deploy_app::rollback_tool(
                    &__deps,
                    cmd.get("unit_name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("environment")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    serde_json::from_value(
                        cmd.get("target_version")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                    serde_json::from_value(
                        cmd.get("reason")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = deploy_deps.clone();
        bus.register("DeploymentStatusTool", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = deploy_app::deployment_status_tool(
                    &__deps,
                    cmd.get("unit_name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("environment")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = deploy_deps.clone();
        bus.register("ListDeploymentsTool", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = deploy_app::list_deployments_tool(
                    &__deps,
                    serde_json::from_value(
                        cmd.get("environment")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                    serde_json::from_value(
                        cmd.get("project")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = deploy_deps.clone();
        bus.register("DeploymentDiffTool", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = deploy_app::deployment_diff_tool(
                    &__deps,
                    cmd.get("unit_name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("environment")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = deploy_deps.clone();
        bus.register("ScaleTool", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = deploy_app::scale_tool(
                    &__deps,
                    cmd.get("unit_name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("environment")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("desired_count")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0),
                    serde_json::from_value(
                        cmd.get("reason")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = change_management_deps.clone();
        bus.register("CreatePullRequest", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = change_management_app::create_pull_request(
                    &__deps,
                    serde_json::from_value(
                        cmd.get("id").cloned().unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                    cmd.get("slug")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("title")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("description")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("jira_ticket")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("author")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = change_management_deps.clone();
        bus.register("CreatePullRequestFlat", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = change_management_app::create_pull_request_flat(
                    &__deps,
                    serde_json::from_value(
                        cmd.get("repo_id")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                    cmd.get("slug")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("title")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("description")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("jira_ticket")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("author")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = change_management_deps.clone();
        bus.register("CommitToChange", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = change_management_app::commit_to_pr(
                    &__deps,
                    serde_json::from_value(
                        cmd.get("id").cloned().unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                    cmd.get("slug")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("path")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("content")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("message")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("author")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = change_management_deps.clone();
        bus.register("SubmitForReview", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = change_management_app::submit_for_review(
                    &__deps,
                    serde_json::from_value(
                        cmd.get("id").cloned().unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = change_management_deps.clone();
        bus.register("ApproveChange", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = change_management_app::approve_pr(
                    &__deps,
                    serde_json::from_value(
                        cmd.get("id").cloned().unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                    cmd.get("reviewer")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    serde_json::from_value(
                        cmd.get("comment")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = change_management_deps.clone();
        bus.register("RequestChanges", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = change_management_app::request_pr_changes(
                    &__deps,
                    serde_json::from_value(
                        cmd.get("id").cloned().unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                    cmd.get("reviewer")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("comment")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = change_management_deps.clone();
        bus.register("MergeChange", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = change_management_app::merge_pr(
                    &__deps,
                    serde_json::from_value(
                        cmd.get("id").cloned().unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                    cmd.get("merger")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("slug")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = change_management_deps.clone();
        bus.register("GetStructuralDiff", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = change_management_app::get_structural_diff(
                    &__deps,
                    serde_json::from_value(
                        cmd.get("id").cloned().unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                    cmd.get("slug")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        bus.register("ComputeStructuralDiffFromSource", move |cmd| async move {
            let __result = change_management_app::compute_structural_diff_from_source(
                cmd.get("base_content")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                cmd.get("branch_content")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                cmd.get("base_label")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                cmd.get("head_label")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
            )
            .await?;
            Ok(serde_json::to_value(__result).map_err(|e| DomainError::External(e.to_string()))?)
        });
    }
    {
        let __deps = change_management_deps.clone();
        bus.register("AddReviewComment", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = change_management_app::add_review_comment(
                    &__deps,
                    serde_json::from_value(
                        cmd.get("id").cloned().unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                    cmd.get("author")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    serde_json::from_value(
                        cmd.get("construct_path")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                    cmd.get("body")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = change_management_deps.clone();
        bus.register("AddComment", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = change_management_app::add_comment(
                    &__deps,
                    serde_json::from_value(
                        cmd.get("pr_id").cloned().unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                    cmd.get("author")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    serde_json::from_value(
                        cmd.get("construct_path")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                    cmd.get("body")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = change_management_deps.clone();
        bus.register("ListPullRequests", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = change_management_app::list_pull_requests(
                    &__deps,
                    serde_json::from_value(
                        cmd.get("id").cloned().unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                    serde_json::from_value(
                        cmd.get("status")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = change_management_deps.clone();
        bus.register("ListAllPullRequests", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = change_management_app::list_all_pull_requests(
                    &__deps,
                    serde_json::from_value(
                        cmd.get("status")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = change_management_deps.clone();
        bus.register("GetPullRequest", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = change_management_app::get_pull_request(
                    &__deps,
                    serde_json::from_value(
                        cmd.get("id").cloned().unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = change_management_deps.clone();
        bus.register("UpdatePullRequestStatus", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = change_management_app::update_pull_request_status(
                    &__deps,
                    serde_json::from_value(
                        cmd.get("id").cloned().unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                    serde_json::from_value(
                        cmd.get("status")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        bus.register("ResolveProvider", move |cmd| async move {
            let __result = agent_app::resolve_provider().await?;
            Ok(serde_json::to_value(__result).map_err(|e| DomainError::External(e.to_string()))?)
        });
    }
    {
        bus.register("GetToolRegistry", move |cmd| async move {
            let __result = agent_app::get_tool_registry().await?;
            Ok(serde_json::to_value(__result).map_err(|e| DomainError::External(e.to_string()))?)
        });
    }
    {
        bus.register("ExecuteTool", move |cmd| async move {
            let __result = agent_app::execute_tool(
                cmd.get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                cmd.get("args").cloned().unwrap_or(serde_json::Value::Null),
                serde_json::from_value(cmd.get("ctx").cloned().unwrap_or(serde_json::Value::Null))
                    .map_err(|e| DomainError::External(e.to_string()))?,
            )
            .await?;
            Ok(serde_json::to_value(__result).map_err(|e| DomainError::External(e.to_string()))?)
        });
    }
    {
        let __deps = agent_deps.clone();
        bus.register("ExecuteAcpTurn", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = agent_app::execute_acp_turn(
                    &__deps,
                    cmd.get("user_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("turn_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    serde_json::from_value(
                        cmd.get("messages")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                    serde_json::from_value(
                        cmd.get("tools").cloned().unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|e| DomainError::External(e.to_string()))?,
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = agent_deps.clone();
        bus.register("SendToolResult", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = agent_app::send_tool_result(
                    &__deps,
                    cmd.get("user_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("turn_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("call_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cmd.get("output")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null),
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        let __deps = agent_deps.clone();
        bus.register("WsAgentChat", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = agent_app::ws_agent_chat(
                    &__deps,
                    cmd.get("msg")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                )
                .await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }
    {
        bus.register("WsAgentAcp", move |cmd| async move {
            let __result = agent_app::ws_agent_acp(
                cmd.get("msg")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
            )
            .await?;
            Ok(serde_json::to_value(__result).map_err(|e| DomainError::External(e.to_string()))?)
        });
    }
    {
        let __deps = agent_deps.clone();
        bus.register("AgentStatus", move |cmd| {
            let __deps = __deps.clone();
            async move {
                let __result = agent_app::agent_status(&__deps).await?;
                Ok(serde_json::to_value(__result)
                    .map_err(|e| DomainError::External(e.to_string()))?)
            }
        });
    }

    let app = storage_router
        .merge(tools_router)
        .merge(daemon_router)
        .merge(exec_router)
        .merge(extensions_router)
        .merge(meta_execution_router)
        .merge(deploy_router)
        .merge(change_management_router)
        .merge(agent_router)
        .route("/health", get(|| async { "ok" }));
    println!("veil_bin: listening on :{}", port);
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port)).await?;
    axum::serve(listener, app.into_make_service()).await?;
    Ok(())
}

async fn storage_create_repo_handler(
    State(deps): State<Arc<storage_Deps>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let name = body
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let description = {
        let __v = body.get("description").cloned().unwrap_or(Value::Null);
        let __v = if matches!(&__v, Value::String(s) if s.is_empty()) {
            Value::Null
        } else {
            __v
        };
        serde_json::from_value(__v).map_err(|_| StatusCode::BAD_REQUEST)?
    };
    match storage_app::create_repo(&deps, name, description).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn storage_list_repos_handler(
    State(deps): State<Arc<storage_Deps>>,
) -> Result<Json<Value>, StatusCode> {
    match storage_app::list_repos(&deps).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn storage_get_repo_handler(
    State(deps): State<Arc<storage_Deps>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<Value>, StatusCode> {
    match storage_app::get_repo(&deps, id).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn storage_delete_repo_handler(
    State(deps): State<Arc<storage_Deps>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<Value>, StatusCode> {
    match storage_app::delete_repo(&deps, id).await {
        Ok(_) => Ok(Json(serde_json::json!({"ok": true}))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn storage_get_project_infra_handler(
    State(deps): State<Arc<storage_Deps>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Value>, StatusCode> {
    let environment = q.get("environment").filter(|s| !s.is_empty()).cloned();
    match storage_app::get_project_infra(&deps, id, environment).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn storage_query_project_modules_handler(
    State(deps): State<Arc<storage_Deps>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let module = body
        .get("module")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let filters_json = body
        .get("filters_json")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    match storage_app::query_project_modules(&deps, module, filters_json).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn storage_sync_repo_to_object_store_handler(
    State(deps): State<Arc<storage_Deps>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let id = body
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let branch = body
        .get("branch")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    match storage_app::sync_repo_to_object_store(&deps, id, branch).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn storage_write_file_handler(
    State(deps): State<Arc<storage_Deps>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let repo_id = serde_json::from_value(body.get("repo_id").cloned().unwrap_or(Value::Null))
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    let branch = body
        .get("branch")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let path = body
        .get("path")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let content = body
        .get("content")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let message = body
        .get("message")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    match storage_app::write_file(&deps, repo_id, branch, path, content, message).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn storage_read_file_handler(
    State(deps): State<Arc<storage_Deps>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let repo_id = serde_json::from_value(body.get("repo_id").cloned().unwrap_or(Value::Null))
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    let branch = body
        .get("branch")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let path = body
        .get("path")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    match storage_app::read_file(&deps, repo_id, branch, path).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn storage_list_files_handler(
    State(deps): State<Arc<storage_Deps>>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Value>, StatusCode> {
    let repo_id = q
        .get("repo_id")
        .and_then(|s| serde_json::from_str(s).ok())
        .ok_or(StatusCode::BAD_REQUEST)?;
    let branch = q.get("branch").cloned().unwrap_or_default();
    let prefix = q.get("prefix").cloned().unwrap_or_default();
    match storage_app::list_files(&deps, repo_id, branch, prefix).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn storage_create_branch_handler(
    State(deps): State<Arc<storage_Deps>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let repo_id = serde_json::from_value(body.get("repo_id").cloned().unwrap_or(Value::Null))
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    let name = body
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let from_ref = body
        .get("from_ref")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    match storage_app::create_branch(&deps, repo_id, name, from_ref).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn storage_list_branches_handler(
    State(deps): State<Arc<storage_Deps>>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Value>, StatusCode> {
    let repo_id = q
        .get("repo_id")
        .and_then(|s| serde_json::from_str(s).ok())
        .ok_or(StatusCode::BAD_REQUEST)?;
    match storage_app::list_branches(&deps, repo_id).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn storage_get_diff_handler(
    State(deps): State<Arc<storage_Deps>>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Value>, StatusCode> {
    let repo_id = q
        .get("repo_id")
        .and_then(|s| serde_json::from_str(s).ok())
        .ok_or(StatusCode::BAD_REQUEST)?;
    let from_ref = q.get("from_ref").cloned().unwrap_or_default();
    let to_ref = q.get("to_ref").cloned().unwrap_or_default();
    match storage_app::get_diff(&deps, repo_id, from_ref, to_ref).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn storage_compile_handler(
    State(deps): State<Arc<storage_Deps>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let repo_id = serde_json::from_value(body.get("repo_id").cloned().unwrap_or(Value::Null))
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    let branch = body
        .get("branch")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let target = serde_json::from_value(body.get("target").cloned().unwrap_or(Value::Null))
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    match storage_app::compile(&deps, repo_id, branch, target).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn storage_deploy_handler(
    State(deps): State<Arc<storage_Deps>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let artifact_id =
        serde_json::from_value(body.get("artifact_id").cloned().unwrap_or(Value::Null))
            .map_err(|_| StatusCode::BAD_REQUEST)?;
    let target = serde_json::from_value(body.get("target").cloned().unwrap_or(Value::Null))
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    match storage_app::deploy(&deps, artifact_id, target).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn storage_get_commit_log_handler(
    State(deps): State<Arc<storage_Deps>>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Value>, StatusCode> {
    let repo_id = q
        .get("repo_id")
        .and_then(|s| serde_json::from_str(s).ok())
        .ok_or(StatusCode::BAD_REQUEST)?;
    let branch = q.get("branch").filter(|s| !s.is_empty()).cloned();
    let limit = q
        .get("limit")
        .and_then(|s| s.parse::<i64>().ok())
        .ok_or(StatusCode::BAD_REQUEST)?;
    let offset = q
        .get("offset")
        .and_then(|s| s.parse::<i64>().ok())
        .ok_or(StatusCode::BAD_REQUEST)?;
    match storage_app::get_commit_log(&deps, repo_id, branch, limit, offset).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn tools_create_repo_tool_handler(
    State(deps): State<Arc<tools_Deps>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let name = body
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let description = {
        let __v = body.get("description").cloned().unwrap_or(Value::Null);
        let __v = if matches!(&__v, Value::String(s) if s.is_empty()) {
            Value::Null
        } else {
            __v
        };
        serde_json::from_value(__v).map_err(|_| StatusCode::BAD_REQUEST)?
    };
    match tools_app::create_repo_tool(&deps, name, description).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn tools_write_file_tool_handler(
    State(deps): State<Arc<tools_Deps>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let repo_id = body
        .get("repo_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let branch = body
        .get("branch")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let path = body
        .get("path")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let content = body
        .get("content")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let message = body
        .get("message")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    match tools_app::write_file_tool(&deps, repo_id, branch, path, content, message).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn tools_read_file_tool_handler(
    State(deps): State<Arc<tools_Deps>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let repo_id = body
        .get("repo_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let branch = body
        .get("branch")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let path = body
        .get("path")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    match tools_app::read_file_tool(&deps, repo_id, branch, path).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn tools_list_files_tool_handler(
    State(deps): State<Arc<tools_Deps>>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Value>, StatusCode> {
    let repo_id = q.get("repo_id").cloned().unwrap_or_default();
    let branch = q.get("branch").cloned().unwrap_or_default();
    let prefix = q.get("prefix").filter(|s| !s.is_empty()).cloned();
    match tools_app::list_files_tool(&deps, repo_id, branch, prefix).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn tools_create_branch_tool_handler(
    State(deps): State<Arc<tools_Deps>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let repo_id = body
        .get("repo_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let name = body
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let from_ref = body
        .get("from_ref")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    match tools_app::create_branch_tool(&deps, repo_id, name, from_ref).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn tools_list_branches_tool_handler(
    State(deps): State<Arc<tools_Deps>>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Value>, StatusCode> {
    let repo_id = q.get("repo_id").cloned().unwrap_or_default();
    match tools_app::list_branches_tool(&deps, repo_id).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn tools_diff_tool_handler(
    State(deps): State<Arc<tools_Deps>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let repo_id = body
        .get("repo_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let from_ref = body
        .get("from_ref")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let to_ref = body
        .get("to_ref")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    match tools_app::diff_tool(&deps, repo_id, from_ref, to_ref).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn tools_compile_tool_handler(
    State(deps): State<Arc<tools_Deps>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let repo_id = body
        .get("repo_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let branch = body
        .get("branch")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let target = body
        .get("target")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    match tools_app::compile_tool(&deps, repo_id, branch, target).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn tools_deploy_tool_handler(
    State(deps): State<Arc<tools_Deps>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let artifact_id = body
        .get("artifact_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let target = body
        .get("target")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let tag = {
        let __v = body.get("tag").cloned().unwrap_or(Value::Null);
        let __v = if matches!(&__v, Value::String(s) if s.is_empty()) {
            Value::Null
        } else {
            __v
        };
        serde_json::from_value(__v).map_err(|_| StatusCode::BAD_REQUEST)?
    };
    match tools_app::deploy_tool(&deps, artifact_id, target, tag).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn tools_list_repos_tool_handler(
    State(deps): State<Arc<tools_Deps>>,
) -> Result<Json<Value>, StatusCode> {
    match tools_app::list_repos_tool(&deps).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn tools_log_tool_handler(
    State(deps): State<Arc<tools_Deps>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let repo_id = body
        .get("repo_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let branch = {
        let __v = body.get("branch").cloned().unwrap_or(Value::Null);
        let __v = if matches!(&__v, Value::String(s) if s.is_empty()) {
            Value::Null
        } else {
            __v
        };
        serde_json::from_value(__v).map_err(|_| StatusCode::BAD_REQUEST)?
    };
    let limit = {
        let __v = body.get("limit").cloned().unwrap_or(Value::Null);
        let __v = if matches!(&__v, Value::String(s) if s.is_empty()) {
            Value::Null
        } else {
            __v
        };
        serde_json::from_value(__v).map_err(|_| StatusCode::BAD_REQUEST)?
    };
    match tools_app::log_tool(&deps, repo_id, branch, limit).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn tools_validate_reaction_palette_tool_handler(
    State(deps): State<Arc<tools_Deps>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let node_kinds = serde_json::from_value(body.get("node_kinds").cloned().unwrap_or(Value::Null))
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    match tools_app::validate_reaction_palette_tool(&deps, node_kinds).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn tools_propose_reaction_graph_tool_handler(
    State(deps): State<Arc<tools_Deps>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let node_kinds = serde_json::from_value(body.get("node_kinds").cloned().unwrap_or(Value::Null))
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    match tools_app::propose_reaction_graph_tool(&deps, node_kinds).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn daemon_load_config_handler(
    State(deps): State<Arc<daemon_Deps>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    match daemon_app::load_config().await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn daemon_handle_connection_handler(
    State(deps): State<Arc<daemon_Deps>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let msg = body
        .get("msg")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    match daemon_app::handle_connection(&deps, msg).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn daemon_handle_agent_message_handler(
    State(deps): State<Arc<daemon_Deps>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let message = body
        .get("message")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    match daemon_app::handle_agent_message(message).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn daemon_handle_tool_call_handler(
    State(deps): State<Arc<daemon_Deps>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let tool = body
        .get("tool")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let args = serde_json::from_value(body.get("args").cloned().unwrap_or(Value::Null))
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    match daemon_app::handle_tool_call(&deps, tool, args).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn daemon_health_check_handler(
    State(deps): State<Arc<daemon_Deps>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    match daemon_app::health_check().await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn exec_parse_manifest_handler(Json(body): Json<Value>) -> Result<Json<Value>, StatusCode> {
    let json = body
        .get("json")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    match exec_app::parse_manifest(json).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn exec_read_all_manifests_handler(
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let workspace_dir = body
        .get("workspace_dir")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    match exec_app::read_all_manifests(workspace_dir).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn exec_load_env_config_handler(Json(body): Json<Value>) -> Result<Json<Value>, StatusCode> {
    let manifests = serde_json::from_value(body.get("manifests").cloned().unwrap_or(Value::Null))
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    match exec_app::load_env_config(manifests).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn exec_wire_application_handler(Json(body): Json<Value>) -> Result<Json<Value>, StatusCode> {
    let manifests = serde_json::from_value(body.get("manifests").cloned().unwrap_or(Value::Null))
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    let env = serde_json::from_value(body.get("env").cloned().unwrap_or(Value::Null))
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    match exec_app::wire_application(manifests, env).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn exec_run_security_scan_handler(
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let workspace_dir = body
        .get("workspace_dir")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let config = serde_json::from_value(body.get("config").cloned().unwrap_or(Value::Null))
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    match exec_app::run_security_scan(workspace_dir, config).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn exec_start_harness_handler(Json(body): Json<Value>) -> Result<Json<Value>, StatusCode> {
    let port = serde_json::from_value(body.get("port").cloned().unwrap_or(Value::Null))
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    match exec_app::start_harness(port).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn extensions_create_extension_handler(
    State(deps): State<Arc<extensions_Deps>>,
    req_headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let name = body
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let kind = body
        .get("kind")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let scope = body
        .get("scope")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let provenance = body
        .get("provenance")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let product_id = {
        let __v = body.get("product_id").cloned().unwrap_or(Value::Null);
        let __v = if matches!(&__v, Value::String(s) if s.is_empty()) {
            Value::Null
        } else {
            __v
        };
        serde_json::from_value(__v).map_err(|_| StatusCode::BAD_REQUEST)?
    };
    let tenant_id = {
        let __v = body.get("tenant_id").cloned().unwrap_or(Value::Null);
        let __v = if matches!(&__v, Value::String(s) if s.is_empty()) {
            Value::Null
        } else {
            __v
        };
        serde_json::from_value(__v).map_err(|_| StatusCode::BAD_REQUEST)?
    };
    let initiative_id = {
        let __v = body.get("initiative_id").cloned().unwrap_or(Value::Null);
        let __v = if matches!(&__v, Value::String(s) if s.is_empty()) {
            Value::Null
        } else {
            __v
        };
        serde_json::from_value(__v).map_err(|_| StatusCode::BAD_REQUEST)?
    };
    let description = {
        let __v = body.get("description").cloned().unwrap_or(Value::Null);
        let __v = if matches!(&__v, Value::String(s) if s.is_empty()) {
            Value::Null
        } else {
            __v
        };
        serde_json::from_value(__v).map_err(|_| StatusCode::BAD_REQUEST)?
    };
    let params_schema = {
        let __v = body.get("params_schema").cloned().unwrap_or(Value::Null);
        let __v = if matches!(&__v, Value::String(s) if s.is_empty()) {
            Value::Null
        } else {
            __v
        };
        serde_json::from_value(__v).map_err(|_| StatusCode::BAD_REQUEST)?
    };
    match extensions_app::create_extension(
        &deps,
        name,
        kind,
        scope,
        provenance,
        product_id,
        tenant_id,
        initiative_id,
        description,
        params_schema,
    )
    .await
    {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn extensions_list_extensions_handler(
    State(deps): State<Arc<extensions_Deps>>,
    Query(q): Query<std::collections::HashMap<String, String>>,
    req_headers: HeaderMap,
) -> Result<Json<Value>, StatusCode> {
    let scope = q.get("scope").filter(|s| !s.is_empty()).cloned();
    let kind = q.get("kind").filter(|s| !s.is_empty()).cloned();
    let product_id = q.get("product_id").filter(|s| !s.is_empty()).cloned();
    let tenant_id = q
        .get("tenant_id")
        .filter(|s| !s.is_empty())
        .and_then(|s| serde_json::from_str(s).ok());
    match extensions_app::list_extensions(&deps, scope, kind, product_id, tenant_id).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn extensions_get_extension_handler(
    State(deps): State<Arc<extensions_Deps>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<Value>, StatusCode> {
    let id = id.parse::<Uuid>().map_err(|_| StatusCode::BAD_REQUEST)?;
    match extensions_app::get_extension(&deps, id).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn extensions_list_extension_versions_handler(
    State(deps): State<Arc<extensions_Deps>>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Value>, StatusCode> {
    let id = q
        .get("id")
        .and_then(|s| s.parse::<Uuid>().ok())
        .ok_or(StatusCode::BAD_REQUEST)?;
    match extensions_app::list_extension_versions(&deps, id).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn extensions_get_extension_version_handler(
    State(deps): State<Arc<extensions_Deps>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Value>, StatusCode> {
    let id = id.parse::<Uuid>().map_err(|_| StatusCode::BAD_REQUEST)?;
    let version = q
        .get("version")
        .and_then(|s| s.parse::<i64>().ok())
        .ok_or(StatusCode::BAD_REQUEST)?;
    match extensions_app::get_extension_version(&deps, id, version).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn extensions_save_extension_version_handler(
    State(deps): State<Arc<extensions_Deps>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let extension_id = body
        .get("extension_id")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<Uuid>().ok())
        .ok_or(StatusCode::BAD_REQUEST)?;
    let source_commit = body
        .get("source_commit")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let artifact_uris =
        serde_json::from_value(body.get("artifact_uris").cloned().unwrap_or(Value::Null))
            .map_err(|_| StatusCode::BAD_REQUEST)?;
    let changelog = {
        let __v = body.get("changelog").cloned().unwrap_or(Value::Null);
        let __v = if matches!(&__v, Value::String(s) if s.is_empty()) {
            Value::Null
        } else {
            __v
        };
        serde_json::from_value(__v).map_err(|_| StatusCode::BAD_REQUEST)?
    };
    match extensions_app::save_extension_version(
        &deps,
        extension_id,
        source_commit,
        artifact_uris,
        changelog,
    )
    .await
    {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn extensions_publish_extension_handler(
    State(deps): State<Arc<extensions_Deps>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let id = body
        .get("id")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<Uuid>().ok())
        .ok_or(StatusCode::BAD_REQUEST)?;
    match extensions_app::publish_extension(&deps, id).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn extensions_invoke_extension_handler(
    State(deps): State<Arc<extensions_Deps>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let extension_id = body
        .get("extension_id")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<Uuid>().ok())
        .ok_or(StatusCode::BAD_REQUEST)?;
    let version = serde_json::from_value(body.get("version").cloned().unwrap_or(Value::Null))
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    let kind = body
        .get("kind")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let params = serde_json::from_value(body.get("params").cloned().unwrap_or(Value::Null))
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    let context = serde_json::from_value(body.get("context").cloned().unwrap_or(Value::Null))
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    match extensions_app::invoke_extension(&deps, extension_id, version, kind, params, context)
        .await
    {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn extensions_list_stock_extensions_handler(
    State(deps): State<Arc<extensions_Deps>>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Value>, StatusCode> {
    let scope = q.get("scope").filter(|s| !s.is_empty()).cloned();
    let kind = q.get("kind").filter(|s| !s.is_empty()).cloned();
    let product_id = q.get("product_id").filter(|s| !s.is_empty()).cloned();
    match extensions_app::list_stock_extensions(&deps, scope, kind, product_id).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn extensions_upsert_stock_extension_handler(
    State(deps): State<Arc<extensions_Deps>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let extension_id = body
        .get("extension_id")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<Uuid>().ok())
        .ok_or(StatusCode::BAD_REQUEST)?;
    let name = body
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let description = body
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let product_id = {
        let __v = body.get("product_id").cloned().unwrap_or(Value::Null);
        let __v = if matches!(&__v, Value::String(s) if s.is_empty()) {
            Value::Null
        } else {
            __v
        };
        serde_json::from_value(__v).map_err(|_| StatusCode::BAD_REQUEST)?
    };
    let params_schema = {
        let __v = body.get("params_schema").cloned().unwrap_or(Value::Null);
        let __v = if matches!(&__v, Value::String(s) if s.is_empty()) {
            Value::Null
        } else {
            __v
        };
        serde_json::from_value(__v).map_err(|_| StatusCode::BAD_REQUEST)?
    };
    let capabilities =
        serde_json::from_value(body.get("capabilities").cloned().unwrap_or(Value::Null))
            .map_err(|_| StatusCode::BAD_REQUEST)?;
    let palette_layer_refs = serde_json::from_value(
        body.get("palette_layer_refs")
            .cloned()
            .unwrap_or(Value::Null),
    )
    .map_err(|_| StatusCode::BAD_REQUEST)?;
    match extensions_app::upsert_stock_extension(
        &deps,
        extension_id,
        name,
        description,
        product_id,
        params_schema,
        capabilities,
        palette_layer_refs,
    )
    .await
    {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn extensions_ensure_stock_catalog_handler(
    State(deps): State<Arc<extensions_Deps>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let activate_id = body
        .get("activate_id")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<Uuid>().ok())
        .ok_or(StatusCode::BAD_REQUEST)?;
    let guard_id = body
        .get("guard_id")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<Uuid>().ok())
        .ok_or(StatusCode::BAD_REQUEST)?;
    let product_id = {
        let __v = body.get("product_id").cloned().unwrap_or(Value::Null);
        let __v = if matches!(&__v, Value::String(s) if s.is_empty()) {
            Value::Null
        } else {
            __v
        };
        serde_json::from_value(__v).map_err(|_| StatusCode::BAD_REQUEST)?
    };
    match extensions_app::ensure_stock_catalog(&deps, activate_id, guard_id, product_id).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn extensions_fork_extension_handler(
    State(deps): State<Arc<extensions_Deps>>,
    req_headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let source_id = body
        .get("source_id")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<Uuid>().ok())
        .ok_or(StatusCode::BAD_REQUEST)?;
    let source_version =
        serde_json::from_value(body.get("source_version").cloned().unwrap_or(Value::Null))
            .map_err(|_| StatusCode::BAD_REQUEST)?;
    let name = body
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let tenant_id = {
        let __v = body.get("tenant_id").cloned().unwrap_or(Value::Null);
        let __v = if matches!(&__v, Value::String(s) if s.is_empty()) {
            Value::Null
        } else {
            __v
        };
        serde_json::from_value(__v).map_err(|_| StatusCode::BAD_REQUEST)?
    };
    let initiative_id = {
        let __v = body.get("initiative_id").cloned().unwrap_or(Value::Null);
        let __v = if matches!(&__v, Value::String(s) if s.is_empty()) {
            Value::Null
        } else {
            __v
        };
        serde_json::from_value(__v).map_err(|_| StatusCode::BAD_REQUEST)?
    };
    match extensions_app::fork_extension(
        &deps,
        source_id,
        source_version,
        name,
        tenant_id,
        initiative_id,
    )
    .await
    {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn extensions_list_extensions_by_scope_handler(
    State(deps): State<Arc<extensions_Deps>>,
    Query(q): Query<std::collections::HashMap<String, String>>,
    req_headers: HeaderMap,
) -> Result<Json<Value>, StatusCode> {
    let scope = q.get("scope").cloned().unwrap_or_default();
    let kind = q.get("kind").filter(|s| !s.is_empty()).cloned();
    let product_id = q.get("product_id").filter(|s| !s.is_empty()).cloned();
    let tenant_id = q
        .get("tenant_id")
        .filter(|s| !s.is_empty())
        .and_then(|s| serde_json::from_str(s).ok());
    match extensions_app::list_extensions_by_scope(&deps, scope, kind, product_id, tenant_id).await
    {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn extensions_promote_extension_handler(
    State(deps): State<Arc<extensions_Deps>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let extension_id = body
        .get("extension_id")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<Uuid>().ok())
        .ok_or(StatusCode::BAD_REQUEST)?;
    let target_scope = body
        .get("target_scope")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let allow_promote =
        serde_json::from_value(body.get("allow_promote").cloned().unwrap_or(Value::Null))
            .map_err(|_| StatusCode::BAD_REQUEST)?;
    match extensions_app::promote_extension(&deps, extension_id, target_scope, allow_promote).await
    {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn extensions_validate_reaction_palette_handler(
    State(deps): State<Arc<extensions_Deps>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let node_kinds = serde_json::from_value(body.get("node_kinds").cloned().unwrap_or(Value::Null))
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    match extensions_app::validate_reaction_palette(node_kinds).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn extensions_mount_ui_extension_handler(
    State(deps): State<Arc<extensions_Deps>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let extension_id = body
        .get("extension_id")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<Uuid>().ok())
        .ok_or(StatusCode::BAD_REQUEST)?;
    let version = serde_json::from_value(body.get("version").cloned().unwrap_or(Value::Null))
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    let slot = body
        .get("slot")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let props = serde_json::from_value(body.get("props").cloned().unwrap_or(Value::Null))
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    match extensions_app::mount_ui_extension(extension_id, version, slot, props).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn meta_execution_resolve_content_hash_handler(
    State(deps): State<Arc<meta_execution_Deps>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let function_id =
        serde_json::from_value(body.get("function_id").cloned().unwrap_or(Value::Null))
            .map_err(|_| StatusCode::BAD_REQUEST)?;
    match meta_execution_app::resolve_content_hash(&deps, function_id).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn meta_execution_ensure_compiled_handler(
    State(deps): State<Arc<meta_execution_Deps>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let function_id =
        serde_json::from_value(body.get("function_id").cloned().unwrap_or(Value::Null))
            .map_err(|_| StatusCode::BAD_REQUEST)?;
    let content_hash = body
        .get("content_hash")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    match meta_execution_app::ensure_compiled(&deps, function_id, content_hash).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn meta_execution_execute_meta_function_handler(
    State(deps): State<Arc<meta_execution_Deps>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let function_id =
        serde_json::from_value(body.get("function_id").cloned().unwrap_or(Value::Null))
            .map_err(|_| StatusCode::BAD_REQUEST)?;
    let input = serde_json::from_value(body.get("input").cloned().unwrap_or(Value::Null))
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    let capabilities =
        serde_json::from_value(body.get("capabilities").cloned().unwrap_or(Value::Null))
            .map_err(|_| StatusCode::BAD_REQUEST)?;
    let timeout_ms = {
        let __v = body.get("timeout_ms").cloned().unwrap_or(Value::Null);
        let __v = if matches!(&__v, Value::String(s) if s.is_empty()) {
            Value::Null
        } else {
            __v
        };
        serde_json::from_value(__v).map_err(|_| StatusCode::BAD_REQUEST)?
    };
    let idempotency_key = {
        let __v = body.get("idempotency_key").cloned().unwrap_or(Value::Null);
        let __v = if matches!(&__v, Value::String(s) if s.is_empty()) {
            Value::Null
        } else {
            __v
        };
        serde_json::from_value(__v).map_err(|_| StatusCode::BAD_REQUEST)?
    };
    match meta_execution_app::execute_meta_function(
        &deps,
        function_id,
        input,
        capabilities,
        timeout_ms,
        idempotency_key,
    )
    .await
    {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn meta_execution_warm_function_handler(
    State(deps): State<Arc<meta_execution_Deps>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let function_id =
        serde_json::from_value(body.get("function_id").cloned().unwrap_or(Value::Null))
            .map_err(|_| StatusCode::BAD_REQUEST)?;
    match meta_execution_app::warm_function(&deps, function_id).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn meta_execution_check_warm_status_handler(
    State(deps): State<Arc<meta_execution_Deps>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let function_id =
        serde_json::from_value(body.get("function_id").cloned().unwrap_or(Value::Null))
            .map_err(|_| StatusCode::BAD_REQUEST)?;
    match meta_execution_app::check_warm_status(&deps, function_id).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn meta_execution_execute_meta_layer_tool_handler(
    State(deps): State<Arc<meta_execution_Deps>>,
    req_headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let tenant_id = body
        .get("tenant_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let function_path = body
        .get("function_path")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let version = {
        let __v = body.get("version").cloned().unwrap_or(Value::Null);
        let __v = if matches!(&__v, Value::String(s) if s.is_empty()) {
            Value::Null
        } else {
            __v
        };
        serde_json::from_value(__v).map_err(|_| StatusCode::BAD_REQUEST)?
    };
    let input = serde_json::from_value(body.get("input").cloned().unwrap_or(Value::Null))
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    let timeout_ms = {
        let __v = body.get("timeout_ms").cloned().unwrap_or(Value::Null);
        let __v = if matches!(&__v, Value::String(s) if s.is_empty()) {
            Value::Null
        } else {
            __v
        };
        serde_json::from_value(__v).map_err(|_| StatusCode::BAD_REQUEST)?
    };
    match meta_execution_app::execute_meta_layer_tool(
        &deps,
        tenant_id,
        function_path,
        version,
        input,
        timeout_ms,
    )
    .await
    {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn meta_execution_warm_meta_layer_tool_handler(
    State(deps): State<Arc<meta_execution_Deps>>,
    req_headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let tenant_id = body
        .get("tenant_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let function_path = body
        .get("function_path")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let version = {
        let __v = body.get("version").cloned().unwrap_or(Value::Null);
        let __v = if matches!(&__v, Value::String(s) if s.is_empty()) {
            Value::Null
        } else {
            __v
        };
        serde_json::from_value(__v).map_err(|_| StatusCode::BAD_REQUEST)?
    };
    match meta_execution_app::warm_meta_layer_tool(&deps, tenant_id, function_path, version).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn meta_execution_meta_layer_status_tool_handler(
    State(deps): State<Arc<meta_execution_Deps>>,
    req_headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let tenant_id = body
        .get("tenant_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let function_path = body
        .get("function_path")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let version = {
        let __v = body.get("version").cloned().unwrap_or(Value::Null);
        let __v = if matches!(&__v, Value::String(s) if s.is_empty()) {
            Value::Null
        } else {
            __v
        };
        serde_json::from_value(__v).map_err(|_| StatusCode::BAD_REQUEST)?
    };
    match meta_execution_app::meta_layer_status_tool(&deps, tenant_id, function_path, version).await
    {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn deploy_reconcile_handler(
    State(deps): State<Arc<deploy_Deps>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let desired = serde_json::from_value(body.get("desired").cloned().unwrap_or(Value::Null))
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    let environment = body
        .get("environment")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let new_artifact_hash = {
        let __v = body
            .get("new_artifact_hash")
            .cloned()
            .unwrap_or(Value::Null);
        let __v = if matches!(&__v, Value::String(s) if s.is_empty()) {
            Value::Null
        } else {
            __v
        };
        serde_json::from_value(__v).map_err(|_| StatusCode::BAD_REQUEST)?
    };
    match deploy_app::reconcile(&deps, desired, environment, new_artifact_hash).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn deploy_deploy_unit_handler(
    State(deps): State<Arc<deploy_Deps>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let desired = serde_json::from_value(body.get("desired").cloned().unwrap_or(Value::Null))
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    let environment = body
        .get("environment")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let artifact_hash = body
        .get("artifact_hash")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let actor = body
        .get("actor")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let message = {
        let __v = body.get("message").cloned().unwrap_or(Value::Null);
        let __v = if matches!(&__v, Value::String(s) if s.is_empty()) {
            Value::Null
        } else {
            __v
        };
        serde_json::from_value(__v).map_err(|_| StatusCode::BAD_REQUEST)?
    };
    match deploy_app::deploy_unit(&deps, desired, environment, artifact_hash, actor, message).await
    {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn deploy_rollback_deployment_handler(
    State(deps): State<Arc<deploy_Deps>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let environment = body
        .get("environment")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let unit_name = body
        .get("unit_name")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let target_version = {
        let __v = body.get("target_version").cloned().unwrap_or(Value::Null);
        let __v = if matches!(&__v, Value::String(s) if s.is_empty()) {
            Value::Null
        } else {
            __v
        };
        serde_json::from_value(__v).map_err(|_| StatusCode::BAD_REQUEST)?
    };
    let actor = body
        .get("actor")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let reason = {
        let __v = body.get("reason").cloned().unwrap_or(Value::Null);
        let __v = if matches!(&__v, Value::String(s) if s.is_empty()) {
            Value::Null
        } else {
            __v
        };
        serde_json::from_value(__v).map_err(|_| StatusCode::BAD_REQUEST)?
    };
    match deploy_app::rollback_deployment(
        &deps,
        environment,
        unit_name,
        target_version,
        actor,
        reason,
    )
    .await
    {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn deploy_get_deployment_status_handler(
    State(deps): State<Arc<deploy_Deps>>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Value>, StatusCode> {
    let environment = q.get("environment").cloned().unwrap_or_default();
    let unit_name = q.get("unit_name").cloned().unwrap_or_default();
    match deploy_app::get_deployment_status(&deps, environment, unit_name).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn deploy_list_all_deployments_handler(
    State(deps): State<Arc<deploy_Deps>>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Value>, StatusCode> {
    let environment = q.get("environment").filter(|s| !s.is_empty()).cloned();
    let project = q.get("project").filter(|s| !s.is_empty()).cloned();
    match deploy_app::list_all_deployments(&deps, environment, project).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn deploy_list_deploy_environments_handler(
    State(deps): State<Arc<deploy_Deps>>,
) -> Result<Json<Value>, StatusCode> {
    match deploy_app::list_deploy_environments(&deps).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn deploy_plan_provision_handler(
    State(deps): State<Arc<deploy_Deps>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let project_slug = body
        .get("project_slug")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let environment = body
        .get("environment")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let repo_id = body
        .get("repo_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let branch = body
        .get("branch")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    match deploy_app::plan_provision(&deps, project_slug, environment, repo_id, branch).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn deploy_provision_project_handler(
    State(deps): State<Arc<deploy_Deps>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let project_slug = body
        .get("project_slug")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let environment = body
        .get("environment")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let repo_id = body
        .get("repo_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let branch = body
        .get("branch")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    match deploy_app::provision_project(&deps, project_slug, environment, repo_id, branch).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn deploy_get_provision_job_handler(
    State(deps): State<Arc<deploy_Deps>>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Value>, StatusCode> {
    let job_id = q.get("job_id").cloned().unwrap_or_default();
    match deploy_app::get_provision_job(&deps, job_id).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn deploy_scale_service_handler(
    State(deps): State<Arc<deploy_Deps>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let environment = body
        .get("environment")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let unit_name = body
        .get("unit_name")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let desired_count =
        serde_json::from_value(body.get("desired_count").cloned().unwrap_or(Value::Null))
            .map_err(|_| StatusCode::BAD_REQUEST)?;
    let actor = body
        .get("actor")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let reason = {
        let __v = body.get("reason").cloned().unwrap_or(Value::Null);
        let __v = if matches!(&__v, Value::String(s) if s.is_empty()) {
            Value::Null
        } else {
            __v
        };
        serde_json::from_value(__v).map_err(|_| StatusCode::BAD_REQUEST)?
    };
    match deploy_app::scale_service(&deps, environment, unit_name, desired_count, actor, reason)
        .await
    {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn deploy_deploy_tool_handler(
    State(deps): State<Arc<deploy_Deps>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let unit_name = body
        .get("unit_name")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let environment = body
        .get("environment")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let artifact_hash = {
        let __v = body.get("artifact_hash").cloned().unwrap_or(Value::Null);
        let __v = if matches!(&__v, Value::String(s) if s.is_empty()) {
            Value::Null
        } else {
            __v
        };
        serde_json::from_value(__v).map_err(|_| StatusCode::BAD_REQUEST)?
    };
    let message = {
        let __v = body.get("message").cloned().unwrap_or(Value::Null);
        let __v = if matches!(&__v, Value::String(s) if s.is_empty()) {
            Value::Null
        } else {
            __v
        };
        serde_json::from_value(__v).map_err(|_| StatusCode::BAD_REQUEST)?
    };
    match deploy_app::deploy_tool(&deps, unit_name, environment, artifact_hash, message).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn deploy_rollback_tool_handler(
    State(deps): State<Arc<deploy_Deps>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let unit_name = body
        .get("unit_name")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let environment = body
        .get("environment")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let target_version = {
        let __v = body.get("target_version").cloned().unwrap_or(Value::Null);
        let __v = if matches!(&__v, Value::String(s) if s.is_empty()) {
            Value::Null
        } else {
            __v
        };
        serde_json::from_value(__v).map_err(|_| StatusCode::BAD_REQUEST)?
    };
    let reason = {
        let __v = body.get("reason").cloned().unwrap_or(Value::Null);
        let __v = if matches!(&__v, Value::String(s) if s.is_empty()) {
            Value::Null
        } else {
            __v
        };
        serde_json::from_value(__v).map_err(|_| StatusCode::BAD_REQUEST)?
    };
    match deploy_app::rollback_tool(&deps, unit_name, environment, target_version, reason).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn deploy_deployment_status_tool_handler(
    State(deps): State<Arc<deploy_Deps>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let unit_name = body
        .get("unit_name")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let environment = body
        .get("environment")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    match deploy_app::deployment_status_tool(&deps, unit_name, environment).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn deploy_list_deployments_tool_handler(
    State(deps): State<Arc<deploy_Deps>>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Value>, StatusCode> {
    let environment = q.get("environment").filter(|s| !s.is_empty()).cloned();
    let project = q.get("project").filter(|s| !s.is_empty()).cloned();
    match deploy_app::list_deployments_tool(&deps, environment, project).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn deploy_deployment_diff_tool_handler(
    State(deps): State<Arc<deploy_Deps>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let unit_name = body
        .get("unit_name")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let environment = body
        .get("environment")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    match deploy_app::deployment_diff_tool(&deps, unit_name, environment).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn deploy_scale_tool_handler(
    State(deps): State<Arc<deploy_Deps>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let unit_name = body
        .get("unit_name")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let environment = body
        .get("environment")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let desired_count =
        serde_json::from_value(body.get("desired_count").cloned().unwrap_or(Value::Null))
            .map_err(|_| StatusCode::BAD_REQUEST)?;
    let reason = {
        let __v = body.get("reason").cloned().unwrap_or(Value::Null);
        let __v = if matches!(&__v, Value::String(s) if s.is_empty()) {
            Value::Null
        } else {
            __v
        };
        serde_json::from_value(__v).map_err(|_| StatusCode::BAD_REQUEST)?
    };
    match deploy_app::scale_tool(&deps, unit_name, environment, desired_count, reason).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn change_management_create_pull_request_handler(
    State(deps): State<Arc<change_management_Deps>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let id = id.parse::<Uuid>().map_err(|_| StatusCode::BAD_REQUEST)?;
    let slug = body
        .get("slug")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let title = body
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let description = body
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let jira_ticket = body
        .get("jira_ticket")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let author = body
        .get("author")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    match change_management_app::create_pull_request(
        &deps,
        id,
        slug,
        title,
        description,
        jira_ticket,
        author,
    )
    .await
    {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn change_management_create_pull_request_flat_handler(
    State(deps): State<Arc<change_management_Deps>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let repo_id = body
        .get("repo_id")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<Uuid>().ok())
        .ok_or(StatusCode::BAD_REQUEST)?;
    let slug = body
        .get("slug")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let title = body
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let description = body
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let jira_ticket = body
        .get("jira_ticket")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let author = body
        .get("author")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    match change_management_app::create_pull_request_flat(
        &deps,
        repo_id,
        slug,
        title,
        description,
        jira_ticket,
        author,
    )
    .await
    {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn change_management_commit_to_pr_handler(
    State(deps): State<Arc<change_management_Deps>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let id = id.parse::<Uuid>().map_err(|_| StatusCode::BAD_REQUEST)?;
    let slug = body
        .get("slug")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let path = body
        .get("path")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let content = body
        .get("content")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let message = body
        .get("message")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let author = body
        .get("author")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    match change_management_app::commit_to_pr(&deps, id, slug, path, content, message, author)
        .await
    {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn change_management_submit_for_review_handler(
    State(deps): State<Arc<change_management_Deps>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let id = id.parse::<Uuid>().map_err(|_| StatusCode::BAD_REQUEST)?;
    match change_management_app::submit_for_review(&deps, id).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn change_management_approve_pr_handler(
    State(deps): State<Arc<change_management_Deps>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let id = id.parse::<Uuid>().map_err(|_| StatusCode::BAD_REQUEST)?;
    let reviewer = body
        .get("reviewer")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let comment = {
        let __v = body.get("comment").cloned().unwrap_or(Value::Null);
        let __v = if matches!(&__v, Value::String(s) if s.is_empty()) {
            Value::Null
        } else {
            __v
        };
        serde_json::from_value(__v).map_err(|_| StatusCode::BAD_REQUEST)?
    };
    match change_management_app::approve_pr(&deps, id, reviewer, comment).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn change_management_request_changes_handler(
    State(deps): State<Arc<change_management_Deps>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let id = id.parse::<Uuid>().map_err(|_| StatusCode::BAD_REQUEST)?;
    let reviewer = body
        .get("reviewer")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let comment = body
        .get("comment")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    match change_management_app::request_pr_changes(&deps, id, reviewer, comment).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn change_management_merge_pr_handler(
    State(deps): State<Arc<change_management_Deps>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let id = id.parse::<Uuid>().map_err(|_| StatusCode::BAD_REQUEST)?;
    let merger = body
        .get("merger")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let slug = body
        .get("slug")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    match change_management_app::merge_pr(&deps, id, merger, slug).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn change_management_get_structural_diff_handler(
    State(deps): State<Arc<change_management_Deps>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Value>, StatusCode> {
    let id = id.parse::<Uuid>().map_err(|_| StatusCode::BAD_REQUEST)?;
    let slug = q.get("slug").cloned().unwrap_or_default();
    match change_management_app::get_structural_diff(&deps, id, slug).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn change_management_add_review_comment_handler(
    State(deps): State<Arc<change_management_Deps>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let id = id.parse::<Uuid>().map_err(|_| StatusCode::BAD_REQUEST)?;
    let author = body
        .get("author")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let construct_path = {
        let __v = body.get("construct_path").cloned().unwrap_or(Value::Null);
        let __v = if matches!(&__v, Value::String(s) if s.is_empty()) {
            Value::Null
        } else {
            __v
        };
        serde_json::from_value(__v).map_err(|_| StatusCode::BAD_REQUEST)?
    };
    let body = body
        .get("body")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    match change_management_app::add_review_comment(&deps, id, author, construct_path, body).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn change_management_list_pull_requests_handler(
    State(deps): State<Arc<change_management_Deps>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Value>, StatusCode> {
    let id = id.parse::<Uuid>().map_err(|_| StatusCode::BAD_REQUEST)?;
    let status = q
        .get("status")
        .filter(|s| !s.is_empty())
        .and_then(|s| serde_json::from_str(s).ok());
    match change_management_app::list_pull_requests(&deps, id, status).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn change_management_list_all_pull_requests_handler(
    State(deps): State<Arc<change_management_Deps>>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Value>, StatusCode> {
    let status = q
        .get("status")
        .filter(|s| !s.is_empty())
        .and_then(|s| serde_json::from_str(s).ok());
    match change_management_app::list_all_pull_requests(&deps, status).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn change_management_get_pull_request_handler(
    State(deps): State<Arc<change_management_Deps>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<Value>, StatusCode> {
    let id = id.parse::<Uuid>().map_err(|_| StatusCode::BAD_REQUEST)?;
    match change_management_app::get_pull_request(&deps, id).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn change_management_update_pull_request_status_handler(
    State(deps): State<Arc<change_management_Deps>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let id = id.parse::<Uuid>().map_err(|_| StatusCode::BAD_REQUEST)?;
    let status = serde_json::from_value(body.get("status").cloned().unwrap_or(Value::Null))
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    match change_management_app::update_pull_request_status(&deps, id, status).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn agent_resolve_provider_handler(
    State(deps): State<Arc<agent_Deps>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    match agent_app::resolve_provider().await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn agent_get_tool_registry_handler(
    State(deps): State<Arc<agent_Deps>>,
) -> Result<Json<Value>, StatusCode> {
    match agent_app::get_tool_registry().await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn agent_execute_tool_handler(
    State(deps): State<Arc<agent_Deps>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let name = body
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let args = serde_json::from_value(body.get("args").cloned().unwrap_or(Value::Null))
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    let ctx = {
        let __v = body.get("ctx").cloned().unwrap_or(Value::Null);
        let __v = if matches!(&__v, Value::String(s) if s.is_empty()) {
            Value::Null
        } else {
            __v
        };
        serde_json::from_value(__v).map_err(|_| StatusCode::BAD_REQUEST)?
    };
    match agent_app::execute_tool(name, args, ctx).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn agent_execute_acp_turn_handler(
    State(deps): State<Arc<agent_Deps>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let user_id = body
        .get("user_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let turn_id = body
        .get("turn_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let messages = serde_json::from_value(body.get("messages").cloned().unwrap_or(Value::Null))
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    let tools = serde_json::from_value(body.get("tools").cloned().unwrap_or(Value::Null))
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    match agent_app::execute_acp_turn(&deps, user_id, turn_id, messages, tools).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn agent_send_tool_result_handler(
    State(deps): State<Arc<agent_Deps>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let user_id = body
        .get("user_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let turn_id = body
        .get("turn_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let call_id = body
        .get("call_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let output = serde_json::from_value(body.get("output").cloned().unwrap_or(Value::Null))
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    match agent_app::send_tool_result(&deps, user_id, turn_id, call_id, output).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn agent_ws_agent_chat_handler(
    State(deps): State<Arc<agent_Deps>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let msg = body
        .get("msg")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    match agent_app::ws_agent_chat(&deps, msg).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn agent_ws_agent_acp_handler(
    State(deps): State<Arc<agent_Deps>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let msg = body
        .get("msg")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    match agent_app::ws_agent_acp(msg).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

async fn agent_agent_status_handler(
    State(deps): State<Arc<agent_Deps>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    match agent_app::agent_status(&deps).await {
        Ok(result) => Ok(Json(veil_json_public(&result))),
        Err(e) => Err(veil_domain_error_status(e)),
    }
}

/// Serialize for HTTP JSON, omitting fields annotated role:secret.
/// Persistence (repos) uses full `Serialize` — secrets still round-trip to storage.
fn veil_json_public<T: serde::Serialize>(value: &T) -> serde_json::Value {
    let mut v = serde_json::to_value(value).unwrap_or_default();
    veil_redact_secrets(&mut v);
    v
}

fn veil_redact_secrets(v: &mut serde_json::Value) {
    // Scalar secret fields from role:secret annotations (INV-001).
    const SECRET_KEYS: &[&str] = &[];
    // Header maps/lists often carry API keys — redact values, keep names.
    const HEADER_CONTAINERS: &[&str] = &["headers"];
    match v {
        serde_json::Value::Object(map) => {
            for k in SECRET_KEYS {
                map.remove(*k);
            }
            for hk in HEADER_CONTAINERS {
                if let Some(headers) = map.get_mut(*hk) {
                    veil_redact_header_values(headers);
                }
            }
            for (_k, child) in map.iter_mut() {
                veil_redact_secrets(child);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items.iter_mut() {
                veil_redact_secrets(item);
            }
        }
        _ => {}
    }
}

fn veil_redact_header_values(v: &mut serde_json::Value) {
    match v {
        serde_json::Value::Array(items) => {
            for item in items.iter_mut() {
                if let serde_json::Value::Object(h) = item {
                    if h.contains_key("value") {
                        h.insert("value".into(), serde_json::Value::String(String::new()));
                    }
                    if h.contains_key("Value") {
                        h.insert("Value".into(), serde_json::Value::String(String::new()));
                    }
                }
            }
        }
        serde_json::Value::Object(map) => {
            // Map-shaped headers: redact all values
            for (_k, val) in map.iter_mut() {
                *val = serde_json::Value::String(String::new());
            }
        }
        _ => {}
    }
}

fn veil_domain_error_status(e: DomainError) -> StatusCode {
    match &e {
        DomainError::NotFound => {
            eprintln!("warn: not found: {e}");
            StatusCode::NOT_FOUND
        }
        DomainError::Validation(msg) => {
            eprintln!("warn: validation: {msg}");
            StatusCode::BAD_REQUEST
        }
        DomainError::External(msg) => {
            eprintln!("error: upstream: {msg}");
            StatusCode::BAD_GATEWAY
        }
    }
}

/// Production-oriented auth:
/// - `/health` + OPTIONS always open
/// - `VEIL_DEV=1` → open (local dual-loop only)
/// - else require a key: `VEIL_API_KEY` (admin) and/or `VEIL_TENANT_KEYS`
///   (`tenant-uuid:secret,tenant-uuid2:secret2`)
/// - Present key via `X-Api-Key` or `Authorization: Bearer <key>`
/// - Tenant-scoped routes: prefer `X-Tenant-Id`; if key is a tenant key, that
///   UUID is forced (body/query tenant_id cannot elevate to another tenant)
async fn veil_api_key_middleware(
    headers: HeaderMap,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    if request.uri().path() == "/health" || request.method() == axum::http::Method::OPTIONS {
        return Ok(next.run(request).await);
    }
    let dev = std::env::var("VEIL_DEV").ok().as_deref() == Some("1");
    let require = std::env::var("VEIL_REQUIRE_AUTH").ok().as_deref() == Some("1");
    let admin_key = std::env::var("VEIL_API_KEY").ok().filter(|s| !s.is_empty());
    let tenant_keys = veil_parse_tenant_keys();
    let presented = headers
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .or_else(|| {
            headers
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.strip_prefix("Bearer ").map(|t| t.to_string()))
        });

    if dev && !require && admin_key.is_none() && tenant_keys.is_empty() {
        return Ok(next.run(request).await);
    }

    let Some(presented) = presented else {
        eprintln!("error: missing X-Api-Key / Authorization Bearer");
        return Err(StatusCode::UNAUTHORIZED);
    };

    let is_admin = admin_key.as_deref() == Some(presented.as_str());
    let tenant_from_key = tenant_keys
        .iter()
        .find(|(_, k)| k == &presented)
        .map(|(t, _)| t.clone());

    if !is_admin && tenant_from_key.is_none() {
        eprintln!("warn: unauthorized — key not recognized");
        return Err(StatusCode::UNAUTHORIZED);
    }

    let path = request.uri().path();
    // Provider catalog is admin-only (tenant keys cannot mutate global catalog).
    if path.starts_with("/api/providers") && !is_admin {
        eprintln!("warn: forbidden — provider catalog requires VEIL_API_KEY (admin)");
        return Err(StatusCode::FORBIDDEN);
    }
    // Tenant-scoped surfaces need either admin+X-Tenant-Id or a tenant key.
    let tenant_route = path.starts_with("/api/integrations") || path.starts_with("/api/execute");
    if tenant_route {
        if let Some(ref t) = tenant_from_key {
            if let Some(hdr) = headers.get("x-tenant-id").and_then(|v| v.to_str().ok()) {
                if hdr != t.as_str() {
                    eprintln!("warn: X-Tenant-Id does not match tenant API key");
                    return Err(StatusCode::FORBIDDEN);
                }
            }
        } else if is_admin {
            // Admin acting for a tenant must pass X-Tenant-Id (enforced in handler
            // when not VEIL_DEV).
        }
    }

    Ok(next.run(request).await)
}

/// `VEIL_TENANT_KEYS=uuid:key,uuid2:key2` → list of (tenant_id, api_key).
fn veil_parse_tenant_keys() -> Vec<(String, String)> {
    let Ok(raw) = std::env::var("VEIL_TENANT_KEYS") else {
        return Vec::new();
    };
    raw.split(',')
        .filter_map(|pair| {
            let pair = pair.trim();
            let (t, k) = pair.split_once(':')?;
            let t = t.trim();
            let k = k.trim();
            if t.is_empty() || k.is_empty() {
                None
            } else {
                Some((t.to_string(), k.to_string()))
            }
        })
        .collect()
}

/// Resolve tenant_id for handlers: tenant API key wins; else X-Tenant-Id;
/// body/query only allowed in VEIL_DEV=1 when no tenant key.
fn veil_resolve_tenant_id(headers: &HeaderMap, fallback: Option<Uuid>) -> Result<Uuid, StatusCode> {
    let presented = headers
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .or_else(|| {
            headers
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.strip_prefix("Bearer ").map(|t| t.to_string()))
        });
    let tenant_keys = veil_parse_tenant_keys();
    if let Some(ref key) = presented {
        if let Some((tid, _)) = tenant_keys.iter().find(|(_, k)| k == key) {
            return tid
                .parse::<Uuid>()
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR);
        }
    }
    if let Some(Ok(s)) = headers.get("x-tenant-id").map(|v| v.to_str()) {
        let from_hdr = s.parse::<Uuid>().map_err(|_| StatusCode::BAD_REQUEST)?;
        if let Some(fb) = fallback {
            if fb != from_hdr {
                eprintln!("warn: body/query tenant_id != X-Tenant-Id");
                return Err(StatusCode::FORBIDDEN);
            }
        }
        return Ok(from_hdr);
    }
    let dev = std::env::var("VEIL_DEV").ok().as_deref() == Some("1");
    if dev {
        return fallback.ok_or(StatusCode::BAD_REQUEST);
    }
    eprintln!("error: production requires X-Tenant-Id (or tenant API key)");
    Err(StatusCode::BAD_REQUEST)
}

/// Restrict CORS: `CORS_ORIGINS=http://a,http://b` or localhost defaults (not *).
fn veil_cors_layer() -> CorsLayer {
    use axum::http::{HeaderValue, Method};
    if let Ok(raw) = std::env::var("CORS_ORIGINS") {
        let origins: Vec<HeaderValue> = raw
            .split(',')
            .filter_map(|s| s.trim().parse().ok())
            .collect();
        if !origins.is_empty() {
            return CorsLayer::new()
                .allow_origin(origins)
                .allow_methods([
                    Method::GET,
                    Method::POST,
                    Method::PUT,
                    Method::PATCH,
                    Method::DELETE,
                    Method::OPTIONS,
                ])
                .allow_headers(Any);
        }
    }
    let local = [
        "http://localhost:5173",
        "http://127.0.0.1:5173",
        "http://localhost:5174",
        "http://127.0.0.1:5174",
        "http://localhost:3000",
        "http://127.0.0.1:3000",
    ];
    let origins: Vec<HeaderValue> = local.iter().filter_map(|s| s.parse().ok()).collect();
    CorsLayer::new()
        .allow_origin(origins)
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers(Any)
}

/// HTML `<input type="date">` and form empties → JSON values chrono/serde accept.
/// `""` → null; bare `YYYY-MM-DD` → `YYYY-MM-DDT00:00:00Z`.
fn veil_normalize_body_dt(v: Value) -> Value {
    match v {
        Value::String(s) if s.is_empty() => Value::Null,
        Value::String(s)
            if s.len() == 10
                && s.as_bytes().get(4) == Some(&b'-')
                && s.as_bytes().get(7) == Some(&b'-')
                && !s.contains('T') =>
        {
            Value::String(format!("{s}T00:00:00Z"))
        }
        other => other,
    }
}
