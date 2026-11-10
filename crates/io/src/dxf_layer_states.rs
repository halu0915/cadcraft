//! Saved layer states in the form AutoCAD reads them: the `ACAD_LAYERSTATES` dictionary in the
//! LAYER table's extension dictionary, one XRECORD per state named by its dictionary key.
//!
//! A state's XRECORD (after `100 AcDbXrecord`, `280 1`):
//!
//! - `91` the properties restoring sets (AutoCAD's layer state mask, [`LayerState::ON`]…),
//!   `301` the description, `290` whether it restores as viewport overrides (written 0),
//!   `302` the layer current when it was saved;
//! - then per layer: `8` name, `90` flags (1 off, 2 frozen, 4 locked, 8 plottable, 16 frozen in
//!   new viewports), `62` colour index, `370` lineweight, `331` linetype (LTYPE handle), `1` plot
//!   style name, `440` transparency.
//!
//! CADCraft also keeps its states in its own `CADCRAFT_LAYERSTATES` record, which holds what the
//! XRECORD can't (true colours, layer descriptions). Reading takes the states from
//! `ACAD_LAYERSTATES` when the file has it, so a state AutoCAD changed or deleted reads as AutoCAD
//! left it, and fills each layer's other properties from CADCraft's record where that agrees with
//! the XRECORD.

use std::collections::{HashMap, HashSet};

use cadcraft_color::Color;
use cadcraft_doc::{Drawing, HVal, Layer, LayerState, Lineweight};
use cadcraft_dxf::Tag;

/// The dictionary key AutoCAD keeps layer states under.
pub(crate) const KEY: &str = "ACAD_LAYERSTATES";

/// Most layers read from one state's XRECORD.
const MAX_LAYERS: usize = 1_000_000;

/// Layer state flags (group 90).
const OFF: i64 = 1;
const FROZEN: i64 = 2;
const LOCKED: i64 = 4;
const PLOT: i64 = 8;
const NEW_VP_FROZEN: i64 = 16;

fn up(s: &str) -> String {
    s.trim().to_ascii_uppercase()
}

fn aci(c: Color) -> i64 {
    match c {
        Color::True(rgb) => i64::from(cadcraft_color::nearest_aci(rgb)),
        c => i64::from(c.to_aci()).clamp(1, 255),
    }
}

fn flags(l: &Layer) -> i64 {
    let bit = |on: bool, b: i64| if on { b } else { 0 };
    bit(!l.on, OFF) | bit(l.frozen, FROZEN) | bit(l.locked, LOCKED) | bit(l.plot, PLOT) | bit(l.vp_freeze_new, NEW_VP_FROZEN)
}

fn transparency(percent: u8) -> i64 {
    if percent == 0 { 0 } else { crate::dxf_ext::transparency_to_dxf(percent) }
}

/// Whether plot styles are colour-dependent (PSTYLEMODE 1, the default): then a layer's plot
/// style is its colour's, `Color_<n>`.
fn colour_dependent(d: &Drawing) -> bool {
    d.header.i64("PSTYLEMODE", 1) != 0
}

/// One record of a tag list: the index of its `0` group and the end (exclusive).
fn records(tags: &[Tag]) -> Vec<(usize, usize)> {
    let starts: Vec<usize> = tags.iter().enumerate().filter(|(_, t)| t.code == 0).map(|(i, _)| i).collect();
    starts.iter().enumerate().map(|(n, &s)| (s, starts.get(n + 1).copied().unwrap_or(tags.len()))).collect()
}

fn val(body: &[Tag], code: i32) -> Option<String> {
    body.iter().find(|t| t.code == code).map(Tag::str)
}

fn hex(h: &str) -> Option<u64> {
    u64::from_str_radix(h.trim(), 16).ok()
}

/// Write the drawing's layer states into the LAYER table's extension dictionary of a finished
/// tag list (after `dxf_raw::finish` has put the kept dictionaries back, without a kept
/// `ACAD_LAYERSTATES`, which is modelled). New objects get handles above every written one and
/// `$HANDSEED` moves past them.
pub(crate) fn write(tags: &mut Vec<Tag>, d: &Drawing) {
    if d.layer_states.is_empty() {
        return;
    }
    let recs = records(tags);
    let body = |r: &(usize, usize)| tags.get(r.0..r.1).unwrap_or(&[]);
    let kind = |r: &(usize, usize)| tags.get(r.0).map(Tag::str).unwrap_or_default();
    // Sections by record: the OBJECTS section's end is where new objects go.
    let mut sec = String::new();
    let mut layer_table: Option<(usize, usize)> = None;
    let mut objects_end: Option<usize> = None;
    let mut ltypes: HashMap<String, String> = HashMap::new();
    let mut dictionaries: HashMap<String, (usize, usize)> = HashMap::new();
    for r in &recs {
        let k = kind(r);
        let b = body(r);
        match k.as_str() {
            "SECTION" => sec = val(b, 2).unwrap_or_default(),
            "ENDSEC" => {
                if sec == "OBJECTS" {
                    objects_end = Some(r.0);
                }
                sec.clear();
            }
            "TABLE" if sec == "TABLES" && val(b, 2).is_some_and(|n| up(&n) == "LAYER") => layer_table = Some(*r),
            "LTYPE" if sec == "TABLES" => {
                if let (Some(n), Some(h)) = (val(b, 2), val(b, 5)) {
                    ltypes.entry(up(&n)).or_insert(h);
                }
            }
            "DICTIONARY" if sec == "OBJECTS" => {
                if let Some(h) = val(b, 5).and_then(|h| hex(&h)) {
                    dictionaries.insert(format!("{h:X}"), *r);
                }
            }
            _ => {}
        }
    }
    let (Some(table), Some(objects_end)) = (layer_table, objects_end) else { return };
    let table_body = body(&table);
    let Some(table_h) = val(table_body, 5) else { return };
    // The table's own extension dictionary, when it has one that was written.
    let xdict_at =
        table_body.windows(2).position(|w| w[0].code == 102 && w[0].str().trim().eq_ignore_ascii_case("{ACAD_XDICTIONARY") && w[1].code == 360);
    let xdict = xdict_at.and_then(|i| table_body.get(i + 1)).and_then(|t| hex(&t.str())).map(|h| format!("{h:X}"));
    let xdict_rec = xdict.as_ref().and_then(|h| dictionaries.get(h)).copied();

    let max = tags.iter().filter(|t| t.code == 5 || t.code == 105).filter_map(|t| hex(&t.str())).max().unwrap_or(0);
    let seed_at = tags.windows(2).position(|w| w[0].code == 9 && w[0].str() == "$HANDSEED" && w[1].code == 5).map(|i| i + 1);
    let seed = seed_at.and_then(|i| tags.get(i)).and_then(|t| hex(&t.str())).unwrap_or(0);
    let mut next = seed.max(max.saturating_add(1));
    let mut h = || {
        let v = next;
        next = next.saturating_add(1);
        format!("{v:X}")
    };
    let new_xdict = xdict_rec.is_none().then(&mut h);
    let xdict_h = match (&xdict_rec, &xdict, &new_xdict) {
        (Some(_), Some(x), _) => x.clone(),
        (_, _, Some(n)) => n.clone(),
        _ => return,
    };
    let states_h = h();
    let mut seen: HashSet<String> = HashSet::new();
    let states: Vec<(&LayerState, String)> =
        d.layer_states.iter().filter(|s| !s.name.trim().is_empty() && seen.insert(s.name.to_ascii_uppercase())).map(|s| (s, h())).collect();

    // The new objects.
    let mut out: Vec<Tag> = Vec::new();
    if let Some(x) = &new_xdict {
        out.extend([
            Tag::s(0, "DICTIONARY"),
            Tag::s(5, x.clone()),
            Tag::s(330, table_h.clone()),
            Tag::s(100, "AcDbDictionary"),
            Tag::i(280, 1),
            Tag::i(281, 1),
            Tag::s(3, KEY),
            Tag::s(360, states_h.clone()),
        ]);
    }
    out.extend([
        Tag::s(0, "DICTIONARY"),
        Tag::s(5, states_h.clone()),
        Tag::s(102, "{ACAD_REACTORS"),
        Tag::s(330, xdict_h.clone()),
        Tag::s(102, "}"),
        Tag::s(330, xdict_h.clone()),
        Tag::s(100, "AcDbDictionary"),
        Tag::i(281, 1),
    ]);
    for (s, rh) in &states {
        out.push(Tag::s(3, s.name.clone()));
        out.push(Tag::s(350, rh.clone()));
    }
    let current = d.header.get("CLAYER").and_then(HVal::as_str).unwrap_or("0").to_string();
    let continuous = ltypes.get("CONTINUOUS").cloned().unwrap_or_else(|| "0".into());
    let by_colour = colour_dependent(d);
    for (s, rh) in &states {
        out.extend([
            Tag::s(0, "XRECORD"),
            Tag::s(5, rh.clone()),
            Tag::s(102, "{ACAD_REACTORS"),
            Tag::s(330, states_h.clone()),
            Tag::s(102, "}"),
            Tag::s(330, states_h.clone()),
            Tag::s(100, "AcDbXrecord"),
            Tag::i(280, 1),
            Tag::i(91, i64::from(s.mask)),
            Tag::s(301, s.description.clone()),
            Tag::i(290, 0),
            Tag::s(302, current.clone()),
        ]);
        for l in s.layers.iter().filter(|l| !l.name.is_empty()) {
            let c = aci(l.color);
            let plot_style = if by_colour { format!("Color_{c}") } else { l.plot_style.clone() };
            out.extend([
                Tag::s(8, l.name.clone()),
                Tag::i(90, flags(l)),
                Tag::i(62, c),
                Tag::i(370, i64::from(l.lineweight.to_dxf())),
                Tag::s(331, ltypes.get(&up(&l.linetype)).cloned().unwrap_or_else(|| continuous.clone())),
                Tag::s(1, plot_style),
                Tag::i(440, transparency(l.transparency)),
            ]);
        }
    }

    // Where the table's extension dictionary group goes (and how many groups it replaces): over
    // a dangling one (its dictionary wasn't written), else after the handle and reactors, as the
    // DXF Reference orders them.
    let table_at = match xdict_at {
        Some(i) => (table.0 + i, 3),
        None => {
            let handle_at = table_body.iter().position(|t| t.code == 5).unwrap_or(1);
            let reactors_end = table_body
                .get(handle_at + 1)
                .filter(|t| t.code == 102 && t.str().trim().eq_ignore_ascii_case("{ACAD_REACTORS"))
                .and_then(|_| table_body.iter().skip(handle_at + 2).position(|t| t.code == 102).map(|p| handle_at + 2 + p));
            (table.0 + reactors_end.unwrap_or(handle_at) + 1, 0)
        }
    };
    // Insert from the end of the file backwards, so earlier positions stay valid.
    tags.splice(objects_end..objects_end, out);
    match xdict_rec {
        Some((start, end)) => {
            // The entry goes before the dictionary's xdata, if any.
            let at = tags.get(start..end).and_then(|t| t.iter().rposition(|g| g.code < 1000)).map_or(end, |i| start + i + 1);
            tags.splice(at..at, [Tag::s(3, KEY), Tag::s(360, states_h)]);
        }
        None => {
            let group = [Tag::s(102, "{ACAD_XDICTIONARY"), Tag::s(360, xdict_h), Tag::s(102, "}")];
            tags.splice(table_at.0..table_at.0 + table_at.1, group);
        }
    }
    if let Some(t) = seed_at.and_then(|i| tags.get_mut(i)) {
        *t = Tag::s(5, format!("{next:X}"));
    }
}

/// The owner of an object (its group 330 outside the reactors group).
fn owner(tags: &[Tag]) -> Option<String> {
    let mut in_reactors = false;
    for t in tags {
        match (t.code, t.str().trim()) {
            (102, "{ACAD_REACTORS") => in_reactors = true,
            (102, "}") => in_reactors = false,
            (330, v) if !in_reactors => return hex(v).map(|h| format!("{h:X}")),
            (100, _) => return None,
            _ => {}
        }
    }
    None
}

/// What a file's `ACAD_LAYERSTATES` dictionaries hold, as layer states in file order; `None`
/// when it has none. `names` maps dictionary entry handles (upper case) to their keys,
/// `xrecords` XRECORD handles to their groups, `ltypes` LTYPE handles to linetype names. A
/// layer's properties the XRECORD doesn't hold come from `ours` (CADCraft's record of the same
/// state), else the drawing's layer, where they agree with it.
pub(crate) fn read(
    d: &Drawing,
    names: &HashMap<String, String>,
    xrecords: &HashMap<String, Vec<Tag>>,
    ltypes: &HashMap<String, String>,
    ours: &[LayerState],
) -> Option<Vec<LayerState>> {
    let dicts: HashSet<String> =
        names.iter().filter(|(_, k)| k.trim().eq_ignore_ascii_case(KEY)).filter_map(|(h, _)| hex(h)).map(|h| format!("{h:X}")).collect();
    if dicts.is_empty() {
        return None;
    }
    let mut found: Vec<(u64, &String, &Vec<Tag>)> = xrecords
        .iter()
        .filter(|(_, t)| owner(t).is_some_and(|o| dicts.contains(&o)))
        .filter_map(|(h, t)| Some((hex(h)?, names.get(h)?, t)))
        .filter(|(_, n, _)| !n.trim().is_empty())
        .collect();
    found.sort_by_key(|(h, _, _)| *h);
    let by_colour = colour_dependent(d);
    let mut states: Vec<LayerState> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for (_, name, tags) in found.into_iter().take(crate::dxf_ext::MAX_LAYER_STATES) {
        if !seen.insert(name.to_ascii_uppercase()) {
            continue;
        }
        let mine = ours.iter().find(|s| s.name.eq_ignore_ascii_case(name));
        states.push(state(d, name, tags, ltypes, mine, by_colour));
    }
    Some(states)
}

/// One state from its XRECORD's groups.
fn state(d: &Drawing, name: &str, tags: &[Tag], ltypes: &HashMap<String, String>, mine: Option<&LayerState>, by_colour: bool) -> LayerState {
    let start = tags.iter().position(|t| t.code == 100 && t.str().trim() == "AcDbXrecord").map_or(0, |i| i + 1);
    let mut st = LayerState::new(name, Vec::new());
    let mut layer: Option<Layer> = None;
    let base = |n: &str| -> Layer {
        mine.and_then(|s| s.layers.iter().find(|l| l.name.eq_ignore_ascii_case(n)))
            .or_else(|| d.layer(n))
            .cloned()
            .map(|mut l| {
                l.name = n.to_string();
                l
            })
            .unwrap_or_else(|| Layer::new(n))
    };
    for t in tags.iter().skip(start) {
        let v = t.str();
        match (t.code, layer.as_mut()) {
            (91, None) => st.mask = u32::try_from(t.i64()).map_or(LayerState::ALL, |m| m & LayerState::ALL),
            (301, None) => st.description = v,
            (8, _) => {
                if v.is_empty() || st.layers.len() >= MAX_LAYERS {
                    layer = None;
                    continue;
                }
                st.layers.extend(layer.take());
                layer = Some(base(&v));
            }
            (90, Some(l)) => {
                let f = t.i64();
                l.on = f & OFF == 0;
                l.frozen = f & FROZEN != 0;
                l.locked = f & LOCKED != 0;
                l.plot = f & PLOT != 0;
                l.vp_freeze_new = f & NEW_VP_FROZEN != 0;
            }
            (62, Some(l)) => {
                let c = t.i64();
                if c < 0 {
                    l.on = false;
                }
                let c = i64::try_from(c.unsigned_abs()).unwrap_or(7).clamp(1, 255);
                if aci(l.color) != c {
                    l.color = Color::Index(u8::try_from(c).unwrap_or(7));
                }
            }
            (370, Some(l)) => {
                if let Ok(w) = i16::try_from(t.i64())
                    && l.lineweight.to_dxf() != w
                {
                    l.lineweight = Lineweight::from_dxf(w);
                }
            }
            (331, Some(l)) => {
                if let Some(n) = ltypes.get(&up(&v)).or_else(|| hex(&v).and_then(|h| ltypes.get(&format!("{h:X}"))))
                    && !n.eq_ignore_ascii_case(&l.linetype)
                {
                    l.linetype = n.clone();
                }
            }
            (6, Some(l)) if !v.trim().is_empty() && !v.eq_ignore_ascii_case(&l.linetype) => l.linetype = v,
            (1, Some(l)) if !by_colour && !v.trim().is_empty() => l.plot_style = v,
            (440, Some(l)) => {
                let x = t.i64();
                if transparency(l.transparency) != x {
                    l.transparency = crate::dxf_ext::transparency_from_dxf(x).unwrap_or(0);
                }
            }
            _ => {}
        }
    }
    st.layers.extend(layer);
    st
}
