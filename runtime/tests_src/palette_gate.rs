use extensions::application;

#[tokio::test]
async fn palette_accepts_ext05_nodes() {
    let ok = application::validate_reaction_palette(vec![
        "Guard".into(),
        "Activate".into(),
        "End".into(),
    ])
    .await
    .unwrap();
    assert!(ok);
}

#[tokio::test]
async fn palette_rejects_unknown_nodes() {
    let ok = application::validate_reaction_palette(vec![
        "Guard".into(),
        "EvalPython".into(),
        "End".into(),
    ])
    .await
    .unwrap();
    assert!(!ok);
}
