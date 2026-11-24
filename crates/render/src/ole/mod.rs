//! The picture an OLE object (OLE2FRAME) shows: its presentation, drawn in its frame.
//!
//! The OLE data (DXF group 310) is a short header followed by a compound file ([MS-CFB]) holding
//! the object's streams. A presentation stream (`\x02OlePres000`, [MS-OLEDS] 2.3.4) carries what
//! the object looks like, as a Windows metafile ([MS-WMF]); an enhanced metafile ([MS-EMF]) is
//! carried inside it in comment records, and that is what is played, since it keeps the text as
//! Unicode. Data that is not a compound file, has no presentation or no enhanced metafile draws
//! nothing (the frame alone, as before).

mod cfb;
mod emf;

use super::{Builder, Ctx, Ink};
use cadcraft_color::{Color, Rgb};
use cadcraft_geom::Vec2;

/// The most OLE data bytes decoded from one entity.
const MAX_OLE_BYTES: usize = 64 << 20;
/// The text height CADCraft draws (a cap height) over the em height a metafile gives.
const CAP_PER_EM: f64 = 0.7;

/// The OLE data of an OLE2FRAME's tags: its 310 groups' hex, outside application groups.
fn payload(tags: &[cadcraft_doc::RawTag]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut in_app = false;
    for t in tags {
        if t.code == 102 {
            in_app = t.value.starts_with('{');
            continue;
        }
        if in_app || t.code != 310 {
            continue;
        }
        let s = t.value.trim().as_bytes();
        if s.len() % 2 != 0 || out.len().saturating_add(s.len() / 2) > MAX_OLE_BYTES {
            return None;
        }
        for pair in s.as_chunks::<2>().0 {
            let hex = |c: u8| (c as char).to_digit(16);
            let (Some(h), Some(l)) = (pair.first().copied().and_then(hex), pair.get(1).copied().and_then(hex)) else { return None };
            out.push((h * 16 + l) as u8);
        }
    }
    (!out.is_empty()).then_some(out)
}

fn u32_at(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(o..o.checked_add(4)?)?.try_into().ok()?))
}

fn u16_at(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(d.get(o..o.checked_add(2)?)?.try_into().ok()?))
}

/// The clipboard format and data of a presentation stream ([MS-OLEDS] 2.3.4).
fn presentation(s: &[u8]) -> Option<(u32, &[u8])> {
    let marker = u32_at(s, 0)?;
    let (format, mut at) = match marker {
        0 => (0, 4),
        0xFFFF_FFFF | 0xFFFF_FFFE => (u32_at(s, 4)?, 8),
        // A registered format named by an ANSI string.
        n => (0, 4usize.checked_add(n as usize)?),
    };
    let device = u32_at(s, at)? as usize;
    at = at.checked_add(device.max(4))?;
    // Aspect, Lindex, Advf, Reserved1, Width, Height, then Size.
    let size = u32_at(s, at.checked_add(24)?)? as usize;
    let start = at.checked_add(28)?;
    let data = s.get(start..start.checked_add(size)?.min(s.len()))?;
    Some((format, data))
}

/// The enhanced metafile a Windows metafile carries in its comment records ([MS-WMF]
/// META_ESCAPE / MFCOMMENT with the "WMFC" signature), joined in order.
fn emf_in_wmf(w: &[u8]) -> Option<Vec<u8>> {
    // A placeable metafile starts with a 22-byte header.
    let start = if u32_at(w, 0)? == 0x9AC6_CDD7 { 22 } else { 0 };
    let header_words = usize::from(u16_at(w, start + 2)?);
    let mut pos = start + header_words.max(9) * 2;
    let mut out = Vec::new();
    let mut total = None;
    while let (Some(words), Some(func)) = (u32_at(w, pos), u16_at(w, pos + 4)) {
        let len = (words as usize).checked_mul(2)?;
        if len < 6 {
            break;
        }
        let Some(r) = w.get(pos..pos.saturating_add(len)) else { break };
        pos += len;
        if func == 0 {
            break;
        }
        // META_ESCAPE, MFCOMMENT, then the comment: "WMFC", ..., this chunk's size, the whole
        // metafile's size, the chunk.
        if func == 0x0626 && u16_at(r, 6) == Some(0x000F) && r.get(10..14) == Some(b"WMFC") {
            let (Some(cur), Some(size)) = (u32_at(r, 10 + 22), u32_at(r, 10 + 30)) else { continue };
            let size = size as usize;
            if size > MAX_OLE_BYTES {
                return None;
            }
            total.get_or_insert(size);
            let chunk = r.get(10 + 34..(10 + 34usize).saturating_add(cur as usize).min(r.len())).unwrap_or_default();
            if out.len().saturating_add(chunk.len()) > size {
                break;
            }
            out.extend_from_slice(chunk);
        }
    }
    (!out.is_empty()).then_some(out)
}

/// Text in an ANSI metafile record, in the code page of the font's character set.
pub(crate) fn decode_ansi(bytes: &[u8], charset: u8) -> String {
    let enc = match charset {
        128 => encoding_rs::SHIFT_JIS,
        129 => encoding_rs::EUC_KR,
        134 => encoding_rs::GBK,
        136 => encoding_rs::BIG5,
        161 => encoding_rs::WINDOWS_1253,
        162 => encoding_rs::WINDOWS_1254,
        177 => encoding_rs::WINDOWS_1255,
        178 => encoding_rs::WINDOWS_1256,
        186 => encoding_rs::WINDOWS_1257,
        204 => encoding_rs::WINDOWS_1251,
        222 => encoding_rs::WINDOWS_874,
        238 => encoding_rs::WINDOWS_1250,
        _ => encoding_rs::WINDOWS_1252,
    };
    enc.decode_without_bom_handling(bytes).0.into_owned()
}

/// The enhanced metafile an OLE object presents, from its OLE data.
fn emf_of(data: &[u8]) -> Option<Vec<u8>> {
    const MAGIC: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
    let at = data.windows(8).position(|w| w == MAGIC)?;
    let wanted = |n: &str| n.starts_with("\u{2}OlePres") || n == "\u{3}EPRINT" || n.eq_ignore_ascii_case("CONTENTS");
    let mut streams = cfb::streams(data.get(at..)?, wanted)?;
    streams.sort_by(|a, b| a.name.cmp(&b.name));
    let is_emf = |d: &[u8]| u32_at(d, 0) == Some(1) && d.get(40..44) == Some(b" EMF");
    for s in streams.iter().filter(|s| s.name.starts_with("\u{2}OlePres")) {
        if let Some((format, d)) = presentation(&s.data) {
            // CF_METAFILEPICT and CF_ENHMETAFILE store a Windows metafile; CF_DIB a bitmap.
            if (format == 3 || format == 14)
                && let Some(e) = emf_in_wmf(d)
            {
                return Some(e);
            }
            if is_emf(d) {
                return Some(d.to_vec());
            }
        }
    }
    // A static picture's metafile stream.
    streams.into_iter().find(|s| (s.name == "\u{3}EPRINT" || s.name.eq_ignore_ascii_case("CONTENTS")) && is_emf(&s.data)).map(|s| s.data)
}

/// The most pictures kept decoded, and the most points they hold together.
const CACHE_PICTURES: usize = 256;
const CACHE_POINTS: usize = 8_000_000;

#[derive(Default)]
struct Cache {
    pictures: std::collections::HashMap<u64, std::sync::Arc<Option<emf::Picture>>>,
    points: usize,
}

fn cache() -> &'static std::sync::RwLock<Cache> {
    static CACHE: std::sync::OnceLock<std::sync::RwLock<Cache>> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| std::sync::RwLock::new(Cache::default()))
}

/// A hash of the OLE data groups of `tags` (what [`payload`] decodes).
fn data_key(tags: &[cadcraft_doc::RawTag]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let mut in_app = false;
    for t in tags {
        if t.code == 102 {
            in_app = t.value.starts_with('{');
        } else if !in_app && t.code == 310 {
            t.value.trim().hash(&mut h);
        }
    }
    h.finish()
}

fn picture_points(p: &emf::Picture) -> usize {
    p.items
        .iter()
        .map(|i| match i {
            emf::Item::Line { pts, .. } => pts.len(),
            emf::Item::Fill { rings, .. } => rings.iter().map(Vec::len).sum(),
            emf::Item::Text(t) => t.advances.len() + 1,
        })
        .sum()
}

/// The picture of the OLE object whose tags are `tags`, played in device units. The same OLE data
/// (an OLE object in a block inserted many times, copies of one object) is decoded once and kept.
fn picture(tags: &[cadcraft_doc::RawTag]) -> std::sync::Arc<Option<emf::Picture>> {
    let key = data_key(tags);
    if let Some(p) = cache().read().ok().and_then(|c| c.pictures.get(&key).cloned()) {
        return p;
    }
    let decoded = std::sync::Arc::new(payload(tags).and_then(|d| emf_of(&d)).and_then(|e| emf::play(&e)));
    let points = decoded.as_ref().as_ref().map_or(0, picture_points);
    if let Ok(mut c) = cache().write() {
        if c.pictures.len() >= CACHE_PICTURES || c.points.saturating_add(points) > CACHE_POINTS {
            c.pictures.clear();
            c.points = 0;
        }
        if points <= CACHE_POINTS {
            c.points += points;
            c.pictures.insert(key, decoded.clone());
        }
    }
    decoded
}

/// Whether the OLE object whose tags are `tags` holds a picture CADCraft draws in its frame (an
/// enhanced metafile presentation that plays to at least one line, fill or text).
pub fn ole_picture_drawn(tags: &[cadcraft_doc::RawTag]) -> bool {
    picture(tags).as_ref().as_ref().is_some_and(|p| !p.items.is_empty())
}

/// The ink an item of the picture draws with: black as colour 7 (black on paper, white on a dark
/// screen), other colours as they are. A white fill is the picture's background and is left out
/// (`None` for `fill`); white lines and text are drawn, as they sit on a darker fill.
fn item_ink(c: emf::Rgb, fill: bool) -> Option<Ink> {
    match c {
        emf::Rgb(255, 255, 255) if fill => None,
        emf::Rgb(0, 0, 0) => Some(super::ink(Color::Index(7), Color::Index(7), Color::Index(7))),
        emf::Rgb(r, g, b) => Some(Ink { rgb: Rgb(r, g, b), aci7: false, aci: 0 }),
    }
}

/// Draw the presentation of an OLE2FRAME in its frame (`frame`: upper left, upper right, lower
/// right, lower left, upper left). Nothing is drawn when the data holds no picture that can be
/// played.
pub(crate) fn draw(b: &mut Builder, ctx: &Ctx, tags: &[cadcraft_doc::RawTag], frame: &[Vec2; 5], lw: f32) {
    let (ul, lr) = (frame[0], frame[2]);
    let decoded = picture(tags);
    let Some(pic) = decoded.as_ref() else { return };
    let [x0, y0, x1, y1] = pic.rect;
    let (dw, dh) = (x1 - x0, y1 - y0);
    if !(dw > 0.0 && dh > 0.0) || !(ul.x.is_finite() && ul.y.is_finite() && lr.x.is_finite() && lr.y.is_finite()) {
        return;
    }
    let (sx, sy) = ((lr.x - ul.x) / dw, (ul.y - lr.y) / dh);
    if sx == 0.0 || sy == 0.0 {
        return;
    }
    let map = |p: Vec2| Vec2::new(ul.x + (p.x - x0) * sx, ul.y - (p.y - y0) * sy);
    let mut fonts: std::collections::HashMap<String, cadcraft_fonts::TextFont> = std::collections::HashMap::new();
    for it in &pic.items {
        match it {
            emf::Item::Line { pts, rgb } => {
                if let Some(ink) = item_ink(*rgb, false) {
                    let pts: Vec<Vec2> = pts.iter().map(|p| map(*p)).collect();
                    b.polyline(ctx, ink, lw, &pts);
                }
            }
            emf::Item::Fill { rings, rgb, winding } => {
                let Some(ink) = item_ink(*rgb, true) else { continue };
                let rings: Vec<Vec<Vec2>> = rings.iter().map(|r| r.iter().map(|p| map(*p)).collect()).collect();
                let bb = cadcraft_geom::Bounds2::from_points(rings.iter().flatten().copied());
                let (w, h) = (bb.max.x - bb.min.x, bb.max.y - bb.min.y);
                // A one-ring fill a pixel or less across (a ruled line drawn as a fill) is drawn as
                // its centre line.
                if let [ring] = rings.as_slice()
                    && ring.len() == 4
                    && (w <= sx.abs() * 1.01 || h <= sy.abs() * 1.01)
                {
                    let c = Vec2::new((bb.min.x + bb.max.x) / 2.0, (bb.min.y + bb.max.y) / 2.0);
                    let line = if w <= h {
                        [Vec2::new(c.x, bb.min.y), Vec2::new(c.x, bb.max.y)]
                    } else {
                        [Vec2::new(bb.min.x, c.y), Vec2::new(bb.max.x, c.y)]
                    };
                    b.polyline(ctx, ink, lw, &line);
                } else {
                    // All rings of one record make one region: alternate (even-odd) or winding
                    // (nonzero) fill, as the metafile's polygon fill mode says.
                    let tris = if *winding { super::text::triangulate_placed(&rings) } else { super::fill::triangulate_evenodd(&rings) };
                    b.tris(ctx, ink, &tris);
                }
            }
            emf::Item::Text(t) => {
                if !b.opts.text {
                    continue;
                }
                let Some(ink) = item_ink(t.rgb, false) else { continue };
                let font = fonts.entry(t.face.clone()).or_insert_with(|| cadcraft_fonts::TextFont::resolve(&t.face)).clone();
                text(b, ctx, &font, t, &map, sy.abs(), sx.abs() / sy.abs(), ink, lw);
            }
        }
    }
}

/// One string of the picture: placed character by character when the metafile gives each
/// character's advance (so it keeps the picture's layout in whatever font draws it), else whole.
#[allow(clippy::too_many_arguments)]
fn text(
    b: &mut Builder,
    ctx: &Ctx,
    font: &cadcraft_fonts::TextFont,
    t: &emf::Text,
    map: &impl Fn(Vec2) -> Vec2,
    scale: f64,
    aspect: f64,
    ink: Ink,
    lw: f32,
) {
    let height = t.em * scale * CAP_PER_EM;
    if !(height > 0.0 && height.is_finite()) {
        return;
    }
    let start = map(t.at);
    let along = map(t.at + t.dir) - start;
    let rotation = along.y.atan2(along.x);
    let unit = along.normalized();
    let width_factor = (t.width_factor * aspect).clamp(0.01, 100.0);
    let params = |at: Vec2| cadcraft_fonts::TextParams { rotation, width_factor, ..cadcraft_fonts::TextParams::new(at, height) };
    if t.advances.is_empty() {
        let (sh, _) = cadcraft_fonts::place(font, &t.text, &params(start));
        let shift = match t.halign {
            1 => sh.width / 2.0,
            2 => sh.width,
            _ => 0.0,
        };
        if shift == 0.0 {
            b.shaped(ctx, ink, lw, &sh);
        } else {
            let (sh, _) = cadcraft_fonts::place(font, &t.text, &params(start - unit * shift));
            b.shaped(ctx, ink, lw, &sh);
        }
        return;
    }
    let k = along.len();
    let total: f64 = t.advances.iter().sum::<f64>() * k;
    let shift = match t.halign {
        1 => total / 2.0,
        2 => total,
        _ => 0.0,
    };
    let mut at = start - unit * shift;
    let mut buf = [0u8; 4];
    for (c, adv) in t.text.chars().zip(&t.advances) {
        if !c.is_whitespace() {
            let (sh, _) = cadcraft_fonts::place(font, c.encode_utf8(&mut buf), &params(at));
            b.shaped(ctx, ink, lw, &sh);
        }
        at += unit * (adv * k);
    }
}

#[cfg(test)]
mod tests;
