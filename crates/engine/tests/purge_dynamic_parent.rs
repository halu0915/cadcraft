//! PURGE keeps a dynamic block that only anonymous references use. AutoCAD draws a dynamic
//! block reference with values of its own through an anonymous block (`*U7`) whose BLOCK_RECORD
//! has `AcDbBlockRepBTag` xdata naming the dynamic block's record. Nothing inserts the dynamic
//! block by name, yet purging it leaves every `*U` without its dynamic definition. Blocks the
//! dynamic block's own definition uses stay too; blocks nothing uses are still purged.

use std::sync::Arc;

use cadcraft_engine::Session;
use cadcraft_engine::doc::foreign::{Foreign, ForeignExtra};
use cadcraft_engine::doc::{Block, Circle, Entity, EntityKind, Handle, Insert, RawTag};
use cadcraft_engine::geom::Vec3;
use serde_json::json;

fn block(name: &str, h: u64, kids: Vec<EntityKind>) -> Arc<Block> {
    let mut b = Block::new(name);
    b.anonymous = name.starts_with('*');
    b.entities.push(Entity::new(Handle(h), EntityKind::Circle(Circle { center: Vec3::ZERO, radius: 0.5 })));
    for (i, k) in kids.into_iter().enumerate() {
        b.entities.push(Entity::new(Handle(h + 1 + i as u64), k));
    }
    Arc::new(b)
}

fn insert(name: &str) -> EntityKind {
    EntityKind::Insert(Insert { block: name.into(), ..Insert::default() })
}

/// Door is dynamic (record handle 1D) and holds a reference to Hinge; *U7 is a Door reference
/// with values of its own, inserted twice in model space; Unused is referenced by nothing.
fn session() -> Session {
    let mut s = Session::new();
    let d = s.doc_mut().unwrap();
    d.blocks.insert("Hinge".into(), block("Hinge", 0x9000, vec![]));
    d.blocks.insert("Door".into(), block("Door", 0x9010, vec![insert("Hinge")]));
    d.blocks.insert("*U7".into(), block("*U7", 0x9020, vec![]));
    d.blocks.insert("Unused".into(), block("Unused", 0x9030, vec![]));
    let mut f = Foreign::default();
    let tag = |code, value: &str| RawTag { code, value: value.into() };
    f.records.insert(
        ("BLOCK_RECORD".into(), "*U7".into()),
        ForeignExtra { xdata: vec![tag(1001, "AcDbBlockRepBTag"), tag(1070, "1"), tag(1005, "1D")], ..Default::default() },
    );
    f.targets.insert(Handle(0x1D), ("BLOCK_RECORD".into(), "DOOR".into()));
    d.foreign = Arc::new(f);
    for (i, x) in [0.0, 50.0].into_iter().enumerate() {
        let mut k = insert("*U7");
        if let EntityKind::Insert(ins) = &mut k {
            ins.insert = Vec3::new(x, 0.0, 0.0);
        }
        d.model.push(Entity::new(Handle(0xA000 + i as u64), k));
    }
    s
}

#[test]
fn purge_keeps_a_dynamic_block_its_anonymous_references_name() {
    let mut s = session();
    let r = s.execute("purge", &json!({ "type": "blocks" })).unwrap();
    assert_eq!(r["purged"]["blocks"], json!(["Unused"]), "{r}");
    let d = s.doc().unwrap();
    for name in ["Door", "Hinge", "*U7"] {
        assert!(d.block(name).is_some(), "{name} was purged");
    }
    // The *U7 link back to Door is still there.
    let rec = d.foreign.records.get(&("BLOCK_RECORD".to_string(), "*U7".to_string())).expect("*U7 record");
    assert!(rec.xdata.iter().any(|t| t.code == 1001 && t.value == "AcDbBlockRepBTag"));
    assert_eq!(d.foreign.targets.get(&Handle(0x1D)).map(|(_, b)| b.as_str()), Some("DOOR"));
}

#[test]
fn purge_removes_the_dynamic_block_once_no_anonymous_reference_is_left() {
    let mut s = session();
    let m = &mut s.doc_mut().unwrap().model;
    for h in m.handles() {
        m.remove(h);
    }
    let r = s.execute("purge", &json!({ "type": "blocks" })).unwrap();
    let mut purged: Vec<String> = serde_json::from_value(r["purged"]["blocks"].clone()).unwrap();
    purged.sort();
    assert_eq!(purged, ["*U7", "Door", "Hinge", "Unused"], "{r}");
}
