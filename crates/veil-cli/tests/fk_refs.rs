use std::path::PathBuf;
use veil_ir::{build_ir_with_registry, EdgeKind, LayerRegistry};

#[test]
fn dlx_core_fk_references() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let path = root.join("examples/dlx_core.veil");
    let reg = LayerRegistry::for_veil_file(&path).expect("registry");
    assert_eq!(
        reg.identity_policy.ref_suffix.as_deref(),
        Some("_id"),
        "layers={:?}",
        reg.layers
    );
    let content = std::fs::read_to_string(&path).unwrap();
    let tokens = veil_parser::lex(&content);
    let sol = veil_parser::parse_with_registry(&tokens, reg.clone()).expect("parse");
    let g = build_ir_with_registry(&sol, Some(&reg));
    let n = g.edges.iter().filter(|e| e.kind == EdgeKind::References).count();
    eprintln!("refs={n} layers={:?}", reg.layers);
    assert!(n > 0, "expected References edges");
}
