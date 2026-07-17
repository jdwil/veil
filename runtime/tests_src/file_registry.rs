use std::sync::Arc;
use extensions::adapters::*;
use extensions::application::{self, Deps};
use tempfile::tempdir;

#[tokio::test]
async fn file_registry_roundtrip() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path().to_string_lossy().to_string();
    let deps = Deps {
        extension_artifact_store: Arc::new(FileExtensionArtifactStore { dir: dir.clone() }),
        extension_executor: Arc::new(FileExtensionExecutor { dir: dir.clone() }),
        extension_registry: Arc::new(FileExtensionRegistry { dir: dir.clone() }),
        extension_source_store: Arc::new(FileExtensionSourceStore { dir }),
    };
    let rec = application::create_extension(
        &deps, "t".into(), "Reaction".into(), "Platform".into(), "Stock".into(),
        None, None, None, None, None,
    ).await.expect("create");
    let got = application::get_extension(&deps, rec.extension_id).await.expect("get");
    assert_eq!(got.name, "t");
    let listed = application::list_extensions(&deps, None, None, None, None).await.unwrap();
    assert_eq!(listed.len(), 1);
    let ver = application::publish_extension(&deps, rec.extension_id).await.expect("pub");
    assert_eq!(ver.version, 1);
}
