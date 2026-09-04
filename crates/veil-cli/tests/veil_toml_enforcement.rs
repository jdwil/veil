//! Spec 1 — `veil.toml` enforcement on the `veil check` CLI path.
//!
//! decision-registry-repo-structure, Decision 3: a `.veil` with no ancestor
//! `veil.toml` must FAIL (never proceed, never panic) with a `missing veil.toml`
//! diagnostic. A proper project (has `veil.toml`) still succeeds.

use std::process::Command;

fn veil_bin() -> &'static str {
    env!("CARGO_BIN_EXE_veil")
}

fn tmp_dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!(
        "veil-enforce-{tag}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn check_fails_without_veil_toml() {
    let dir = tmp_dir("no-toml");
    let leaf = dir.join("app.veil");
    std::fs::write(&leaf, "pkg App\n").unwrap();

    let out = Command::new(veil_bin())
        .arg("check")
        .arg(&leaf)
        .output()
        .expect("run veil check");

    assert!(
        !out.status.success(),
        "check MUST fail without veil.toml (exit non-zero)"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("missing veil.toml"),
        "stderr must contain 'missing veil.toml': {stderr}"
    );
    assert!(
        stderr.contains(&dir.to_string_lossy().to_string()),
        "stderr must contain the offending abs path: {stderr}"
    );
}

#[test]
fn check_succeeds_with_veil_toml() {
    let dir = tmp_dir("has-toml");
    std::fs::write(
        dir.join("veil.toml"),
        "[package]\nname = \"app\"\nmain = \"app.veil\"\n",
    )
    .unwrap();
    let leaf = dir.join("app.veil");
    // Minimal valid package (no layers required for a bare pkg).
    std::fs::write(&leaf, "pkg App\n").unwrap();

    let out = Command::new(veil_bin())
        .arg("check")
        .arg(&leaf)
        .output()
        .expect("run veil check");

    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "check MUST succeed for a proper project.\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        !stderr.contains("missing veil.toml"),
        "must not report missing veil.toml for a valid project: {stderr}"
    );
}

#[test]
fn check_bundled_example_still_works() {
    // examples/ now has a [workspace] veil.toml, so bundled examples resolve a root.
    // pure_lib.veil checks cleanly (no content errors), so full success is expected.
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let example = root.join("examples/pure_lib.veil");
    let out = Command::new(veil_bin())
        .arg("check")
        .arg(&example)
        .output()
        .expect("run veil check on bundled example");
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stderr.contains("missing veil.toml") && !stdout.contains("missing veil.toml"),
        "bundled example must resolve the examples/ [workspace] root: {stderr}{stdout}"
    );
    assert!(
        out.status.success(),
        "veil check examples/pure_lib.veil must succeed:\nstdout: {stdout}\nstderr: {stderr}"
    );
}
