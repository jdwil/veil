//! Phase 2 verification: the provider REST client shapes correct requests for
//! opening a PR, posting the `veil/review` status (the merge gate), and merging.
//!
//! A local axum server stands in for the GitHub API (via VEIL_GITHUB_API_BASE),
//! capturing method + path + body so we can assert the contract without real
//! credentials.
//!
//! Run: `cargo test -p veil-server --test git_provider_contract`

use std::sync::{Arc, Mutex};

use axum::{
    Json, Router,
    extract::State,
    routing::{get, post, put},
};
use serde_json::{Value, json};

use veil_server::git_origin::GitProvider;
use veil_server::git_provider::{ProviderRepo, VEIL_REVIEW_CONTEXT};

#[derive(Clone, Default)]
struct Captured {
    log: Arc<Mutex<Vec<(String, String, Value)>>>,
}

async fn create_pr(State(cap): State<Captured>, Json(body): Json<Value>) -> Json<Value> {
    cap.log
        .lock()
        .unwrap()
        .push(("POST".into(), "/repos/org/name/pulls".into(), body));
    Json(json!({ "number": 42, "head": { "sha": "abc123" }, "html_url": "http://x/pr/42" }))
}

async fn post_status(
    State(cap): State<Captured>,
    axum::extract::Path(sha): axum::extract::Path<String>,
    Json(body): Json<Value>,
) -> Json<Value> {
    cap.log
        .lock()
        .unwrap()
        .push(("POST".into(), format!("/statuses/{sha}"), body));
    Json(json!({ "state": "success" }))
}

async fn merge_pr(State(cap): State<Captured>, Json(body): Json<Value>) -> Json<Value> {
    cap.log
        .lock()
        .unwrap()
        .push(("PUT".into(), "/merge".into(), body));
    Json(json!({ "sha": "merged999", "merged": true }))
}

#[tokio::test]
async fn github_pr_status_merge_contract() {
    let cap = Captured::default();
    let app = Router::new()
        .route("/repos/org/name/pulls", post(create_pr))
        .route("/repos/org/name/statuses/{sha}", post(post_status))
        .route("/repos/org/name/pulls/{n}/merge", put(merge_pr))
        .with_state(cap.clone());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let base = format!("http://{addr}");
    let log = cap.log.clone();
    let result = tokio::task::spawn_blocking(move || {
        // SAFETY: single test; env is process config.
        unsafe {
            std::env::set_var("VEIL_GITHUB_API_BASE", &base);
            std::env::set_var("VEIL_GITHUB_TOKEN", "test-token");
        }
        let repo = ProviderRepo::new(GitProvider::GitHub, "org/name");
        let pr = repo
            .create_pull_request("feat-x", "main", "Add x", "body")
            .expect("create pr");
        assert_eq!(pr.number, 42);
        assert_eq!(pr.head_sha, "abc123");

        repo.post_veil_review_status(&pr.head_sha, true, "approved", None)
            .expect("post status");

        let merged = repo
            .merge_pull_request(pr.number, Some("merge it"))
            .expect("merge");
        assert_eq!(merged, "merged999");
        unsafe {
            std::env::remove_var("VEIL_GITHUB_API_BASE");
            std::env::remove_var("VEIL_GITHUB_TOKEN");
        }
    })
    .await;
    result.unwrap();

    let entries = log.lock().unwrap().clone();
    assert_eq!(entries.len(), 3, "expected create + status + merge");

    // 1) create PR body: head/base/title.
    let (m, p, body) = &entries[0];
    assert_eq!(m, "POST");
    assert!(p.ends_with("/pulls"));
    assert_eq!(body["head"], "feat-x");
    assert_eq!(body["base"], "main");
    assert_eq!(body["title"], "Add x");

    // 2) status on the head sha with the veil/review context = success.
    let (m, p, body) = &entries[1];
    assert_eq!(m, "POST");
    assert!(p.contains("/statuses/abc123"));
    assert_eq!(body["context"], VEIL_REVIEW_CONTEXT);
    assert_eq!(body["state"], "success");

    // 3) merge.
    let (m, _p, body) = &entries[2];
    assert_eq!(m, "PUT");
    assert_eq!(body["merge_method"], "merge");
}

#[derive(Clone, Default)]
struct BbCaptured {
    log: Arc<Mutex<Vec<(String, String)>>>,
}

async fn bb_create_pr(
    State(cap): State<BbCaptured>,
    axum::extract::Path((proj, slug)): axum::extract::Path<(String, String)>,
    Json(body): Json<Value>,
) -> Json<Value> {
    cap.log.lock().unwrap().push((
        "create".into(),
        format!(
            "/projects/{proj}/repos/{slug}/pull-requests fromRef={}",
            body["fromRef"]["id"]
        ),
    ));
    Json(
        json!({ "id": 7, "fromRef": { "latestCommit": "deadbeef" }, "links": { "self": [ { "href": "http://bb/pr/7" } ] } }),
    )
}

async fn bb_build_status(
    State(cap): State<BbCaptured>,
    axum::extract::Path(sha): axum::extract::Path<String>,
    Json(body): Json<Value>,
) -> Json<Value> {
    cap.log.lock().unwrap().push((
        "status".into(),
        format!(
            "/build-status/commits/{sha} key={} state={}",
            body["key"], body["state"]
        ),
    ));
    Json(json!({}))
}

/// Bitbucket Server/DC uses distinct REST paths (rest/api/1.0/projects/.../repos/...
/// and rest/build-status/1.0/commits/{sha}) and Bearer auth. This asserts the
/// variant routing + endpoint shapes without real credentials.
#[tokio::test]
async fn bitbucket_server_variant_contract() {
    let cap = BbCaptured::default();
    let app = Router::new()
        .route(
            "/rest/api/1.0/projects/{proj}/repos/{slug}/pull-requests",
            post(bb_create_pr),
        )
        .route(
            "/rest/build-status/1.0/commits/{sha}",
            post(bb_build_status),
        )
        .with_state(cap.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let base = format!("http://{addr}");
    let log = cap.log.clone();
    tokio::task::spawn_blocking(move || {
        unsafe {
            std::env::set_var("VEIL_BITBUCKET_VARIANT", "server");
            std::env::set_var("VEIL_BITBUCKET_API_BASE", &base);
            std::env::set_var("VEIL_BITBUCKET_TOKEN", "bb-http-token");
        }
        let repo = ProviderRepo::new(GitProvider::Bitbucket, "DLX/agent-core");
        let pr = repo
            .create_pull_request("feat-y", "main", "Y", "body")
            .expect("bb-server create pr");
        assert_eq!(pr.number, 7);
        assert_eq!(pr.head_sha, "deadbeef");
        repo.post_veil_review_status(&pr.head_sha, true, "ok", None)
            .expect("bb-server status");
        unsafe {
            std::env::remove_var("VEIL_BITBUCKET_VARIANT");
            std::env::remove_var("VEIL_BITBUCKET_API_BASE");
            std::env::remove_var("VEIL_BITBUCKET_TOKEN");
        }
    })
    .await
    .unwrap();

    let entries = log.lock().unwrap().clone();
    assert_eq!(
        entries.len(),
        2,
        "expected create + status on server endpoints"
    );
    assert!(
        entries[0]
            .1
            .contains("/projects/DLX/repos/agent-core/pull-requests")
    );
    assert!(
        entries[0].1.contains("refs/heads/feat-y"),
        "fromRef id shape"
    );
    assert!(entries[1].1.contains("/build-status/commits/deadbeef"));
    assert!(entries[1].1.contains("key=\"veil/review\""));
    assert!(entries[1].1.contains("state=\"SUCCESSFUL\""));
}

#[derive(Clone, Default)]
struct GhCreateCap {
    log: Arc<Mutex<Vec<(String, Value)>>>,
}

async fn gh_user(State(cap): State<GhCreateCap>) -> Json<Value> {
    cap.log
        .lock()
        .unwrap()
        .push(("GET /user".into(), json!({})));
    Json(json!({ "login": "jd", "html_url": "https://github.com/jd" }))
}

async fn gh_user_repos(
    State(cap): State<GhCreateCap>,
    Json(body): Json<Value>,
) -> (axum::http::StatusCode, Json<Value>) {
    cap.log
        .lock()
        .unwrap()
        .push(("POST /user/repos".into(), body.clone()));
    (
        axum::http::StatusCode::CREATED,
        Json(json!({
            "full_name": format!("jd/{}", body["name"].as_str().unwrap_or("x")),
            "html_url": "https://github.com/jd/widgets",
            "private": body["private"],
            "size": 0
        })),
    )
}

#[tokio::test]
async fn github_create_repo_contract() {
    let cap = GhCreateCap::default();
    let app = Router::new()
        .route("/user", get(gh_user))
        .route("/user/repos", post(gh_user_repos))
        .with_state(cap.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let base = format!("http://{addr}");
    let log = cap.log.clone();
    let result = tokio::task::spawn_blocking(move || {
        unsafe {
            std::env::set_var("VEIL_GITHUB_API_BASE", &base);
            std::env::set_var("VEIL_GITHUB_TOKEN", "test-token");
            std::env::set_var("VEIL_GITHUB_OWNER", "jd");
            std::env::set_var("VEIL_GITHUB_GH_CLI", "0");
        }
        let me = veil_server::git_provider::github_whoami().expect("whoami");
        assert_eq!(me.login, "jd");
        let repo = veil_server::git_provider::github_create_repo(
            "jd",
            "widgets",
            true,
            Some("a veil product"),
        )
        .expect("create");
        assert_eq!(repo.full_name, "jd/widgets");
        assert!(repo.empty);
        unsafe {
            std::env::remove_var("VEIL_GITHUB_API_BASE");
            std::env::remove_var("VEIL_GITHUB_TOKEN");
            std::env::remove_var("VEIL_GITHUB_OWNER");
        }
    })
    .await;
    result.unwrap();
    let entries = log.lock().unwrap().clone();
    assert!(entries.iter().any(|(m, _)| m == "GET /user"));
    let body = entries
        .iter()
        .find(|(m, _)| m == "POST /user/repos")
        .map(|(_, b)| b.clone())
        .expect("create body");
    assert_eq!(body["name"], "widgets");
    assert_eq!(body["private"], true);
    assert_eq!(body["auto_init"], false);
}
