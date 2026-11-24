//! Play an Enhanced Metafile ([MS-EMF]) into vector items in device units: lines, filled
//! polygons and text. Only drawing records an OLE presentation uses are played (lines, polylines,
//! polygons, rectangles, pattern fills, text and the state they depend on); others are skipped.
//! Bitmaps are not drawn.

use cadcraft_geom::Vec2;

/// A colour of the metafile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Rgb(pub u8, pub u8, pub u8);

/// A piece of the picture, in device units (y down).
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Item {
    Line {
        pts: Vec<Vec2>,
        rgb: Rgb,
    },
    /// One region: every ring of a record filled together, so a ring inside another is a hole
    /// (alternate fill) or not (`winding`).
    Fill {
        rings: Vec<Vec<Vec2>>,
        rgb: Rgb,
        winding: bool,
    },
    Text(Text),
}

/// A string drawn at its baseline start.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Text {
    pub text: String,
    /// The baseline start, device units.
    pub at: Vec2,
    /// Per-character advances (device units) when the record gives them, else empty.
    pub advances: Vec<f64>,
    /// The em height, device units.
    pub em: f64,
    /// The width over the height of the device scale (non-uniform mapping).
    pub width_factor: f64,
    /// The text's direction, a unit vector in device units (y down).
    pub dir: Vec2,
    pub face: String,
    pub rgb: Rgb,
    /// 0 left, 1 centre, 2 right of `at`.
    pub halign: u8,
}

/// A played metafile: its picture rectangle in device units and what it draws.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Picture {
    pub rect: [f64; 4],
    pub items: Vec<Item>,
}

/// The most records played and items kept from one metafile.
const MAX_RECORDS: usize = 1_000_000;
const MAX_ITEMS: usize = 200_000;
/// The deepest SAVEDC nesting kept.
const MAX_SAVED: usize = 256;
/// The most object handles a metafile may declare that are kept.
const MAX_OBJECTS: usize = 65_536;
/// The most characters drawn from one text record.
const MAX_CHARS: usize = 4096;
/// The most points of one poly record.
const MAX_POINTS: usize = 1_000_000;
/// The most points kept from one metafile, all items together: a picture of a spreadsheet holds a
/// few thousand. Past it the remaining drawing records are skipped, which bounds the memory a
/// hostile metafile can make one picture take (its points are stored as two f64 each).
const MAX_TOTAL_POINTS: usize = 2_000_000;
/// ETO_PDY: the advances come in pairs (x, y).
const ETO_PDY: u32 = 0x2000;

fn u32_at(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(o..o.checked_add(4)?)?.try_into().ok()?))
}
fn i32_at(d: &[u8], o: usize) -> Option<i32> {
    Some(i32::from_le_bytes(d.get(o..o.checked_add(4)?)?.try_into().ok()?))
}
fn i16_at(d: &[u8], o: usize) -> Option<i16> {
    Some(i16::from_le_bytes(d.get(o..o.checked_add(2)?)?.try_into().ok()?))
}
fn f32_at(d: &[u8], o: usize) -> Option<f64> {
    let v = f64::from(f32::from_le_bytes(d.get(o..o.checked_add(4)?)?.try_into().ok()?));
    v.is_finite().then_some(v)
}
fn colorref(v: u32) -> Rgb {
    Rgb((v & 0xFF) as u8, ((v >> 8) & 0xFF) as u8, ((v >> 16) & 0xFF) as u8)
}

#[derive(Clone, Debug)]
enum Object {
    Pen { rgb: Rgb, null: bool },
    Brush { rgb: Rgb, null: bool },
    Font(Font),
}

#[derive(Clone, Debug, PartialEq)]
struct Font {
    height: i32,
    escapement: i32,
    charset: u8,
    face: String,
}

impl Default for Font {
    fn default() -> Self {
        Font { height: 12, escapement: 0, charset: 0, face: String::new() }
    }
}

/// An affine transform `x' = m11 x + m21 y + dx`, `y' = m12 x + m22 y + dy`.
#[derive(Clone, Copy, Debug)]
struct Xform([f64; 6]);

impl Xform {
    const ID: Xform = Xform([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    fn apply(&self, p: Vec2) -> Vec2 {
        let [a, b, c, d, e, f] = self.0;
        Vec2::new(a * p.x + c * p.y + e, b * p.x + d * p.y + f)
    }
    /// `self` then `o`.
    fn then(&self, o: &Xform) -> Xform {
        let [a, b, c, d, e, f] = self.0;
        let [a2, b2, c2, d2, e2, f2] = o.0;
        Xform([a * a2 + b * c2, a * b2 + b * d2, c * a2 + d * c2, c * b2 + d * d2, e * a2 + f * c2 + e2, e * b2 + f * d2 + f2])
    }
    fn read(d: &[u8], o: usize) -> Option<Xform> {
        Some(Xform([f32_at(d, o)?, f32_at(d, o + 4)?, f32_at(d, o + 8)?, f32_at(d, o + 12)?, f32_at(d, o + 16)?, f32_at(d, o + 20)?]))
    }
}

#[derive(Clone, Debug)]
struct State {
    map_mode: u32,
    win_org: Vec2,
    win_ext: Vec2,
    vp_org: Vec2,
    vp_ext: Vec2,
    world: Xform,
    text_rgb: Rgb,
    text_align: u32,
    pen: Option<Rgb>,
    brush: Option<Rgb>,
    font: Font,
    cur: Vec2,
    /// SETPOLYFILLMODE WINDING (else ALTERNATE).
    winding: bool,
}

impl State {
    fn new() -> Self {
        State {
            map_mode: 1,
            win_org: Vec2::new(0.0, 0.0),
            win_ext: Vec2::new(1.0, 1.0),
            vp_org: Vec2::new(0.0, 0.0),
            vp_ext: Vec2::new(1.0, 1.0),
            world: Xform::ID,
            text_rgb: Rgb(0, 0, 0),
            text_align: 0,
            pen: Some(Rgb(0, 0, 0)),
            brush: Some(Rgb(255, 255, 255)),
            font: Font::default(),
            cur: Vec2::new(0.0, 0.0),
            winding: false,
        }
    }
}

struct Player {
    st: State,
    saved: Vec<State>,
    objects: Vec<Option<Object>>,
    /// Device pixels per millimetre, x and y (from the header).
    px_per_mm: Vec2,
    items: Vec<Item>,
    /// Points that may still be kept ([`MAX_TOTAL_POINTS`]).
    points_left: usize,
}

impl Player {
    /// Page units to device units, per axis, for the fixed map modes.
    fn page_scale(&self) -> Vec2 {
        let mm = |k: f64| Vec2::new(self.px_per_mm.x * k, -self.px_per_mm.y * k);
        match self.st.map_mode {
            2 => mm(0.1),
            3 => mm(0.01),
            4 => mm(0.254),
            5 => mm(0.0254),
            6 => mm(25.4 / 1440.0),
            7 | 8 => {
                let (we, ve) = (self.st.win_ext, self.st.vp_ext);
                if we.x == 0.0 || we.y == 0.0 {
                    return Vec2::new(1.0, 1.0);
                }
                let s = Vec2::new(ve.x / we.x, ve.y / we.y);
                if self.st.map_mode == 7 {
                    // Isotropic: the smaller scale on both axes, keeping each sign.
                    let k = s.x.abs().min(s.y.abs());
                    Vec2::new(k.copysign(s.x), k.copysign(s.y))
                } else {
                    s
                }
            }
            _ => Vec2::new(1.0, 1.0),
        }
    }
    /// The whole logical → device transform.
    fn xform(&self) -> Xform {
        let s = self.page_scale();
        let (wo, vo) = (self.st.win_org, self.st.vp_org);
        let page = Xform([s.x, 0.0, 0.0, s.y, vo.x - wo.x * s.x, vo.y - wo.y * s.y]);
        self.st.world.then(&page)
    }
    /// Keep `it` unless it would pass the item or point budget or holds a coordinate that is not
    /// finite (an overflowing transform).
    fn push(&mut self, it: Item) {
        let finite = |p: &Vec2| p.x.is_finite() && p.y.is_finite();
        let (points, ok) = match &it {
            Item::Line { pts, .. } => (pts.len(), pts.iter().all(finite)),
            Item::Fill { rings, .. } => (rings.iter().map(Vec::len).sum(), rings.iter().flatten().all(finite)),
            Item::Text(t) => (t.advances.len() + 1, finite(&t.at) && finite(&t.dir) && t.em.is_finite() && t.width_factor.is_finite()),
        };
        if ok && self.items.len() < MAX_ITEMS && points <= self.points_left {
            self.points_left -= points;
            self.items.push(it);
        }
    }
    fn line(&mut self, pts: Vec<Vec2>) {
        if let Some(rgb) = self.st.pen
            && pts.len() >= 2
        {
            let xf = self.xform();
            let pts = pts.into_iter().map(|p| xf.apply(p)).collect();
            self.push(Item::Line { pts, rgb });
        }
    }
    /// A closed shape: filled with the brush, outlined with the pen.
    fn shape(&mut self, pts: Vec<Vec2>) {
        self.shapes(vec![pts]);
    }
    /// Closed shapes filled as one region with the brush (by the polygon fill mode), each outlined
    /// with the pen.
    fn shapes(&mut self, rings: Vec<Vec<Vec2>>) {
        let xf = self.xform();
        let dev: Vec<Vec<Vec2>> = rings.into_iter().filter(|r| r.len() >= 3).map(|r| r.into_iter().map(|p| xf.apply(p)).collect()).collect();
        if dev.is_empty() {
            return;
        }
        let outlines: Vec<Vec<Vec2>> = if self.st.pen.is_some() {
            dev.iter()
                .map(|r| {
                    let mut l = r.clone();
                    if let Some(f) = l.first().copied() {
                        l.push(f);
                    }
                    l
                })
                .collect()
        } else {
            Vec::new()
        };
        if let Some(rgb) = self.st.brush {
            self.push(Item::Fill { rings: dev, rgb, winding: self.st.winding });
        }
        if let Some(rgb) = self.st.pen {
            for l in outlines {
                self.push(Item::Line { pts: l, rgb });
            }
        }
    }
    fn rect_pts(l: f64, t: f64, r: f64, b: f64) -> Vec<Vec2> {
        vec![Vec2::new(l, t), Vec2::new(r, t), Vec2::new(r, b), Vec2::new(l, b)]
    }
    fn select(&mut self, ih: u32) {
        if ih & 0x8000_0000 != 0 {
            // Stock objects.
            match ih & 0x7FFF_FFFF {
                0 => self.st.brush = Some(Rgb(255, 255, 255)),
                1 => self.st.brush = Some(Rgb(192, 192, 192)),
                2 => self.st.brush = Some(Rgb(128, 128, 128)),
                3 => self.st.brush = Some(Rgb(64, 64, 64)),
                4 => self.st.brush = Some(Rgb(0, 0, 0)),
                5 => self.st.brush = None,
                6 => self.st.pen = Some(Rgb(255, 255, 255)),
                7 => self.st.pen = Some(Rgb(0, 0, 0)),
                8 => self.st.pen = None,
                _ => {}
            }
            return;
        }
        match self.objects.get(ih as usize).cloned().flatten() {
            Some(Object::Pen { rgb, null }) => self.st.pen = (!null).then_some(rgb),
            Some(Object::Brush { rgb, null }) => self.st.brush = (!null).then_some(rgb),
            Some(Object::Font(f)) => self.st.font = f,
            None => {}
        }
    }
    fn store(&mut self, ih: u32, o: Object) {
        if let Some(slot) = self.objects.get_mut(ih as usize) {
            *slot = Some(o);
        }
    }
    /// Points of a 16-bit (`small`) or 32-bit poly record whose count is at `o`.
    fn points(r: &[u8], o: usize, small: bool) -> Vec<Vec2> {
        let n = (u32_at(r, o).unwrap_or(0) as usize).min(MAX_POINTS);
        let at = o + 4;
        let mut pts = Vec::with_capacity(n.min(r.len() / 4));
        for k in 0..n {
            let p = if small {
                i16_at(r, at + k * 4).zip(i16_at(r, at + k * 4 + 2)).map(|(x, y)| Vec2::new(f64::from(x), f64::from(y)))
            } else {
                i32_at(r, at + k * 8).zip(i32_at(r, at + k * 8 + 4)).map(|(x, y)| Vec2::new(f64::from(x), f64::from(y)))
            };
            match p {
                Some(p) => pts.push(p),
                None => break,
            }
        }
        pts
    }
    /// The polygons of a POLYPOLY record: `count` polygons, their point counts, then the points.
    fn poly_poly(r: &[u8], small: bool) -> Vec<Vec<Vec2>> {
        let n = (u32_at(r, 24).unwrap_or(0) as usize).min(MAX_POINTS);
        let total = (u32_at(r, 28).unwrap_or(0) as usize).min(MAX_POINTS);
        let counts: Vec<usize> = (0..n).map_while(|k| u32_at(r, 32 + k * 4).map(|c| c as usize)).collect();
        let mut at = 32 + counts.len() * 4;
        let step = if small { 4 } else { 8 };
        let mut out = Vec::new();
        let mut used = 0usize;
        for c in counts {
            if used.saturating_add(c) > total {
                break;
            }
            used += c;
            let mut pts = Vec::with_capacity(c.min(r.len() / step));
            for _ in 0..c {
                let p = if small {
                    i16_at(r, at).zip(i16_at(r, at + 2)).map(|(x, y)| Vec2::new(f64::from(x), f64::from(y)))
                } else {
                    i32_at(r, at).zip(i32_at(r, at + 4)).map(|(x, y)| Vec2::new(f64::from(x), f64::from(y)))
                };
                at += step;
                match p {
                    Some(p) => pts.push(p),
                    None => return out,
                }
            }
            out.push(pts);
        }
        out
    }
    fn text(&mut self, r: &[u8], wide: bool) {
        // EMREXTTEXTOUT: bounds, graphics mode, scales, then EMRTEXT at 36.
        let (Some(rx), Some(ry), Some(n), Some(off)) = (i32_at(r, 36), i32_at(r, 40), u32_at(r, 44), u32_at(r, 48)) else { return };
        let n = (n as usize).min(MAX_CHARS);
        let off = off as usize;
        let text = if wide {
            let units: Vec<u16> = (0..n)
                .map_while(|k| {
                    let o = off.checked_add(k.checked_mul(2)?)?;
                    r.get(o..o.checked_add(2)?).map(|b| u16::from_le_bytes([b[0], b[1]]))
                })
                .collect();
            String::from_utf16_lossy(&units)
        } else {
            let bytes = r.get(off..off.saturating_add(n).min(r.len())).unwrap_or_default();
            super::decode_ansi(bytes, self.st.font.charset)
        };
        if text.trim().is_empty() {
            return;
        }
        let off_dx = u32_at(r, 72).unwrap_or(0) as usize;
        // With ETO_PDY the advances are (x, y) pairs; the x of each is the advance along the text.
        let per: usize = if u32_at(r, 52).unwrap_or(0) & ETO_PDY != 0 { 8 } else { 4 };
        let mut dx: Vec<f64> = if off_dx >= 76 {
            (0..n).map_while(|k| off_dx.checked_add(k.checked_mul(per)?).and_then(|o| i32_at(r, o)).map(f64::from)).collect()
        } else {
            Vec::new()
        };
        let chars = text.chars().count();
        if !wide || dx.len() != n || chars != n {
            // Advances that don't pair one to one with the characters are not used.
            dx.clear();
        }
        let align = self.st.text_align;
        let mut reference = Vec2::new(f64::from(rx), f64::from(ry));
        if align & 1 != 0 {
            reference = self.st.cur;
        }
        let xf = self.xform();
        // The device lengths of a logical unit along x and y.
        let ux = xf.apply(Vec2::new(1.0, 0.0)) - xf.apply(Vec2::new(0.0, 0.0));
        let uy = xf.apply(Vec2::new(0.0, 1.0)) - xf.apply(Vec2::new(0.0, 0.0));
        let (sx, sy) = (ux.len(), uy.len());
        if sx <= 0.0 || sy <= 0.0 || !sx.is_finite() || !sy.is_finite() {
            return;
        }
        let f = self.st.font.clone();
        // A negative height is the em height; a positive one the cell height (taken as the em).
        let em_log = f64::from(f.height.unsigned_abs().max(1));
        let em = em_log * sy;
        // Where the baseline is from the reference: top (TA_TOP), bottom or baseline.
        let ascent = 0.8 * em_log;
        let v = align & 24;
        let down = match v {
            24 => 0.0,
            8 => -(em_log - ascent),
            _ => ascent,
        };
        // The escapement is measured from the device's x axis, counter-clockwise as seen, whatever
        // the mapping (GDI); in device units, whose y grows downward, that is (cos, -sin).
        let angle = f64::from(f.escapement) / 10.0 * std::f64::consts::PI / 180.0;
        let mut dir = Vec2::new(angle.cos(), -angle.sin());
        let mut dn = Vec2::new(-dir.y, dir.x);
        // In the advanced graphics mode (iGraphicsMode 2) the world transform applies to text as
        // to everything else: its linear part turns the direction and the baseline offset. Only
        // their directions are taken; the em height already carries the scale.
        if u32_at(r, 24) == Some(2) {
            let w = self.st.world.0;
            let turn = |v: Vec2| {
                let t = Vec2::new(w[0] * v.x + w[2] * v.y, w[1] * v.x + w[3] * v.y);
                let l = t.len();
                (l > 0.0 && l.is_finite()).then(|| t * (1.0 / l))
            };
            if let (Some(d), Some(n)) = (turn(dir), turn(dn)) {
                (dir, dn) = (d, n);
            }
        }
        let at = xf.apply(reference) + dn * (down * sy);
        let halign = match align & 6 {
            6 => 1,
            2 => 2,
            _ => 0,
        };
        let advances: Vec<f64> = dx.iter().map(|d| d * sx).collect();
        if align & 1 != 0 {
            // The current position moves along the text in logical units (an unrotated mapping).
            let total: f64 = dx.iter().sum();
            self.st.cur = reference + Vec2::new(total, 0.0);
        }
        self.push(Item::Text(Text { text, at, advances, em, width_factor: sx / sy, dir, face: f.face, rgb: self.st.text_rgb, halign }));
    }

    /// Play one record of type `t`; `false` at the end of the metafile.
    fn record(&mut self, t: u32, r: &[u8]) -> bool {
        if t == 14 {
            return false;
        }
        if !self.state_record(t, r) {
            self.draw_record(t, r);
        }
        true
    }

    /// A record that changes the drawing state; `false` when `t` is not one.
    fn state_record(&mut self, t: u32, r: &[u8]) -> bool {
        let pt = |o: usize| i32_at(r, o).zip(i32_at(r, o + 4)).map(|(x, y)| Vec2::new(f64::from(x), f64::from(y)));
        match t {
            9 => self.st.win_ext = pt(8).unwrap_or(self.st.win_ext),
            10 => self.st.win_org = pt(8).unwrap_or(self.st.win_org),
            11 => self.st.vp_ext = pt(8).unwrap_or(self.st.vp_ext),
            12 => self.st.vp_org = pt(8).unwrap_or(self.st.vp_org),
            17 => self.st.map_mode = u32_at(r, 8).unwrap_or(1),
            19 => self.st.winding = u32_at(r, 8) == Some(2),
            22 => self.st.text_align = u32_at(r, 8).unwrap_or(0),
            24 => self.st.text_rgb = colorref(u32_at(r, 8).unwrap_or(0)),
            33 => {
                if self.saved.len() < MAX_SAVED {
                    self.saved.push(self.st.clone());
                }
            }
            34 => {
                // A negative level counts back from the latest saved state, a positive one is
                // absolute (1 = the first saved). Out of range: ignored, as GDI refuses it.
                let k = i64::from(i32_at(r, 8).unwrap_or(-1));
                let len = self.saved.len() as i64;
                let at = if k < 0 { len + k } else { k - 1 };
                if (0..len).contains(&at)
                    && let Ok(at) = usize::try_from(at)
                {
                    self.saved.truncate(at + 1);
                    if let Some(s) = self.saved.pop() {
                        self.st = s;
                    }
                }
            }
            35 => {
                if let Some(x) = Xform::read(r, 8) {
                    self.st.world = x;
                }
            }
            36 => {
                if let (Some(x), Some(mode)) = (Xform::read(r, 8), u32_at(r, 32)) {
                    self.st.world = match mode {
                        1 => Xform::ID,
                        2 => x.then(&self.st.world),
                        3 => self.st.world.then(&x),
                        4 => x,
                        _ => self.st.world,
                    };
                }
            }
            37 => self.select(u32_at(r, 8).unwrap_or(u32::MAX)),
            40 => {
                if let Some(slot) = u32_at(r, 8).and_then(|i| self.objects.get_mut(i as usize)) {
                    *slot = None;
                }
            }
            38 => {
                if let (Some(ih), Some(style), Some(c)) = (u32_at(r, 8), u32_at(r, 12), u32_at(r, 24)) {
                    self.store(ih, Object::Pen { rgb: colorref(c), null: style & 0xF == 5 });
                }
            }
            95 => {
                if let (Some(ih), Some(style), Some(bs), Some(c)) = (u32_at(r, 8), u32_at(r, 28), u32_at(r, 36), u32_at(r, 40)) {
                    self.store(ih, Object::Pen { rgb: colorref(c), null: style & 0xF == 5 || bs == 1 });
                }
            }
            39 => {
                if let (Some(ih), Some(style), Some(c)) = (u32_at(r, 8), u32_at(r, 12), u32_at(r, 16)) {
                    self.store(ih, Object::Brush { rgb: colorref(c), null: style == 1 });
                }
            }
            82 => {
                if let (Some(ih), Some(h), Some(esc)) = (u32_at(r, 8), i32_at(r, 12), i32_at(r, 20)) {
                    let charset = r.get(35).copied().unwrap_or(0);
                    let units: Vec<u16> = (0..32)
                        .map_while(|k| r.get(40 + k * 2..42 + k * 2).map(|b| u16::from_le_bytes([b[0], b[1]])))
                        .take_while(|u| *u != 0)
                        .collect();
                    self.store(ih, Object::Font(Font { height: h, escapement: esc, charset, face: String::from_utf16_lossy(&units) }));
                }
            }
            _ => return false,
        }
        true
    }

    /// A record that draws (or moves the current position).
    fn draw_record(&mut self, t: u32, r: &[u8]) {
        let pt = |o: usize| i32_at(r, o).zip(i32_at(r, o + 4)).map(|(x, y)| Vec2::new(f64::from(x), f64::from(y)));
        match t {
            27 => self.st.cur = pt(8).unwrap_or(self.st.cur),
            54 => {
                if let Some(q) = pt(8) {
                    let a = self.st.cur;
                    self.line(vec![a, q]);
                    self.st.cur = q;
                }
            }
            43 => {
                if let (Some(a), Some(b)) = (pt(8), pt(16)) {
                    self.shape(Self::rect_pts(a.x, a.y, b.x, b.y));
                }
            }
            3 | 86 => self.shape(Self::points(r, 24, t == 86)),
            4 | 87 => self.line(Self::points(r, 24, t == 87)),
            6 | 90 => {
                let mut pts = vec![self.st.cur];
                pts.extend(Self::points(r, 24, t == 90));
                if let Some(l) = pts.last().copied() {
                    self.st.cur = l;
                }
                self.line(pts);
            }
            7 | 91 => {
                for l in Self::poly_poly(r, t == 91) {
                    self.line(l);
                }
            }
            8 | 92 => {
                let rings = Self::poly_poly(r, t == 92);
                self.shapes(rings);
            }
            76 => {
                // BITBLT without a source bitmap: a pattern fill of the destination rectangle.
                let (Some(x), Some(y), Some(w), Some(h), Some(rop), Some(cb_bmi)) =
                    (i32_at(r, 24), i32_at(r, 28), i32_at(r, 32), i32_at(r, 36), u32_at(r, 40), u32_at(r, 88))
                else {
                    return;
                };
                if cb_bmi == 0 && rop == 0x00F0_0021 && w != 0 && h != 0 {
                    let saved_pen = self.st.pen.take();
                    let (x, y) = (f64::from(x), f64::from(y));
                    self.shape(Self::rect_pts(x, y, x + f64::from(w), y + f64::from(h)));
                    self.st.pen = saved_pen;
                } else if cb_bmi == 0 && rop == 0x0000_0042 && w != 0 && h != 0 {
                    // BLACKNESS.
                    let (pen, brush) = (self.st.pen.take(), self.st.brush.replace(Rgb(0, 0, 0)));
                    let (x, y) = (f64::from(x), f64::from(y));
                    self.shape(Self::rect_pts(x, y, x + f64::from(w), y + f64::from(h)));
                    (self.st.pen, self.st.brush) = (pen, brush);
                }
            }
            83 | 84 => self.text(r, t == 84),
            _ => {}
        }
    }
}

/// Play the metafile `emf` (starting with its header record).
pub(crate) fn play(emf: &[u8]) -> Option<Picture> {
    if u32_at(emf, 0)? != 1 || emf.get(40..44)? != b" EMF" {
        return None;
    }
    let bounds = [i32_at(emf, 8)?, i32_at(emf, 12)?, i32_at(emf, 16)?, i32_at(emf, 20)?].map(f64::from);
    let frame = [i32_at(emf, 24)?, i32_at(emf, 28)?, i32_at(emf, 32)?, i32_at(emf, 36)?].map(f64::from);
    let handles = (u32_at(emf, 56).unwrap_or(0) as usize).clamp(1, MAX_OBJECTS);
    let dev = Vec2::new(f64::from(i32_at(emf, 72)?), f64::from(i32_at(emf, 76)?));
    let mm = Vec2::new(f64::from(i32_at(emf, 80)?), f64::from(i32_at(emf, 84)?));
    let px_per_mm = if mm.x > 0.0 && mm.y > 0.0 && dev.x > 0.0 && dev.y > 0.0 { Vec2::new(dev.x / mm.x, dev.y / mm.y) } else { Vec2::new(1.0, 1.0) };
    // The picture rectangle: the frame (0.01 mm, inclusive) in device units, else the bounds.
    let fr = [
        frame[0] * px_per_mm.x / 100.0,
        frame[1] * px_per_mm.y / 100.0,
        (frame[2] + 1.0) * px_per_mm.x / 100.0,
        (frame[3] + 1.0) * px_per_mm.y / 100.0,
    ];
    let rect = if fr[2] > fr[0] && fr[3] > fr[1] && fr.iter().all(|v| v.is_finite()) {
        fr
    } else if bounds[2] >= bounds[0] && bounds[3] >= bounds[1] {
        [bounds[0], bounds[1], bounds[2] + 1.0, bounds[3] + 1.0]
    } else {
        return None;
    };
    let mut p =
        Player { st: State::new(), saved: Vec::new(), objects: vec![None; handles], px_per_mm, items: Vec::new(), points_left: MAX_TOTAL_POINTS };
    let mut pos = 0usize;
    for _ in 0..MAX_RECORDS {
        let (Some(t), Some(size)) = (u32_at(emf, pos), u32_at(emf, pos + 4)) else { break };
        let size = size as usize;
        if size < 8 || !size.is_multiple_of(4) {
            break;
        }
        let Some(r) = emf.get(pos..pos.saturating_add(size)) else { break };
        pos += size;
        if !p.record(t, r) {
            break;
        }
    }
    Some(Picture { rect, items: p.items })
}
