//! Autodesk MIT sample DWGs: `count --by_param` reads without error.

use std::collections::BTreeMap;
use std::path::PathBuf;

use cadcraft_engine::Session;
use serde_json::json;

fn sample(name: &str) -> PathBuf {
    let home = std::env::var("HOME").expect("HOME");
    PathBuf::from(home).join(format!(
        ".cargo/registry/src/index.crates.io-1949cf8c6b5b557f/acadrust-0.6.6/tests/fixtures/dynamic-blocks/{name}"
    ))
}

#[test]
fn autodesk_lookup_and_point_samples_count_by_param() {
    cadcraft_engine::cmd::file::set_io(cadcraft_engine::cmd::file::IoHooks {
        read: |b, path| cadcraft_io::read(b, path).map_err(|e| e.to_string()),
        write: |d, path| cadcraft_io::write(d, path).map_err(|e| e.to_string()),
        plot: None,
    });
    for name in ["lookup.dwg", "point.dwg"] {
        let path = sample(name);
        assert!(path.exists(), "missing {}", path.display());
        let mut s = Session::new();
        s.execute("open", &json!({ "path": path.to_string_lossy() })).unwrap();
        let r = s.execute("count", &json!({ "by_param": true, "nested": false })).unwrap();
        let _: BTreeMap<String, BTreeMap<String, u64>> = serde_json::from_value(r["by_param"].clone()).unwrap();
    }
}