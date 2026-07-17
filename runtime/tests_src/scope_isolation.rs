use std::sync::Arc;
use extensions::adapters::*;
use extensions::application::{self, Deps};
use tempfile::tempdir;
use uuid::Uuid;

fn deps(dir: &str) -> Deps {
    Deps {
        extension_artifact_store: Arc::new(FileExtensionArtifactStore { dir: dir.to_string() }),
        extension_executor: Arc::new(FileExtensionExecutor { dir: dir.to_string() }),
        extension_registry: Arc::new(FileExtensionRegistry { dir: dir.to_string() }),
        extension_source_store: Arc::new(FileExtensionSourceStore { dir: dir.to_string() }),
    }
}

#[tokio::test]
async fn tenant_scope_requires_tenant_id() {
    let tmp = tempdir().unwrap();
    let d = deps(tmp.path().to_str().unwrap());
    let empty = application::list_extensions_by_scope(&d, "Tenant".into(), None, None, None)
        .await
        .unwrap();
    assert!(empty.is_empty());
}

#[tokio::test]
async fn fork_marks_lineage() {
    let tmp = tempdir().unwrap();
    let d = deps(tmp.path().to_str().unwrap());
    let stock = application::create_extension(
        &d, "S".into(), "Reaction".into(), "Platform".into(), "Stock".into(),
        Some("wear_test".into()), None, None, None, None,
    ).await.unwrap();
    let fork = application::fork_extension(
        &d, stock.extension_id, 1, "S custom".into(), Some(Uuid::nil()), None,
    ).await.unwrap();
    assert!(fork.created_from.is_some());
    assert_eq!(fork.name, "S custom");
}
