//! Spike probe (not shipped): dump the dynamic-block object tree of one block definition.
//!
//! `cargo run -p cadcraft-dwg --example dynblock_probe -- dump <file.dwg> [block-name-substring]`
//! `cargo run -p cadcraft-dwg --example dynblock_probe -- list <file.dwg>`
//!
//! Walks BLOCK_RECORD -> extension dictionary -> ACAD_ENHANCEDBLOCK -> evaluation graph ->
//! node expressions, printing each object's typed acadrust form, so the minimal object set
//! for authoring can be read off a real AutoCAD-made block.

use acadrust::objects::{DynamicBlockData, ObjectType};
use acadrust::{CadDocument, DwgReader, Handle};
use std::collections::{BTreeSet, VecDeque};

fn read(path: &str) -> CadDocument {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    DwgReader::from_stream(std::io::Cursor::new(bytes)).read().expect("parse DWG")
}

fn kind(doc: &CadDocument, h: Handle) -> String {
    match doc.objects.get(&h) {
        Some(ObjectType::DynamicBlock(o)) => o.dxf_name.clone(),
        Some(ObjectType::Dictionary(_)) => "DICTIONARY".into(),
        Some(ObjectType::XRecord(_)) => "XRECORD".into(),
        Some(ObjectType::BlockVisibilityParameter(_)) => "BLOCKVISIBILITYPARAMETER".into(),
        Some(o) => format!("{:?}", std::mem::discriminant(o)),
        None => match doc.get_entity(h) {
            Some(e) => format!("entity {}", e.as_entity().entity_type()),
            None => "?".into(),
        },
    }
}

/// Handles an object refers to (owner excluded), for the tree walk.
fn children(o: &ObjectType) -> Vec<Handle> {
    let mut out = Vec::new();
    match o {
        ObjectType::Dictionary(d) => out.extend(d.entries.iter().map(|(_, h)| *h)),
        ObjectType::DynamicBlock(d) => {
            let mut c = d.clone();
            let owner = d.owner;
            c.visit_handles_mut(&mut |h: &mut Handle| out.push(*h));
            out.retain(|h| *h != owner);
        }
        _ => {}
    }
    if let ObjectType::DynamicBlock(d) = o {
        out.retain(|h| !d.reactors.contains(h));
    }
    out
}

fn dump_tree(doc: &CadDocument, root: Handle, label: &str) {
    println!("==== {label} root {root:?}");
    let mut seen = BTreeSet::new();
    let mut q = VecDeque::from([root]);
    while let Some(h) = q.pop_front() {
        if h.is_null() || !seen.insert(h.value()) {
            continue;
        }
        match doc.objects.get(&h) {
            Some(o) => {
                println!("--- {:X} {}\n{:?}", h.value(), kind(doc, h), o);
                q.extend(children(o));
            }
            None => println!("--- {:X} {} (not an object)", h.value(), kind(doc, h)),
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (cmd, path) = (args.get(1).map(String::as_str), args.get(2).expect("file"));
    let doc = read(path);
    let pick = args.get(3).cloned().unwrap_or_default();
    if cmd == Some("classes") {
        for c in doc.classes.iter() {
            println!("{c:?}");
        }
        return;
    }
    for br in doc.block_records.iter() {
        let Some(xd) = doc.extension_dictionary_handle(br.handle) else { continue };
        let Some(ObjectType::Dictionary(d)) = doc.objects.get(&xd) else { continue };
        let Some((_, eb)) = d.entries.iter().find(|(k, _)| k == "ACAD_ENHANCEDBLOCK") else { continue };
        let mut counts = std::collections::BTreeMap::<String, usize>::new();
        if let Some(ObjectType::DynamicBlock(g)) = doc.objects.get(eb)
            && let DynamicBlockData::EvaluationGraph(graph) = &g.data
        {
            for n in &graph.nodes {
                *counts.entry(kind(&doc, n.expression)).or_default() += 1;
            }
        }
        match cmd {
            Some("list") => println!("{:?} br={:X} ents={} nodes={:?}", br.name, br.handle.value(), br.entity_handles.len(), counts),
            Some("dump") if br.name.contains(&pick) => {
                println!(
                    "#### BLOCK {:?} br={:X} base={:?} flags={:?} ents={:?}",
                    br.name,
                    br.handle.value(),
                    br.base_point,
                    br.flags,
                    br.entity_handles
                );
                println!("xdata: {:?}", doc.object_xdata(br.handle));
                for e in &br.entity_handles {
                    println!("  ent {:X} {}", e.value(), kind(&doc, *e));
                }
                dump_tree(&doc, xd, "block xdict");
                // A few inserts of this definition with their representation subtree.
                for ins in br.insert_handles.iter().take(2) {
                    if let Some(x) = doc.extension_dictionary_handle(*ins) {
                        dump_tree(&doc, x, &format!("insert {:X} xdict", ins.value()));
                    }
                }
                return;
            }
            _ => {}
        }
    }
    if cmd == Some("dump") {
        // Root-level dynamic-block bookkeeping (purge preventer etc.)
        for (h, o) in &doc.objects {
            if let ObjectType::DynamicBlock(d) = o
                && d.dxf_name.contains("PURGEPREVENTER")
            {
                println!("{:X} {:#?}", h.value(), d);
            }
        }
    }
}
