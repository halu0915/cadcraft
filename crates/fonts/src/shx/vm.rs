use super::{Error, Font, Kind, Reader, Result};
use cadcraft_geom::Vec2;
use std::f64::consts::{FRAC_PI_4, TAU};
struct State {
    pos: Vec2,
    scale: f64,
    pen: bool,
    stack: Vec<Vec2>,
    strokes: Vec<Vec<Vec2>>,
    budget: usize,
}
pub(super) fn render(font: &Font, code: u16) -> Result<crate::Run> {
    let mut s = State { pos: Vec2::ZERO, scale: 1.0, pen: true, stack: Vec::new(), strokes: Vec::new(), budget: 65536 };
    s.shape(font, code, 0)?;
    Ok(crate::Run { strokes: s.strokes, width: s.pos.x })
}
impl State {
    fn spend(&mut self) -> Result<()> {
        self.budget = self.budget.checked_sub(1).ok_or(Error("shape operation limit"))?;
        Ok(())
    }
    fn to(&mut self, p: Vec2) -> Result<()> {
        self.spend()?;
        if !p.is_finite() || p.x.abs().max(p.y.abs()) > 1e9 {
            return Err(Error("shape coordinate limit"));
        }
        if self.pen {
            self.strokes.push(vec![self.pos, p]);
        }
        self.pos = p;
        Ok(())
    }
    fn arc(&mut self, r: f64, start: f64, sweep: f64) -> Result<()> {
        if !r.is_finite() || r.abs() > 1e9 {
            return Err(Error("invalid arc radius"));
        }
        let center = self.pos - Vec2::new(start.cos(), start.sin()) * r;
        let n = (sweep.abs() / 0.05).ceil().clamp(1.0, 128.0) as usize;
        for k in 1..=n {
            let a = start + sweep * k as f64 / n as f64;
            self.to(center + Vec2::new(a.cos(), a.sin()) * r)?;
        }
        Ok(())
    }
    fn bulge(&mut self, d: Vec2, b: i8) -> Result<()> {
        if b == i8::MIN {
            return Err(Error("invalid bulge"));
        }
        let end = self.pos + d * self.scale;
        if b == 0 || d.len2() == 0.0 {
            return self.to(end);
        }
        let v = end - self.pos;
        let b = f64::from(b) / 127.0;
        let center = (self.pos + end) * 0.5 + v.perp() * ((1.0 - b * b) / (4.0 * b));
        let radial = self.pos - center;
        self.arc(radial.len(), radial.y.atan2(radial.x), 4.0 * b.atan())?;
        self.pos = end;
        Ok(())
    }
    fn shape(&mut self, font: &Font, code: u16, depth: usize) -> Result<()> {
        if code == 0 {
            return Err(Error("font metadata is not a glyph"));
        }
        if depth >= 32 {
            return Err(Error("subshape recursion limit"));
        }
        let data = font.shapes.get(&code).ok_or(Error("missing subshape"))?;
        let mut r = Reader { data, pos: 0 };
        loop {
            let op = r.byte()?;
            if op == 0 {
                break;
            }
            self.command(font, &mut r, op, depth, true)?;
        }
        Ok(())
    }
    fn command(&mut self, font: &Font, r: &mut Reader<'_>, op: u8, depth: usize, execute: bool) -> Result<()> {
        self.spend()?;
        match op {
            0 => {}
            1 | 2 => {
                if execute {
                    self.pen = op == 1;
                }
            }
            3 | 4 => {
                let n = f64::from(r.byte()?);
                if n == 0.0 {
                    return Err(Error("zero scale divisor"));
                }
                if execute {
                    self.scale = if op == 3 { self.scale / n } else { self.scale * n };
                    if !self.scale.is_finite() || self.scale.abs() > 1e6 {
                        return Err(Error("shape scale limit"));
                    }
                }
            }
            5 => {
                if execute {
                    if self.stack.len() >= 4 {
                        return Err(Error("position stack overflow"));
                    }
                    self.stack.push(self.pos);
                }
            }
            6 => {
                if execute {
                    self.pos = self.stack.pop().ok_or(Error("position stack underflow"))?;
                }
            }
            7 => {
                let code = if font.kind == Kind::Unicode { r.word()? } else { u16::from(r.byte()?) };
                if font.kind == Kind::BigFont && code == 0 {
                    let primitive = r.word()?;
                    let origin = Vec2::new(f64::from(r.byte()? as i8), f64::from(r.byte()? as i8));
                    let width = f64::from(r.byte()?);
                    let height = f64::from(r.byte()?);
                    if execute {
                        let cap_width =
                            font.shapes.get(&0).and_then(|m| m.get(3)).copied().filter(|w| *w != 0).ok_or(Error("missing primitive width"))?;
                        let mut child =
                            State { pos: Vec2::ZERO, scale: 1.0, pen: self.pen, stack: Vec::new(), strokes: Vec::new(), budget: self.budget };
                        child.shape(font, primitive, depth + 1)?;
                        self.budget = child.budget;
                        let offset = self.pos + origin * self.scale;
                        for stroke in child.strokes {
                            let points: Vec<_> = stroke
                                .into_iter()
                                .map(|p| offset + Vec2::new(p.x * width / f64::from(cap_width), p.y * height / font.above) * self.scale)
                                .collect();
                            if points.iter().any(|p| !p.is_finite() || p.x.abs().max(p.y.abs()) > 1e9) {
                                return Err(Error("primitive coordinate limit"));
                            }
                            self.strokes.push(points);
                        }
                    }
                } else if execute {
                    self.shape(font, code, depth + 1)?;
                }
            }
            8 | 9 | 12 | 13 => loop {
                let x = r.byte()? as i8;
                let y = r.byte()? as i8;
                if matches!(op, 9 | 13) && x == 0 && y == 0 {
                    break;
                }
                let b = if matches!(op, 12 | 13) { r.byte()? as i8 } else { 0 };
                if execute {
                    self.bulge(Vec2::new(f64::from(x), f64::from(y)), b)?;
                }
                if matches!(op, 8 | 12) {
                    break;
                }
                self.spend()?;
            },
            10 | 11 => {
                let (start_off, end_off, radius) = if op == 11 {
                    let a = f64::from(r.byte()?) / 256.0;
                    let b = f64::from(r.byte()?) / 256.0;
                    let hi = r.byte()?;
                    let lo = r.byte()?;
                    (a, b, f64::from(u16::from(hi) * 256 + u16::from(lo)))
                } else {
                    (0.0, 0.0, f64::from(r.byte()?))
                };
                // Compiled SHX stores arc direction in bit 7 (not two's complement).
                if radius == 0.0 {
                    return Err(Error("zero arc radius"));
                }
                let spec = r.byte()?;
                let unsigned = spec & 0x7f;
                if unsigned > 0x77 {
                    return Err(Error("invalid octant specification"));
                }
                let oct = f64::from((unsigned >> 4) & 7);
                let count = unsigned & 15;
                if count > 7 {
                    return Err(Error("invalid octant count"));
                }
                let sign = if spec & 0x80 != 0 { -1.0 } else { 1.0 };
                let start = (oct + sign * start_off) * FRAC_PI_4;
                let sweep = if op == 10 {
                    sign * if count == 0 { TAU } else { f64::from(count) * FRAC_PI_4 }
                } else {
                    sign * (f64::from(if count == 0 { 8 } else { count }) - if end_off == 0.0 { 0.0 } else { 1.0 } + end_off - start_off) * FRAC_PI_4
                };
                if sweep * sign <= 0.0 {
                    return Err(Error("invalid fractional arc span"));
                }
                if execute {
                    self.arc(radius * self.scale, start, sweep)?;
                }
            }
            14 => {
                let next = r.byte()?;
                if next == 14 {
                    return Err(Error("nested vertical command"));
                }
                self.command(font, r, next, depth, false)?;
            }
            15 => return Err(Error("reserved shape opcode")),
            _ => {
                if execute {
                    const DIR: [(f64, f64); 16] = [
                        (1., 0.),
                        (1., 0.5),
                        (1., 1.),
                        (0.5, 1.),
                        (0., 1.),
                        (-0.5, 1.),
                        (-1., 1.),
                        (-1., 0.5),
                        (-1., 0.),
                        (-1., -0.5),
                        (-1., -1.),
                        (-0.5, -1.),
                        (0., -1.),
                        (0.5, -1.),
                        (1., -1.),
                        (1., -0.5),
                    ];
                    let (x, y) = DIR.get(usize::from(op & 15)).copied().ok_or(Error("invalid vector"))?;
                    self.to(self.pos + Vec2::new(x, y) * (f64::from(op >> 4) * self.scale))?;
                }
            }
        }
        Ok(())
    }
}
