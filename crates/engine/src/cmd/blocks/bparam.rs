//! `bparam.*`: author dynamic-block parameters on an ordinary block (phase 1: flip, visibility).

use std::path::Path;
use std::sync::Arc;

use cadcraft_doc::{Drawing, Handle};
use cadcraft_dxf::Version;
use cadcraft_geom::Vec2;
use cadcraft_io::{SaveOptions, read, write_with};
use serde_json::{Value, json};

use super::{bad, has_doc, point_param, point_req, str_param};
use crate::{CommandSpec, Result, Session};

fn save_dwg(d: &Drawing) -> Result<Vec<u8>> {
    write_with(d, Path::new("drawing.dwg"), &SaveOptions { version: Some(Version::R2018), ..SaveOptions::default() })
        .map_err(|e| bad("bparam", e.to_string()))
}

fn reload_dwg(s: &mut Session, bytes: &[u8]) -> Result<()> {
    let d = read(bytes, Path::new("drawing.dwg")).map_err(|e| bad("bparam", e.to_string()))?;
    let st = s.state_mut()?;
    st.doc = Arc::new(d);
    s.bump_doc_epoch();
    Ok(())
}

fn flip_line(cmd: &str, p: &Value) -> Result<([f64; 2], [f64; 2])> {
    if let Some(line) = p.get("line").and_then(|v| v.as_array()) {
        if line.len() != 2 {
            return Err(bad(cmd, "`line` must be [[x1,y1],[x2,y2]]"));
        }
        let a = crate::cmd::point_value(&line[0]).ok_or_else(|| bad(cmd, "`line[0]` must be [x,y]"))?;
        let b = crate::cmd::point_value(&line[1]).ok_or_else(|| bad(cmd, "`line[1]` must be [x,y]"))?;
        return Ok(([a.x, a.y], [b.x, b.y]));
    }
    let base = point_req(cmd, p, "base")?;
    let end = point_req(cmd, p, "end")?;
    Ok(([base.x, base.y], [end.x, end.y]))
}

fn entity_index(d: &Drawing, block: &str, cmd: &str, v: &Value) -> Result<usize> {
    let b = d.block(block).ok_or_else(|| bad(cmd, format!("no block \"{block}\"")))?;
    if let Some(i) = v.as_u64() {
        let i = usize::try_from(i).map_err(|_| bad(cmd, "entity index out of range"))?;
        return (i < b.entities.len()).then_some(i).ok_or_else(|| bad(cmd, format!("entity index {i} out of range ({} entities)", b.entities.len())));
    }
    let hex = v.as_str().ok_or_else(|| bad(cmd, "state entities must be handles (hex strings) or indexes"))?;
    let h = Handle::parse_hex(hex).ok_or_else(|| bad(cmd, format!("\"{hex}\" is not a handle")))?;
    b.entities
        .iter()
        .position(|e| e.handle == h)
        .ok_or_else(|| bad(cmd, format!("block \"{block}\" has no entity {hex}")))
}

fn visibility_states(cmd: &str, d: &Drawing, block: &str, p: &Value) -> Result<Vec<(String, Vec<usize>)>> {
    let states = p.get("states").ok_or_else(|| bad(cmd, "`states` is required"))?;
    let mut out = Vec::new();
    match states {
        Value::Object(map) => {
            for (name, ents) in map {
                let arr = ents.as_array().ok_or_else(|| bad(cmd, format!("state \"{name}\": entities must be an array")))?;
                let idx: Vec<usize> = arr.iter().map(|v| entity_index(d, block, cmd, v)).collect::<Result<_>>()?;
                out.push((name.clone(), idx));
            }
        }
        Value::Array(rows) => {
            for row in rows {
                let name = row
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| bad(cmd, "each state needs `name`"))?
                    .to_string();
                let arr = row.get("entities").or_else(|| row.get("handles")).ok_or_else(|| bad(cmd, format!("state \"{name}\": `entities` array required")))?;
                let idx: Vec<usize> = arr.as_array().ok_or_else(|| bad(cmd, format!("state \"{name}\": entities must be an array")))?.iter().map(|v| entity_index(d, block, cmd, v)).collect::<Result<_>>()?;
                out.push((name, idx));
            }
        }
        _ => return Err(bad(cmd, "`states` must be an object or array")),
    }
    if out.is_empty() {
        return Err(bad(cmd, "at least one visibility state is required"));
    }
    if let Some(def) = p.get("default").and_then(Value::as_str) {
        if let Some(pos) = out.iter().position(|(n, _)| n.eq_ignore_ascii_case(def)) {
            let item = out.remove(pos);
            out.insert(0, item);
        }
    }
    Ok(out)
}

fn run_flip(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "bparam.flip";
    let block = str_param(p, "block").ok_or_else(|| bad(CMD, "`block` is required"))?;
    let (base, end) = flip_line(CMD, p)?;
    let bytes = save_dwg(s.doc()?)?;
    let out = cadcraft_dwg::dynblock_author::flip_dwg_bytes(&bytes, block, base, end).map_err(|e| bad(CMD, e))?;
    reload_dwg(s, &out)?;
    Ok(json!({ "block": block, "message": format!("Block \"{block}\" now has a flip parameter.") }))
}

fn run_visibility(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "bparam.visibility";
    let block = str_param(p, "block").ok_or_else(|| bad(CMD, "`block` is required"))?;
    let states = visibility_states(CMD, s.doc()?, block, p)?;
    let grip = point_param(p, "grip").unwrap_or(Vec2::new(33.0, 30.0));
    let bytes = save_dwg(s.doc()?)?;
    let out = cadcraft_dwg::dynblock_author::visibility_dwg_bytes(
        &bytes,
        block,
        &states,
        [grip.x, grip.y],
    )
    .map_err(|e| bad(CMD, e))?;
    reload_dwg(s, &out)?;
    let names: Vec<_> = states.iter().map(|(n, _)| n.clone()).collect();
    Ok(json!({ "block": block, "states": names, "message": format!("Block \"{block}\" now has visibility states: {}.", names.join(", ")) }))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("bparam.flip", "Dynamic Block Flip", run_flip)
            .params("{block, line?: [[x1,y1],[x2,y2]] | base: [x,y], end: [x,y]} → {block}")
            .enabled(has_doc),
        CommandSpec::new("bparam.visibility", "Dynamic Block Visibility", run_visibility)
            .params("{block, states: {name: [handle|index,...]} | [{name, entities}], default?, grip?: [x,y]} → {block, states}")
            .enabled(has_doc),
    ]
}