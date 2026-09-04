//! Spec 3 (decision-registry-repo-structure §"Detect-and-Offer-Fix"):
//! add-project-to-subpath detects whether the shared repo ROOT is a
//! multi-project VEIL workspace and offers to initialize it. Exercised at the
//! `GitOrigin` API level against a local `--bare` repo standing in for
//! GitHub via `VEIL_GITHUB_BASE_URL=file://…`.
//!
//! Run: `cargo test -p veil-server --test workspace_detect_fix`

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use veil_server::git_origin::{GitOrigin, GitProvider, RemoteConfig};

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir()
        .join("veil-ws-detect-it")
        .join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir_all(&d).unwrap();
    d.join(name)
}

fn git(cwd: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .current_dir(cwd)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .args(["-c", "user.name=T", "-c", "user.email=t@t"])
        .args(args)
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {:?} failed", args);
}

/// Create a bare "provider" repo. If `workspace` is Some, seed a root
/// `veil.toml [workspace]` with the given members; otherwise seed a plain repo
/// with just a README (NOT a workspace).
fn bare_repo(workspace: Option<&[&str]>) -> PathBuf {
    let bare = tmp("provider.git");
    std::fs::create_dir_all(&bare).unwrap();
    assert!(Command::new("git")
        .args(["init", "--bare", "-b", "main"])
        .current_dir(&bare)
        .status()
        .unwrap()
        .success());
    let seed = tmp("seed");
    std::fs::create_dir_all(&seed).unwrap();
    if let Some(members) = workspace {
        let refs: Vec<&str> = members.to_vec();
        let toml = veil_server::project_layout::workspace_root_veil_toml(&refs);
        std::fs::write(seed.join("veil.toml"), toml).unwrap();
    }
    std::fs::write(seed.join("README.md"), "shared repo\n").unwrap();
    git(&seed, &["init", "-b", "main"]);
    git(&seed, &["add", "-A"]);
    git(&seed, &["commit", "-m", "seed"]);
    git(&seed, &["remote", "add", "origin", &bare.to_string_lossy()]);
    git(&seed, &["push", "origin", "main:main"]);
    let _ = std::fs::remove_dir_all(&seed);
    bare
}

fn scaffold() -> Vec<(String, String)> {
    veil_server::project_layout::scaffold_file_contents("inventory").unwrap()
}

fn origin_for(bare: &Path, repo_id: &str, subpath: &str) -> GitOrigin {
    let cfg = RemoteConfig {
        provider: GitProvider::GitHub,
        repo: "org/shared".into(),
        subpath: Some(subpath.into()),
        branch: "main".into(),
    };
    let _ = bare;
    GitOrigin::with_remote(repo_id, cfg)
}

/// Read a file from a fresh checkout of the remote root (proves it round-tripped).
fn read_root(origin: &GitOrigin, rel: &str) -> Option<String> {
    use veil_server::git_origin::CheckoutMode;
    let work = tmp("read-root");
    origin.checkout(&work, "main", CheckoutMode::ResetHard).ok()?;
    let out = std::fs::read_to_string(work.join(rel)).ok();
    let _ = std::fs::remove_dir_all(&work);
    out
}

fn set_base(bare: &Path) {
    let url = format!("file://{}", bare.to_string_lossy());
    unsafe {
        std::env::set_var("VEIL_GITHUB_BASE_URL", &url);
        std::env::set_var("VEIL_GIT_ORIGIN", "1");
    }
}

fn clear_base() {
    unsafe {
        std::env::remove_var("VEIL_GITHUB_BASE_URL");
        std::env::remove_var("VEIL_GIT_ORIGIN");
    }
}

#[test]
fn fresh_repo_reports_needs_workspace_init_and_does_not_seed() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let bare = bare_repo(None); // NOT a workspace
    set_base(&bare);

    let origin = origin_for(&bare, "it-fresh-0001", "inventory");
    // Detection: the root is not a workspace.
    assert!(
        !origin.subpath_root_is_workspace("main").unwrap(),
        "fresh repo must not be detected as a workspace"
    );
    // The offer path must NOT seed: no subpath project appears on the remote.
    assert_eq!(
        read_root(&origin, "inventory/veil.toml"),
        None,
        "detect-and-offer must not seed the subpath before confirm"
    );
    // And there is still no root workspace manifest.
    assert_eq!(read_root(&origin, "veil.toml"), None);

    clear_base();
    let _ = std::fs::remove_dir_all(&bare);
}

#[test]
fn fix_initializes_workspace_seeds_and_is_idempotent() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let bare = bare_repo(None); // NOT a workspace
    set_base(&bare);

    let origin = origin_for(&bare, "it-fix-0001", "inventory");
    assert!(!origin.subpath_root_is_workspace("main").unwrap());

    // Fix: init workspace root + seed subproject + add member (one commit).
    let sha = origin
        .init_workspace_and_seed_subpath(&scaffold(), "main", true)
        .unwrap();
    assert!(sha.is_some(), "fix must produce a commit");

    // Root is now a workspace listing the member.
    let root_toml = read_root(&origin, "veil.toml").expect("root veil.toml exists");
    assert!(root_toml.contains("[workspace]"), "{root_toml}");
    assert!(root_toml.contains("\"inventory\""), "member listed: {root_toml}");
    assert!(
        origin.subpath_root_is_workspace("main").unwrap(),
        "root is now a workspace"
    );
    // Subproject was seeded under the subpath.
    assert!(
        read_root(&origin, "inventory/veil.toml").is_some(),
        "subproject seeded"
    );
    assert!(read_root(&origin, "inventory/main.veil").is_some());

    // Re-run is a no-op (idempotent): no new commit.
    let sha2 = origin
        .init_workspace_and_seed_subpath(&scaffold(), "main", true)
        .unwrap();
    assert!(sha2.is_none(), "idempotent re-run must not commit again");

    clear_base();
    let _ = std::fs::remove_dir_all(&bare);
}

#[test]
fn already_workspace_seeds_and_appends_member_in_one_step() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // Repo is ALREADY a workspace (with an existing member "auth").
    let bare = bare_repo(Some(&["auth"]));
    set_base(&bare);

    let origin = origin_for(&bare, "it-ws-0001", "inventory");
    assert!(
        origin.subpath_root_is_workspace("main").unwrap(),
        "existing workspace detected"
    );

    // Fast path: init_workspace_root=false — just seed + append member.
    let sha = origin
        .init_workspace_and_seed_subpath(&scaffold(), "main", false)
        .unwrap();
    assert!(sha.is_some(), "seeding a new member must commit");

    // Member appended (sorted with the pre-existing one), subproject seeded.
    let root_toml = read_root(&origin, "veil.toml").expect("root veil.toml");
    assert!(root_toml.contains("\"auth\""), "pre-existing member kept: {root_toml}");
    assert!(root_toml.contains("\"inventory\""), "new member added: {root_toml}");
    assert!(read_root(&origin, "inventory/main.veil").is_some(), "seeded");

    // Idempotent re-run.
    let sha2 = origin
        .init_workspace_and_seed_subpath(&scaffold(), "main", false)
        .unwrap();
    assert!(sha2.is_none(), "idempotent");

    clear_base();
    let _ = std::fs::remove_dir_all(&bare);
}

/// The create-project seed SOURCE (`seed_new_repo_scaffold_ws`) — the function
/// whose result `create_project_domain` threads into `needs_workspace_init` —
/// reports the offer WITHOUT git-seeding when the shared repo is not a workspace.
#[test]
fn seed_new_repo_scaffold_ws_offers_when_not_a_workspace() {
    use veil_server::git_origin::{register_origin, RemoteConfig};
    use veil_server::provider::s3_workspace::{seed_new_repo_scaffold_ws, SubpathSeedOutcome};

    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let bare = bare_repo(None); // NOT a workspace
    set_base(&bare);
    // Local source mode → skip S3 writes (no AWS in this test).
    unsafe {
        std::env::set_var("VEIL_SOURCE_MODE", "local");
    }

    // Bind a subpath git origin for the repo id via the process-local cache.
    let repo_id = "it-createflow-0001";
    register_origin(
        repo_id,
        Some(RemoteConfig {
            provider: GitProvider::GitHub,
            repo: "org/shared".into(),
            subpath: Some("inventory".into()),
            branch: "main".into(),
        }),
    );

    let outcome = seed_new_repo_scaffold_ws(repo_id, "inventory").unwrap();
    match outcome {
        SubpathSeedOutcome::NeedsWorkspaceInit { .. } => { /* expected */ }
        other => panic!("expected NeedsWorkspaceInit, got {other:?}"),
    }
    // Nothing was git-seeded on the remote.
    let origin = GitOrigin::with_remote(
        repo_id,
        RemoteConfig {
            provider: GitProvider::GitHub,
            repo: "org/shared".into(),
            subpath: Some("inventory".into()),
            branch: "main".into(),
        },
    );
    assert_eq!(read_root(&origin, "inventory/veil.toml"), None, "not seeded");
    assert_eq!(read_root(&origin, "veil.toml"), None, "no workspace root");

    register_origin(repo_id, None);
    unsafe {
        std::env::remove_var("VEIL_SOURCE_MODE");
    }
    clear_base();
    let _ = std::fs::remove_dir_all(&bare);
}

