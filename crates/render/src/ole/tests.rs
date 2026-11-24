//! OLE2FRAME pictures, from synthetic OLE data built here: an enhanced metafile inside a Windows
//! metafile inside a presentation stream inside a compound file, as AutoCAD stores an embedded
//! spreadsheet.

use super::*;
use crate::{Options, build};
use cadcraft_doc::{Common, Drawing, EntityKind, RawTag, Space, Unknown};

fn le32(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_le_bytes());
}
fn rec(t: u32, body: &[u8]) -> Vec<u8> {
    let mut v = Vec::new();
    le32(&mut v, t);
    le32(&mut v, (8 + body.len()) as u32);
    v.extend_from_slice(body);
    v
}
fn ints(xs: &[i32]) -> Vec<u8> {
    xs.iter().flat_map(|x| x.to_le_bytes()).collect()
}

/// A 100 × 50 pixel metafile (10 pixels per millimetre) drawing a black line from (10, 10) to
/// (90, 10), a solid red rectangle and the text "負載" with its top at (20, 20).
fn sample_emf() -> Vec<u8> {
    let mut body = Vec::new();
    body.extend(rec(38, &ints(&[1, 0, 0, 0, 0]))); // CREATEPEN 1: solid, black
    body.extend(rec(37, &ints(&[1])));
    body.extend(rec(27, &ints(&[10, 10]))); // MOVETOEX
    body.extend(rec(54, &ints(&[90, 10]))); // LINETO
    body.extend(rec(39, &ints(&[2, 0, 0x0000FF, 0]))); // CREATEBRUSHINDIRECT 2: solid red
    body.extend(rec(37, &ints(&[2])));
    body.extend(rec(37, &[0x08, 0, 0, 0x80])); // NULL_PEN
    body.extend(rec(43, &ints(&[60, 30, 80, 40]))); // RECTANGLE
    // EXTCREATEFONTINDIRECTW 3: height -10, face "PMingLiU".
    let mut lf = ints(&[3, -10, 0, 0, 0, 400]);
    lf.extend_from_slice(&[0, 0, 0, 136, 0, 0, 0, 0]);
    let face: Vec<u16> = "PMingLiU".encode_utf16().collect();
    let mut name = [0u8; 64];
    for (k, u) in face.iter().enumerate() {
        name[k * 2..k * 2 + 2].copy_from_slice(&u.to_le_bytes());
    }
    lf.extend_from_slice(&name);
    lf.resize(4 + 92 + 128 + 16, 0);
    body.extend(rec(82, &lf));
    body.extend(rec(37, &ints(&[3])));
    // EXTTEXTOUTW at (20, 20), two characters with advances 10, 10.
    let text: Vec<u8> = "負載".encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    let mut t = ints(&[0, 0, 0, 0, 1]); // bounds, graphics mode
    t.extend_from_slice(&1f32.to_le_bytes());
    t.extend_from_slice(&1f32.to_le_bytes());
    let off_string = 8 + t.len() as i32 + 40;
    t.extend(ints(&[20, 20, 2, off_string, 0, 0, 0, -1, -1, off_string + 4]));
    t.extend_from_slice(&text);
    t.extend(ints(&[10, 10]));
    body.extend(rec(84, &t));
    body.extend(rec(14, &ints(&[0, 0, 0])));
    // The header: bounds, frame (0.01 mm), signature, version, size, records, handles, ...,
    // device 1000 × 1000 pixels over 100 × 100 mm.
    let mut h = ints(&[0, 0, 99, 49, 0, 0, 999, 499]);
    h.extend_from_slice(b" EMF");
    h.extend(ints(&[0x10000, 0, 0]));
    h.extend(ints(&[4, 0, 0, 0, 1000, 1000, 100, 100]));
    let mut out = rec(1, &h);
    out.extend(body);
    let size = out.len() as u32;
    out[48..52].copy_from_slice(&size.to_le_bytes());
    out
}

/// `emf` in a Windows metafile's comment records, `chunk` bytes per record.
fn wmf_with(emf: &[u8], chunk: usize) -> Vec<u8> {
    let mut w = Vec::new();
    w.extend_from_slice(&[1, 0, 9, 0, 0, 3]);
    w.extend_from_slice(&[0; 12]);
    for c in emf.chunks(chunk) {
        let mut p = Vec::new();
        p.extend_from_slice(&0x000Fu16.to_le_bytes());
        p.extend_from_slice(&((34 + c.len()) as u16).to_le_bytes());
        p.extend_from_slice(b"WMFC");
        le32(&mut p, 1);
        le32(&mut p, 0x10000);
        p.extend_from_slice(&[0, 0]);
        le32(&mut p, 0);
        le32(&mut p, 1);
        le32(&mut p, c.len() as u32);
        le32(&mut p, 0);
        le32(&mut p, emf.len() as u32);
        p.extend_from_slice(c);
        if p.len() % 2 != 0 {
            p.push(0);
        }
        le32(&mut w, (3 + p.len() / 2) as u32);
        w.extend_from_slice(&0x0626u16.to_le_bytes());
        w.extend(p);
    }
    w.extend_from_slice(&[3, 0, 0, 0, 0, 0]);
    w
}

/// A presentation stream (CF_ENHMETAFILE) holding `wmf`.
fn olepres(wmf: &[u8]) -> Vec<u8> {
    let mut v = Vec::new();
    for x in [0xFFFF_FFFF, 14, 4, 1, 0xFFFF_FFFF, 2, 0, 1000, 500, wmf.len() as u32] {
        le32(&mut v, x);
    }
    v.extend_from_slice(wmf);
    v
}

/// A compound file of 512-byte sectors with one stream, every stream in regular sectors.
fn compound(name: &str, data: &[u8]) -> Vec<u8> {
    let n = data.len().div_ceil(512).max(1);
    let mut h = vec![0u8; 512];
    h[..8].copy_from_slice(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]);
    h[24..26].copy_from_slice(&0x3Eu16.to_le_bytes());
    h[26..28].copy_from_slice(&3u16.to_le_bytes());
    h[28..30].copy_from_slice(&0xFFFEu16.to_le_bytes());
    h[30..32].copy_from_slice(&9u16.to_le_bytes());
    h[32..34].copy_from_slice(&6u16.to_le_bytes());
    h[44..48].copy_from_slice(&1u32.to_le_bytes());
    h[48..52].copy_from_slice(&1u32.to_le_bytes());
    h[60..64].copy_from_slice(&0xFFFF_FFFEu32.to_le_bytes());
    h[68..72].copy_from_slice(&0xFFFF_FFFEu32.to_le_bytes());
    for k in 0..109 {
        h[76 + k * 4..80 + k * 4].copy_from_slice(&if k == 0 { 0u32 } else { 0xFFFF_FFFF }.to_le_bytes());
    }
    let mut fat = vec![0xFFFF_FFFDu32, 0xFFFF_FFFE];
    for k in 0..n {
        fat.push(if k + 1 == n { 0xFFFF_FFFE } else { (k + 3) as u32 });
    }
    fat.resize(128, 0xFFFF_FFFF);
    let mut out = h;
    out.extend(fat.iter().flat_map(|x| x.to_le_bytes()));
    let entry = |name: &str, kind: u8, start: u32, size: u32| {
        let mut e = vec![0u8; 128];
        let units: Vec<u16> = name.encode_utf16().collect();
        for (k, u) in units.iter().enumerate() {
            e[k * 2..k * 2 + 2].copy_from_slice(&u.to_le_bytes());
        }
        e[64..66].copy_from_slice(&(((units.len() + 1) * 2) as u16).to_le_bytes());
        e[66] = kind;
        e[116..120].copy_from_slice(&start.to_le_bytes());
        e[120..124].copy_from_slice(&size.to_le_bytes());
        e
    };
    out.extend(entry("Root Entry", 5, 0xFFFF_FFFE, 0));
    out.extend(entry(name, 2, 2, data.len() as u32));
    out.resize(out.len() + 256, 0);
    out.extend_from_slice(data);
    out.resize(512 * (3 + n), 0);
    out
}

/// OLE data: a 128-byte header, then the compound file.
fn ole_data(cfb: &[u8]) -> Vec<u8> {
    let mut v = vec![0u8; 128];
    v.extend_from_slice(cfb);
    v
}

fn frame_tags(data: &[u8]) -> Vec<RawTag> {
    let tag = |code: i32, value: &str| RawTag { code, value: value.into() };
    let mut tags = vec![tag(10, "0"), tag(20, "50"), tag(11, "100"), tag(21, "0"), tag(90, &data.len().to_string())];
    for c in data.chunks(127) {
        tags.push(tag(310, &c.iter().map(|b| format!("{b:02X}")).collect::<String>()));
    }
    tags
}

fn drawing(tags: Vec<RawTag>) -> Drawing {
    let mut d = Drawing::new_imperial();
    d.add(&Space::Model, Common::default(), EntityKind::Unknown(Unknown { dxf_type: "OLE2FRAME".into(), tags })).ok();
    d
}

fn sample_tags() -> Vec<RawTag> {
    frame_tags(&ole_data(&compound("\u{2}OlePres000", &olepres(&wmf_with(&sample_emf(), 100)))))
}

#[test]
fn the_metafile_is_found_through_its_wrappers() {
    let emf = sample_emf();
    let data = payload(&sample_tags()).unwrap_or_default();
    assert_eq!(emf_of(&data), Some(emf));
}

#[test]
fn the_picture_plays_lines_fills_and_text() {
    let pic = (*picture(&sample_tags())).clone().unwrap_or_default();
    assert_eq!(pic.rect, [0.0, 0.0, 100.0, 50.0]);
    assert!(pic.items.contains(&emf::Item::Line { pts: vec![Vec2::new(10.0, 10.0), Vec2::new(90.0, 10.0)], rgb: emf::Rgb(0, 0, 0) }));
    assert!(pic.items.iter().any(|i| matches!(i, emf::Item::Fill { rgb: emf::Rgb(255, 0, 0), .. })));
    let text = pic.items.iter().find_map(|i| if let emf::Item::Text(t) = i { Some(t.clone()) } else { None });
    let Some(t) = text else { panic!("no text in {:?}", pic.items) };
    assert_eq!(t.text, "負載");
    assert_eq!(t.advances, vec![10.0, 10.0]);
    assert_eq!(t.em, 10.0);
    // The top is at y 20, the baseline 0.8 em below.
    assert_eq!(t.at, Vec2::new(20.0, 28.0));
}

/// The picture is fitted to the frame: device (10, 10) is 10 right of and 10 below the upper-left
/// corner (0, 50) of a 100 × 50 frame over a 100 × 50 pixel picture; the text is drawn in it.
#[test]
fn an_ole_frame_draws_its_picture_in_its_frame() {
    let l = build(&drawing(sample_tags()), &Space::Model, &Options { tolerance: 0.01, ..Default::default() });
    assert!(l.verts.contains(&Vec2::new(10.0, 40.0)) && l.verts.contains(&Vec2::new(90.0, 40.0)), "the line, mapped into the frame");
    assert!(l.prims.len() > 3, "frame, line, fill and glyphs: {}", l.prims.len());
    let glyphs = l.tris.len() + l.verts.len();
    assert!(glyphs > 20);
    let inside = |p: &Vec2| p.x >= -0.5 && p.x <= 100.5 && p.y >= -0.5 && p.y <= 50.5;
    assert!(l.verts.iter().chain(&l.tris).all(inside), "everything inside the frame");
    let red = l.prims.iter().filter(|p| p.color == Rgb(255, 0, 0)).count();
    assert_eq!(red, 1, "the red rectangle");
}

/// AutoCAD stores the compound file without the unused end of its last sector; the stream's tail
/// in that short sector is still read.
#[test]
fn a_short_last_sector_is_read_as_far_as_it_goes() {
    let emf = sample_emf();
    let pres = olepres(&wmf_with(&emf, 100));
    let mut cfb = compound("\u{2}OlePres000", &pres);
    let used = 512 * 3 + pres.len();
    cfb.truncate(used);
    let streams = cfb::streams(&cfb, |_| true).unwrap_or_default();
    assert_eq!(streams.first().map(|s| s.data.len()), Some(pres.len()));
    assert_eq!(emf_of(&ole_data(&cfb)), Some(emf));
}

/// Data that is not a compound file, a presentation without a metafile, and every truncation of
/// good data draw the frame and do not panic.
#[test]
fn damaged_ole_data_draws_the_frame_alone() {
    let frame_only = |tags: Vec<RawTag>| build(&drawing(tags), &Space::Model, &Options::default()).prims.len();
    assert_eq!(frame_only(frame_tags(&[0xAB; 600])), 1);
    assert_eq!(frame_only(frame_tags(&ole_data(&compound("\u{2}OlePres000", &[0xFF; 40])))), 1);
    let mut bad = sample_tags();
    bad.push(RawTag { code: 310, value: "XYZ".into() });
    assert_eq!(frame_only(bad), 1, "data that is not hex");
    let data = ole_data(&compound("\u{2}OlePres000", &olepres(&wmf_with(&sample_emf(), 100))));
    for n in (0..data.len()).step_by(7) {
        let _ = picture(&frame_tags(&data[..n]));
    }
    // Every byte of the metafile flipped in turn.
    let emf = sample_emf();
    for k in 0..emf.len() {
        let mut e = emf.clone();
        e[k] ^= 0xFF;
        let _ = emf::play(&e);
    }
}

#[test]
fn ansi_text_is_read_in_its_font_code_page() {
    assert_eq!(decode_ansi(&[0xAD, 0x74, 0xB8, 0xFC], 136), "負載");
    assert_eq!(decode_ansi(b"Load", 0), "Load");
}

/// What the CLI's note on undrawn OLE content asks: a playable picture, or not.
#[test]
fn an_ole_picture_is_reported_as_drawn_only_when_it_plays() {
    assert!(ole_picture_drawn(&sample_tags()));
    assert!(!ole_picture_drawn(&frame_tags(&[0xAB; 600])));
    assert!(!ole_picture_drawn(&[]));
}

/// A compound file whose directory names one presentation stream thousands of times, every entry
/// on the same sector chain, decodes no more bytes than the file holds (each entry used to be read
/// in full: hundreds of GiB from a 64 MiB file).
#[test]
fn directory_entries_sharing_a_chain_do_not_multiply_the_bytes_read() {
    let data = vec![0x5Au8; 8 * 1024];
    let mut cfb = compound("\u{2}OlePres000", &data);
    // Rewrite the directory chain to run on through more directory sectors, each full of copies
    // of the stream's entry: 4 entries a sector, as many sectors as the FAT sector maps.
    let entry: Vec<u8> = cfb[1024 + 128..1024 + 256].to_vec();
    let first_extra = (cfb.len() / 512 - 1) as u32;
    let extra = 500u32;
    let mut fat: Vec<u32> = cfb[512..1024].chunks(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
    fat[1] = first_extra;
    for k in 0..extra {
        let s = (first_extra + k) as usize;
        if s >= fat.len() {
            break;
        }
        fat[s] = if k + 1 == extra || s + 1 >= fat.len() { 0xFFFF_FFFE } else { first_extra + k + 1 };
        let mut sector = Vec::with_capacity(512);
        for _ in 0..4 {
            sector.extend_from_slice(&entry);
        }
        cfb.extend(sector);
    }
    for (k, v) in fat.iter().enumerate() {
        cfb[512 + k * 4..516 + k * 4].copy_from_slice(&v.to_le_bytes());
    }
    let read: usize = cfb::streams(&cfb, |_| true).unwrap_or_default().iter().map(|s| s.data.len()).sum();
    assert!(read <= cfb.len(), "{read} bytes decoded from a {}-byte file", cfb.len());
}

/// RESTOREDC with a relative level far past the saved states (i32::MIN) is refused at once, and
/// one past the states restores nothing; a valid one restores the state saved there.
#[test]
fn restoredc_out_of_range_is_ignored() {
    let start = std::time::Instant::now();
    let mut e = sample_emf();
    // Before EOF: SAVEDC, RESTOREDC i32::MIN, RESTOREDC -2, RESTOREDC 5.
    let eof = e.len() - 20;
    let mut extra = rec(33, &[]);
    extra.extend(rec(34, &i32::MIN.to_le_bytes()));
    extra.extend(rec(34, &(-2i32).to_le_bytes()));
    extra.extend(rec(34, &5i32.to_le_bytes()));
    extra.extend(rec(34, &(-1i32).to_le_bytes()));
    e.splice(eof..eof, extra);
    assert!(emf::play(&e).is_some());
    assert!(start.elapsed() < std::time::Duration::from_secs(1), "{:?}", start.elapsed());
}

/// A metafile of the sample header with `body` as its records.
fn from_records(body: Vec<u8>) -> Vec<u8> {
    let mut e = sample_emf()[..88].to_vec();
    e.extend(body);
    e.extend(rec(14, &ints(&[0, 0, 0])));
    let n = e.len() as u32;
    e[48..52].copy_from_slice(&n.to_le_bytes());
    e
}

fn build_emf(e: &[u8]) -> crate::DisplayList {
    build(&drawing(frame_tags(&ole_data(&compound("\u{2}OlePres000", &olepres(&wmf_with(e, 100)))))), &Space::Model, &Options::default())
}

/// The sample's records with `edit` applied to each (record type, record bytes).
fn sample_with(prefix: Vec<u8>, edit: impl Fn(u32, &mut Vec<u8>)) -> Vec<u8> {
    let e = sample_emf();
    let mut pos = 88;
    let mut body = prefix;
    while pos < e.len() {
        let typ = u32::from_le_bytes(e[pos..pos + 4].try_into().unwrap());
        let len = u32::from_le_bytes(e[pos + 4..pos + 8].try_into().unwrap()) as usize;
        if typ == 14 {
            break;
        }
        let mut r = e[pos..pos + len].to_vec();
        edit(typ, &mut r);
        body.extend(r);
        pos += len;
    }
    from_records(body)
}

/// A POLYPOLYGON's rings are one region: with the alternate fill mode the inner ring is a hole.
#[test]
fn a_polypolygon_inner_ring_is_a_hole() {
    let mut b = rec(39, &ints(&[1, 0, 0x0000FF, 0]));
    b.extend(rec(37, &ints(&[1])));
    b.extend(rec(37, &0x8000_0008u32.to_le_bytes()));
    b.extend(rec(8, &ints(&[0, 0, 100, 50, 2, 8, 4, 4, 10, 10, 90, 10, 90, 40, 10, 40, 30, 20, 70, 20, 70, 30, 30, 30])));
    let l = build_emf(&from_records(b));
    let covers = |p: Vec2| {
        let cross = |a: Vec2, b: Vec2| (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x);
        l.tris.as_chunks::<3>().0.iter().any(|[a, b, c]| {
            let v = [cross(*a, *b), cross(*b, *c), cross(*c, *a)];
            v.iter().all(|x| *x >= 0.0) || v.iter().all(|x| *x <= 0.0)
        })
    };
    // Device (50, 25) is the hole's centre, (20, 15) inside the outer ring only; the frame maps
    // device y down to drawing y up (50 - y).
    assert!(!covers(Vec2::new(50.0, 25.0)), "the hole is filled");
    assert!(covers(Vec2::new(20.0, 35.0)), "the ring around it is not");
}

/// A transform that overflows leaves no coordinate that is not finite in the picture or the
/// display list.
#[test]
fn an_overflowing_transform_draws_nothing_that_is_not_finite() {
    let mut body = Vec::new();
    let mut xf: Vec<u8> = [1e38f32, 0., 0., 1e38, 0., 0.].into_iter().flat_map(|v| v.to_le_bytes()).collect();
    xf.extend(2u32.to_le_bytes());
    for _ in 0..10 {
        body.extend(rec(36, &xf));
    }
    body.extend(rec(43, &ints(&[1, 1, 10, 10])));
    let e = from_records(body);
    let finite = |p: &Vec2| p.x.is_finite() && p.y.is_finite();
    let p = emf::play(&e).unwrap_or_default();
    assert!(p.items.iter().all(|i| match i {
        emf::Item::Line { pts, .. } => pts.iter().all(finite),
        emf::Item::Text(t) => finite(&t.at),
        _ => true,
    }));
    let l = build_emf(&e);
    assert!(l.verts.iter().chain(&l.tris).all(finite));
}

/// With ETO_PDY the advances are (x, y) pairs: each character advances by the x of its pair.
#[test]
fn eto_pdy_advances_take_the_x_of_each_pair() {
    let e = sample_with(Vec::new(), |t, r| {
        if t == 84 {
            r[52..56].copy_from_slice(&0x2000u32.to_le_bytes());
            let off = u32::from_le_bytes(r[72..76].try_into().unwrap()) as usize;
            r.truncate(off);
            r.extend(ints(&[10, 0, 20, 0]));
            let n = r.len() as u32;
            r[4..8].copy_from_slice(&n.to_le_bytes());
        }
    });
    let p = emf::play(&e).unwrap_or_default();
    let t = p.items.iter().find_map(|i| if let emf::Item::Text(t) = i { Some(t.clone()) } else { None });
    assert_eq!(t.map(|t| t.advances), Some(vec![10.0, 20.0]));
}

/// A font's escapement turns its text from the device's x axis: 90° reads upward whether the
/// mapping's y axis points down (MM_TEXT) or up (MM_LOMETRIC).
#[test]
fn escapement_turns_text_the_same_way_in_every_mapping() {
    let dir = |mode: Option<u32>| {
        let prefix = mode.map(|m| rec(17, &m.to_le_bytes())).unwrap_or_default();
        let e = sample_with(prefix, |t, r| {
            if t == 82 {
                r[20..24].copy_from_slice(&900i32.to_le_bytes());
            }
        });
        emf::play(&e).unwrap_or_default().items.iter().find_map(|i| if let emf::Item::Text(t) = i { Some(t.dir) } else { None })
    };
    // Device y grows downward: upward is negative y.
    assert!(dir(None).is_some_and(|d| d.y < -0.9));
    assert!(dir(Some(2)).is_some_and(|d| d.y < -0.9));
}

/// White text on a dark fill (a spreadsheet's heading) is drawn; a white fill (the background) is
/// not.
#[test]
fn white_text_is_drawn_and_a_white_fill_is_not() {
    let e = sample_with(Vec::new(), |t, r| {
        if t == 39 {
            // The rectangle's brush: white.
            r[16..20].copy_from_slice(&0x00FF_FFFFu32.to_le_bytes());
        }
    });
    let mut body = e[88..e.len() - 20].to_vec();
    body.extend(rec(24, &0x00FF_FFFFu32.to_le_bytes()));
    let text: Vec<u8> = "AB".encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    let mut t = ints(&[0, 0, 0, 0, 1]);
    t.extend_from_slice(&1f32.to_le_bytes());
    t.extend_from_slice(&1f32.to_le_bytes());
    let off = 8 + t.len() as i32 + 40;
    t.extend(ints(&[40, 30, 2, off, 0, 0, 0, -1, -1, 0]));
    t.extend_from_slice(&text);
    body.extend(rec(84, &t));
    let l = build_emf(&from_records(body));
    // True white, not colour 7 (which the frame and the black items draw with).
    let white = |p: &&crate::display::DPrim| p.color == Rgb(255, 255, 255) && !p.aci7;
    // The text: glyph outlines or strokes, whichever font the test machine resolves.
    assert!(l.prims.iter().filter(white).count() > 0, "the white text");
    let fill = |p: &&crate::display::DPrim| white(p) && p.kind == crate::display::Kind::Tris && p.len == 6;
    assert!(!l.prims.iter().any(|p| fill(&p)), "the white rectangle");
}

/// A metafile with more points than one picture keeps stops keeping them: the picture holds at most
/// its budget.
#[test]
fn a_picture_keeps_a_bounded_number_of_points() {
    let mut body = Vec::new();
    let n = 600_000u32;
    for _ in 0..5 {
        let mut r = ints(&[0, 0, 100, 50, n as i32]);
        for i in 0..n {
            r.extend(((i % 100) as i16).to_le_bytes());
            r.extend(((i % 50) as i16).to_le_bytes());
        }
        body.extend(rec(87, &r));
    }
    let p = emf::play(&from_records(body)).unwrap_or_default();
    let kept: usize = p.items.iter().map(|i| if let emf::Item::Line { pts, .. } = i { pts.len() } else { 0 }).sum();
    assert!(kept < 5 * n as usize, "{kept} points kept");
    assert!(kept > 0);
}

/// The same OLE data drawn twice (copies of an object, a block inserted many times) is decoded
/// once: the second lookup returns the picture already decoded.
#[test]
fn the_same_ole_data_is_decoded_once() {
    let tags = sample_tags();
    assert!(std::sync::Arc::ptr_eq(&picture(&tags), &picture(&tags)));
}
