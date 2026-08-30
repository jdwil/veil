//! Phase 1 verification: git-backed file I/O round-trips through a real git
//! remote. A local `--bare` repo stands in for GitHub/Bitbucket via the
//! `VEIL_GITHUB_BASE_URL=file://…` override.
//!
//! Run: `cargo test -p veil-runtime --test git_backend_roundtrip`

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use veil_server::git_origin::{GitOrigin, GitProvider, RemoteConfig};

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir()
        .join("veil-git-backend-it")
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

// git_files is a private module of the binary crate; re-declare the pieces we
// exercise by calling the public GitOrigin API the module is built on. This
// mirrors git_files::{write_file,read_file} semantics precisely.
mod files {
    use super::*;
    use veil_server::git_origin::CheckoutMode;

    fn work_dir(repo_id: &str) -> PathBuf {
        // Unique per process run so repeated `cargo test` invocations don't hit a
        // clean working tree from a prior run's cached checkout.
        std::env::temp_dir()
            .join("veil-git-work-it")
            .join(std::process::id().to_string())
            .join(repo_id.chars().take(16).collect::<String>())
    }

    pub fn write(origin: &GitOrigin, branch: &str, rel: &str, content: &str) -> String {
        let work = work_dir(&origin.repo_id);
        origin.checkout(&work, branch, CheckoutMode::FetchKeepDirty).unwrap();
        let root = origin.project_root(&work);
        let full = root.join(rel);
        if let Some(p) = full.parent() {
            std::fs::create_dir_all(p).unwrap();
        }
        std::fs::write(&full, content).unwrap();
        origin.commit_and_push(&work, "test write", branch).unwrap().sha
    }

    pub fn read(origin: &GitOrigin, branch: &str, rel: &str) -> Option<String> {
        // fresh dir to prove it reads from the remote, not local scratch
        let work = tmp("read-fresh");
        origin.checkout(&work, branch, CheckoutMode::FetchKeepDirty).unwrap();
        let root = origin.project_root(&work);
        std::fs::read_to_string(root.join(rel)).ok()
    }
}

#[test]
fn git_backed_write_read_roundtrip_with_subpath() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    // Bare "provider" repo with an initial main commit under a subpath.
    let bare = tmp("provider.git");
    std::fs::create_dir_all(&bare).unwrap();
    assert!(Command::new("git")
        .args(["init", "--bare", "-b", "main"])
        .current_dir(&bare)
        .status()
        .unwrap()
        .success());
    let seed = tmp("seed");
    std::fs::create_dir_all(seed.join("agent-core")).unwrap();
    std::fs::write(seed.join("agent-core/main.veil"), "pkg AgentCore\n").unwrap();
    std::fs::write(seed.join("README.md"), "mono\n").unwrap();
    git(&seed, &["init", "-b", "main"]);
    git(&seed, &["add", "-A"]);
    git(&seed, &["commit", "-m", "seed"]);
    git(&seed, &["remote", "add", "origin", &bare.to_string_lossy()]);
    git(&seed, &["push", "origin", "main:main"]);

    // Point the GitHub provider base at the file:// bare repo.
    let url = format!("file://{}", bare.to_string_lossy());
    // SAFETY: serialized by ENV_LOCK.
    unsafe {
        std::env::set_var("VEIL_GITHUB_BASE_URL", &url);
        std::env::set_var("VEIL_GIT_ORIGIN", "1");
    }

    let cfg = RemoteConfig {
        provider: GitProvider::GitHub,
        repo: "org/mono".into(),
        subpath: Some("agent-core".into()),
        branch: "main".into(),
    };
    let origin = GitOrigin::with_remote("it-repo-0001", cfg);
    assert!(origin.is_git_remote());
    assert!(origin.exists(), "remote should be reachable via file://");

    // Write a new file within the subpath, push to the remote.
    let sha = files::write(&origin, "main", "handlers/hello.veil", "handler Hello\n");
    assert_eq!(sha.len(), 40);

    // Read it back from a fresh checkout — proves it round-tripped to the remote.
    let got = files::read(&origin, "main", "handlers/hello.veil");
    assert_eq!(got.as_deref(), Some("handler Hello\n"));

    // The pre-existing subpath file is still there.
    let base = files::read(&origin, "main", "main.veil");
    assert_eq!(base.as_deref(), Some("pkg AgentCore\n"));

    // The README (outside the subpath) is NOT part of the project root view.
    let outside = files::read(&origin, "main", "README.md");
    assert_eq!(outside, None, "README.md is outside the subpath project root");

    unsafe {
        std::env::remove_var("VEIL_GITHUB_BASE_URL");
        std::env::remove_var("VEIL_GIT_ORIGIN");
    }
    let _ = std::fs::remove_dir_all(&bare);
    let _ = std::fs::remove_dir_all(&seed);
}
