//! Phase 1 dynamic block authoring: `bparam.flip`, `bparam.visibility`, save round-trip, `count --by_param`.

use std::collections::BTreeMap;

use cadcraft_doc::dynblock::{ObjectIndex, dynamic_block, is_dynamic_block_in};
use cadcraft_engine::Session;
use cadcraft_engine::doc::EntityKind;
use serde_json::{Value, json};

#[path = "support/temp_dir.rs"]
mod temp_dir;

use temp_dir::TempDir;

fn install_io() {
    cadcraft_engine::cmd::file::set_io(cadcraft_engine::cmd::file::IoHooks {
        read: |b, name| cadcraft_io::read(b, name).map_err(|e| e.to_string()),
        write: |d, name| cadcraft_io::write(d, name).map_err(|e| e.to_string()),
        plot: None,
    });
}

fn line(s: &mut Session, a: [f64; 2], b: [f64; 2]) -> Value {
    s.execute("line", &json!({ "points": [a, b] })).unwrap()["handles"][0].clone()
}

/// Square frame (4 lines) + arrow shaft + head → block ARROW for flip tests.
fn arrow_block(s: &mut Session) -> String {
    let mut hs = Vec::new();
    for (a, b) in [([0.0, 0.0], [30.0, 0.0]), ([30.0, 0.0], [30.0, 30.0]), ([30.0, 30.0], [0.0, 30.0]), ([0.0, 30.0], [0.0, 0.0])] {
        hs.push(line(s, a, b));
    }
    hs.push(line(s, [5.0, 15.0], [25.0, 15.0]));
    hs.push(line(s, [18.0, 10.0], [25.0, 15.0]));
    hs.push(line(s, [18.0, 20.0], [25.0, 15.0]));
    let r = s.execute("block", &json!({ "name": "ARROW", "base": [0, 0], "handles": hs })).unwrap();
    if let Some(ins) = r.get("insert") {
        s.execute("erase", &json!({ "handles": [ins] })).unwrap();
    }
    "ARROW".into()
}

/// Frame + X + circle for four visibility states (probe symbol).
fn detector_block(s: &mut Session) -> String {
    let mut hs = Vec::new();
    for (a, b) in [([0.0, 0.0], [20.0, 0.0]), ([20.0, 0.0], [20.0, 20.0]), ([20.0, 20.0], [0.0, 20.0]), ([0.0, 20.0], [0.0, 0.0])] {
        hs.push(line(s, a, b));
    }
    hs.push(line(s, [4.0, 4.0], [16.0, 16.0]));
    hs.push(line(s, [16.0, 4.0], [4.0, 16.0]));
    hs.push(
        s.execute("circle", &json!({ "center": [10, 10], "radius": 5 }))
            .unwrap()["handle"]
            .clone(),
    );
    let r = s.execute("block", &json!({ "name": "DETECTOR", "base": [0, 0], "handles": hs })).unwrap();
    if let Some(ins) = r.get("insert") {
        s.execute("erase", &json!({ "handles": [ins] })).unwrap();
    }
    "DETECTOR".into()
}

fn dyn_metrics(s: &Session, block: &str) -> (usize, usize, bool) {
    let d = s.doc().unwrap();
    let idx = ObjectIndex::new(d);
    let is_dyn = is_dynamic_block_in(d, &idx, block);
    let Some(m) = dynamic_block(d, block) else {
        return (0, 0, is_dyn);
    };
    (m.parameters.len(), m.actions.len(), is_dyn)
}

fn round_trip(s: &mut Session, path: &std::path::Path) {
    s.execute("saveas", &json!({ "path": path.to_string_lossy() })).unwrap();
    s.execute("open", &json!({ "path": path.to_string_lossy() })).unwrap();
}

#[test]
fn bparam_flip_makes_block_dynamic() {
    let mut s = Session::new();
    let block = arrow_block(&mut s);
    s.execute(
        "bparam.flip",
        &json!({ "block": block, "line": [[15.0, -3.0], [15.0, 33.0]] }),
    )
    .unwrap();
    let (params, actions, is_dyn) = dyn_metrics(&s, &block);
    assert!(is_dyn, "ARROW should be dynamic after bparam.flip");
    assert_eq!(params, 1, "one flip parameter");
    assert_eq!(actions, 1, "one flip action");
}

#[test]
fn bparam_visibility_makes_block_dynamic_with_states() {
    let mut s = Session::new();
    let block = detector_block(&mut s);
    s.execute(
        "bparam.visibility",
        &json!({
            "block": block,
            "grip": [22, 18],
            "default": "SMOKE",
            "states": {
                "SMOKE": [0, 1, 2, 3, 4, 5],
                "HEAT": [0, 1, 2, 3, 6],
                "DIFF": [0, 1, 2, 3, 4],
                "ADDR": [0, 1, 2, 3]
            }
        }),
    )
    .unwrap();
    let (params, actions, is_dyn) = dyn_metrics(&s, &block);
    assert!(is_dyn);
    assert_eq!(params, 1);
    assert_eq!(actions, 0, "visibility has no stretch/flip actions");
    let m = s.execute("blocks.dynamic", &json!({ "name": block })).unwrap();
    let states = m["parameters"][0]["kind"]["states"].as_array().expect("visibility states");
    assert_eq!(states.len(), 4);
}

#[test]
fn dynamic_objects_survive_open_save_edit_resave() {
    install_io();
    let dir = TempDir::new("bparam-roundtrip");
    let path = dir.join("flip_roundtrip.dwg");
    let path2 = dir.join("flip_roundtrip_2.dwg");

    let mut s = Session::new();
    let block = arrow_block(&mut s);
    s.execute(
        "bparam.flip",
        &json!({ "block": block, "line": [[15.0, -3.0], [15.0, 33.0]] }),
    )
    .unwrap();
    let before = dyn_metrics(&s, &block);
    assert!(before.2);

    round_trip(&mut s, &path);
    let after_open = dyn_metrics(&s, &block);
    assert_eq!(after_open, before, "open+save keeps dynamic metadata");

    let ins = s.execute("insert", &json!({ "name": block, "at": [100, 0] })).unwrap()["handle"].clone();
    let ih = cadcraft_engine::cmd::curves::handle_value(&ins).unwrap();
    s.execute("move", &json!({ "handles": [ins.clone()], "from": [100, 0], "to": [120, 5] })).unwrap();
    assert!(matches!(
        s.doc().unwrap().entity(ih).unwrap().kind,
        EntityKind::Insert(_)
    ));

    round_trip(&mut s, &path2);
    let after_edit = dyn_metrics(&s, &block);
    assert_eq!(after_edit, before, "move insert + resave keeps dynamic metadata");
}

#[test]
fn count_by_param_groups_flip_and_visibility_inserts() {
    let mut s = Session::new();
    let flip = arrow_block(&mut s);
    s.execute(
        "bparam.flip",
        &json!({ "block": flip, "line": [[15.0, -3.0], [15.0, 33.0]] }),
    )
    .unwrap();
    let vis = detector_block(&mut s);
    s.execute(
        "bparam.visibility",
        &json!({
            "block": vis,
            "default": "SMOKE",
            "states": { "SMOKE": [0,1,2,3,4,5], "HEAT": [0,1,2,3,6], "DIFF": [0,1,2,3,4], "ADDR": [0,1,2,3] }
        }),
    )
    .unwrap();

    let mut flip_ins = Vec::new();
    for _ in 0..5 {
        flip_ins.push(
            s.execute("insert", &json!({ "name": flip, "at": [0, 0] }))
                .unwrap()["handle"]
                .clone(),
        );
    }
    // Three default (Not flipped), two set to Flipped.
    for h in flip_ins.iter().skip(3) {
        s.execute(
            "blocks.reference.set",
            &json!({ "handle": h, "name": "Flip state1", "value": "Flipped" }),
        )
        .unwrap();
    }

    let plan: &[(&str, usize)] = &[("SMOKE", 5), ("HEAT", 3), ("DIFF", 2), ("ADDR", 1)];
    let mut vis_ins = Vec::new();
    for (state, n) in plan {
        for _ in 0..*n {
            let h = s.execute("insert", &json!({ "name": vis, "at": [50, 0] })).unwrap()["handle"].clone();
            s.execute(
                "blocks.reference.set",
                &json!({ "handle": h, "name": "Visibility1", "value": state }),
            )
            .unwrap();
            vis_ins.push(h);
        }
    }

    let r = s.execute("count", &json!({ "by_param": true, "nested": false })).unwrap();
    let by: BTreeMap<String, BTreeMap<String, u64>> = serde_json::from_value(r["by_param"].clone()).unwrap();

    let flip_counts = &by["ARROW"];
    assert_eq!(flip_counts.get("Flip state1=Not flipped"), Some(&3));
    assert_eq!(flip_counts.get("Flip state1=Flipped"), Some(&2));

    let vis_counts = &by["DETECTOR"];
    assert_eq!(vis_counts.get("Visibility1=SMOKE"), Some(&5));
    assert_eq!(vis_counts.get("Visibility1=HEAT"), Some(&3));
    assert_eq!(vis_counts.get("Visibility1=DIFF"), Some(&2));
    assert_eq!(vis_counts.get("Visibility1=ADDR"), Some(&1));
}

/// Writes a phase-1 acceptance DWG for AcCoreConsole (set env `CADCRAFT_DYNBLOCK_FIXTURE` to output dir).
#[test]
#[ignore = "writes DWG + scr for manual AcCoreConsole; run with CADCRAFT_DYNBLOCK_FIXTURE set"]
fn write_phase1_fixture_for_autocad() {
    install_io();
    let out = std::env::var("CADCRAFT_DYNBLOCK_FIXTURE").expect("CADCRAFT_DYNBLOCK_FIXTURE");
    let dir = std::path::PathBuf::from(out);
    std::fs::create_dir_all(&dir).unwrap();
    let mut s = Session::new();
    s.execute("setvar", &json!({ "name": "INSUNITS", "value": 5 })).unwrap();

    let flip = arrow_block(&mut s);
    s.execute(
        "bparam.flip",
        &json!({ "block": flip, "line": [[15.0, -3.0], [15.0, 33.0]] }),
    )
    .unwrap();
    let vis = detector_block(&mut s);
    s.execute(
        "bparam.visibility",
        &json!({
            "block": vis,
            "default": "SMOKE",
            "states": { "SMOKE": [0,1,2,3,4,5], "HEAT": [0,1,2,3,6], "DIFF": [0,1,2,3,4], "ADDR": [0,1,2,3] }
        }),
    )
    .unwrap();
    for _ in 0..3 {
        s.execute("insert", &json!({ "name": flip, "at": [0, 0] })).unwrap();
    }
    for _ in 0..2 {
        let h = s.execute("insert", &json!({ "name": flip, "at": [40, 0] })).unwrap()["handle"].clone();
        s.execute(
            "blocks.reference.set",
            &json!({ "handle": h, "name": "Flip state1", "value": "Flipped" }),
        )
        .unwrap();
    }
    let plan: &[(&str, usize)] = &[("SMOKE", 5), ("HEAT", 3), ("DIFF", 2), ("ADDR", 1)];
    for (state, n) in plan {
        for _ in 0..*n {
            let h = s.execute("insert", &json!({ "name": vis, "at": [0, -50] })).unwrap()["handle"].clone();
            s.execute(
                "blocks.reference.set",
                &json!({ "handle": h, "name": "Visibility1", "value": state }),
            )
            .unwrap();
        }
    }
    let dwg = dir.join("phase1_by_param.dwg");
    s.execute("saveas", &json!({ "path": dwg.to_string_lossy() })).unwrap();
    let saveas2018 = dir.join("phase1_saveas2018.dwg");
    let scr = dir.join("phase1_audit.scr");
    std::fs::write(
        &scr,
        format!(
            "FILEDIA 0\nCMDDIA 0\nINSUNITS 5\nAUDIT Y\nLIST ALL\n\n\n\nSAVEAS 2018 {}\nQUIT Y\n",
            saveas2018.display()
        ),
    )
    .unwrap();
    eprintln!("wrote {} and {}", dwg.display(), scr.display());
}