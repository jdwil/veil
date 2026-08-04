use std::path::PathBuf;
use std::sync::Arc;

use axum::routing::post;
use axum::Router;
use tower_http::cors::CorsLayer;

use acr_eval::builtin_tasks;
use acr_eval::harness::EvalHarness;
use acr_library::store::FsStore;

use crate::handlers::{self, AppState};

pub async fn build_router(library_path: PathBuf) -> Router {
    let store = Arc::new(FsStore::new(library_path).await.expect("Failed to create store"));
    let tasks = builtin_tasks::all_tasks();
    let harness = EvalHarness::new();

    let state = Arc::new(AppState {
        store,
        tasks,
        harness,
    });

    Router::new()
        .route("/tools/list_algorithms", post(handlers::list_algorithms))
        .route("/tools/create_candidate", post(handlers::create_candidate))
        .route("/tools/update_candidate", post(handlers::update_candidate))
        .route("/tools/run_evaluation", post(handlers::run_evaluation))
        .route("/tools/get_trace", post(handlers::get_trace))
        .route("/tools/promote", post(handlers::promote))
        .route("/tools/list_tasks", post(handlers::list_tasks))
        .route(
            "/tools/get_library_status",
            post(handlers::get_library_status),
        )
        .layer(CorsLayer::permissive())
        .with_state(state)
}
