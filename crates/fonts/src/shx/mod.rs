//! Bounded SHX stroke-font reader. Geometry follows Autodesk's public Special Codes
//! Reference. No font assets are embedded; callers supply licensed font bytes.
mod resolver;
mod vm;
use cadcraft_geom::Vec2;
pub use resolver::{find, font_path, set_font_path};
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Shapes,
    Unicode,
    BigFont,
}
#[derive(Clone, Debug)]
pub struct Font {
    pub kind: Kind,
    pub above: f64,
    pub below: f64,
    pub ranges: Vec<(u16, u16)>,
    shapes: BTreeMap<u16, Vec<u8>>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error(pub &'static str);
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for Error {}
type Result<T> = std::result::Result<T, Error>;
struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}
impl<'a> Reader<'a> {
    fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).ok_or(Error("offset overflow"))?;
        let b = self.data.get(self.pos..end).ok_or(Error("truncated SHX"))?;
        self.pos = end;
        Ok(b)
    }
    fn byte(&mut self) -> Result<u8> {
        self.bytes(1)?.first().copied().ok_or(Error("truncated byte"))
    }
    fn word(&mut self) -> Result<u16> {
        let a = self.byte()?;
        let b = self.byte()?;
        Ok(u16::from_le_bytes([a, b]))
    }
    fn dword(&mut self) -> Result<u32> {
        Ok(u32::from(self.word()?) | (u32::from(self.word()?) << 16))
    }
}
fn program(record: &[u8]) -> Result<Vec<u8>> {
    let end = record.iter().position(|b| *b == 0).ok_or(Error("unterminated shape name"))?;
    Ok(record.get(end + 1..).ok_or(Error("missing shape program"))?.to_vec())
}
impl Font {
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() > 32 * 1024 * 1024 {
            return Err(Error("SHX exceeds 32 MiB"));
        }
        let marker = data.iter().take(64).position(|b| *b == 26).ok_or(Error("missing SHX signature"))?;
        let header = data.get(..marker).ok_or(Error("missing header"))?;
        let kind = if header == b"AutoCAD-86 shapes 1.0\r\n" {
            Kind::Shapes
        } else if header == b"AutoCAD-86 unifont 1.0\r\n" {
            Kind::Unicode
        } else if header == b"AutoCAD-86 bigfont 1.0\r\n" {
            Kind::BigFont
        } else {
            return Err(Error("unsupported SHX signature"));
        };
        let mut r = Reader { data, pos: marker + 1 };
        let mut shapes = BTreeMap::new();
        let mut ranges = Vec::new();
        match kind {
            Kind::Shapes => {
                let _first = r.word()?;
                let _last = r.word()?;
                let count = r.word()?;
                let mut index = Vec::new();
                for _ in 0..count {
                    index.push((r.word()?, usize::from(r.word()?)));
                }
                for (code, len) in index {
                    if shapes.insert(code, program(r.bytes(len)?)?).is_some() {
                        return Err(Error("duplicate shape record"));
                    }
                }
            }
            Kind::BigFont => {
                let _entry_size = r.word()?;
                let count = r.word()?;
                let nr = r.word()?;
                for _ in 0..nr {
                    let lo = r.word()?;
                    let hi = r.word()?;
                    if lo > hi {
                        return Err(Error("invalid escape range"));
                    }
                    ranges.push((lo, hi));
                }
                let mut total = 0usize;
                for _ in 0..count {
                    let code = r.word()?;
                    let len = usize::from(r.word()?);
                    let offset = r.dword()? as usize;
                    total = total.checked_add(len).ok_or(Error("record size overflow"))?;
                    if total > 32 * 1024 * 1024 {
                        return Err(Error("expanded SHX exceeds 32 MiB"));
                    }
                    if len == 0 {
                        continue;
                    }
                    let mut rec = Reader { data, pos: offset };
                    if shapes.insert(code, program(rec.bytes(len)?)?).is_some() {
                        return Err(Error("duplicate shape record"));
                    }
                }
            }
            Kind::Unicode => {
                // Unicode fonts have a six-byte definition prefix and sequential records.
                r.bytes(6)?;
                let start = r.pos;
                while r.byte()? != 0 {}
                let mut metadata = program(data.get(start..r.pos).ok_or(Error("invalid definition"))?)?;
                metadata.extend_from_slice(r.bytes(6)?);
                shapes.insert(0, metadata);
                while r.pos < data.len() {
                    let code = r.word()?;
                    let len = usize::from(r.word()?);
                    if shapes.insert(code, program(r.bytes(len)?)?).is_some() {
                        return Err(Error("duplicate shape record"));
                    }
                }
            }
        }
        let info = shapes.get(&0).ok_or(Error("missing font metrics"))?;
        let above = f64::from(*info.first().ok_or(Error("missing cap height"))?);
        let below = f64::from(*info.get(1).ok_or(Error("missing descent"))?);
        if above == 0.0 {
            return Err(Error("zero cap height"));
        }
        Ok(Self { kind, above, below, ranges, shapes })
    }
    pub fn glyph(&self, code: u16) -> Result<crate::Run> {
        vm::render(self, code)
    }
    pub fn contains(&self, code: u16) -> bool {
        code != 0 && self.shapes.contains_key(&code)
    }
}

/// Explicit legacy encoding: SHX escape ranges do not uniquely identify a code page.
#[derive(Clone, Copy, Debug, Default)]
pub enum Encoding {
    #[default]
    Big5,
    Gb,
}
#[derive(Clone, Debug)]
pub struct Pair {
    pub regular: Arc<Font>,
    pub big: Option<Arc<Font>>,
    pub encoding: Encoding,
}
impl Pair {
    fn glyph(&self, c: char) -> Option<(crate::Run, f64)> {
        let encoded = |big: &Font| {
            let mut buf = [0; 4];
            let codec = match self.encoding {
                Encoding::Big5 => encoding_rs::BIG5,
                Encoding::Gb => encoding_rs::GBK,
            };
            let (bytes, _, error) = codec.encode(c.encode_utf8(&mut buf));
            if !error
                && let [hi, lo] = bytes.as_ref()
                && big.ranges.iter().any(|(a, b)| (*a..=*b).contains(&u16::from(*hi)))
            {
                let code = (u16::from(*hi) << 8) | u16::from(*lo);
                return big.glyph(code).ok().map(|g| (g, big.above));
            }
            None
        };
        if !c.is_ascii()
            && let Some(glyph) = self.big.as_deref().and_then(encoded)
        {
            return Some(glyph);
        }
        if self.regular.kind == Kind::BigFont {
            return encoded(&self.regular);
        }
        let legacy_symbol = match c {
            '°' => Some((256, 127)),
            '±' => Some((257, 128)),
            '⌀' => Some((258, 129)),
            _ => None,
        };
        if self.regular.kind != Kind::Unicode
            && let Some((reserved, legacy)) = legacy_symbol
            && let Some(code) = [reserved, legacy].into_iter().find(|code| self.regular.contains(*code))
        {
            return self.regular.glyph(code).ok().map(|g| (g, self.regular.above));
        }
        let code = match c {
            '⌀' if self.regular.kind == Kind::Unicode && self.regular.contains(0x2300) => 0x2300,
            '⌀' if self.regular.kind == Kind::Unicode && self.regular.contains(0x2205) => 0x2205,
            '⌀' if self.regular.kind != Kind::Unicode => 216,
            _ => u16::try_from(u32::from(c)).ok()?,
        };
        if self.regular.kind == Kind::Shapes && code > 255 {
            return None;
        }
        self.regular.glyph(code).ok().map(|g| (g, self.regular.above))
    }
    pub fn shape(&self, s: &str, height: f64, wf: f64, oblique: f64) -> crate::Shaped {
        let wf = if wf.is_finite() && wf > 1e-6 { wf } else { 1.0 };
        // Keep the existing whole-line bidi and contextual shaping path. Splitting
        // missing RTL characters into isolated fallback glyphs breaks their order/joining.
        if crate::bidi::has_rtl(s) {
            resolver::warn("SHX right-to-left line uses contextual fallback");
            return crate::shape_stroke(s, height, wf, oblique);
        }
        let h = if height.is_finite() && height > 0.0 { height } else { 1.0 };
        let shear = if oblique.is_finite() { oblique.tan().clamp(-10.0, 10.0) } else { 0.0 };
        let mut out = crate::Shaped::default();
        let mut spans = Vec::new();
        let mut characters = crate::decode_controls(s).into_iter().peekable();
        while let Some((c, u, o)) = characters.next() {
            if crate::bidi::is_format_control(c) {
                continue;
            }
            let x = out.width;
            // ASCII escape pairs also occur in symbol bigfonts (for example |A).
            // Unicode CJK text is encoded through the explicit legacy code page instead.
            let escaped = self.big.as_ref().and_then(|big| {
                if !c.is_ascii() || !big.ranges.iter().any(|(a, b)| (*a..=*b).contains(&(c as u16))) {
                    return None;
                }
                let &(next, _, _) = characters.peek()?;
                if !next.is_ascii() {
                    return None;
                }
                let code = ((c as u16) << 8) | next as u16;
                let g = match big.glyph(code) {
                    Ok(g) => g,
                    Err(_) => {
                        resolver::warn("SHX escape glyph unavailable; preserving both characters");
                        return None;
                    }
                };
                characters.next();
                Some((g, big.above))
            });
            if let Some((g, cap)) = escaped.or_else(|| self.glyph(c)) {
                let scale = h / cap;
                out.strokes.extend(
                    g.strokes
                        .into_iter()
                        .map(|st| st.into_iter().map(|p| Vec2::new(x + p.x * scale * wf + p.y * scale * shear, p.y * scale)).collect()),
                );
                out.width += g.width * scale * wf;
            } else {
                resolver::warn("SHX glyph unavailable; using fallback");
                // Preserve built-in letter spacing and contextual fallback within a run.
                // Re-encode decoration flags and literal percent signs for shape_stroke.
                let mut text = String::new();
                let mut flags = (false, false);
                let mut append = |ch, under, over| {
                    if under != flags.0 {
                        text.push_str("%%u");
                    }
                    if over != flags.1 {
                        text.push_str("%%o");
                    }
                    flags = (under, over);
                    if ch == '%' {
                        text.push_str("%%%");
                    } else {
                        text.push(ch);
                    }
                };
                append(c, u, o);
                while let Some(&(next, under, over)) = characters.peek() {
                    let escape_lead =
                        self.big.as_ref().is_some_and(|big| next.is_ascii() && big.ranges.iter().any(|(a, b)| (*a..=*b).contains(&(next as u16))));
                    if escape_lead || crate::bidi::is_format_control(next) || self.glyph(next).is_some() {
                        break;
                    }
                    characters.next();
                    append(next, under, over);
                }
                let mut fallback = crate::shape_stroke(&text, h, wf, oblique);
                fallback.map(|p| p + Vec2::new(x, 0.0));
                out.width += fallback.width;
                out.extend(fallback);
                // Its own decorations must not connect the surrounding SHX spans.
                spans.push((x, out.width, false, false));
                continue; // Fallback already contains this run's decorations.
            }
            spans.push((x, out.width, u, o));
        }
        out.strokes.extend(crate::decorations(&spans, h));
        out
    }
}

#[cfg(test)]
mod tests;
