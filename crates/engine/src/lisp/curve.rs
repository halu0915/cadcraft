//! The `vlax-curve-*` functions: lengths, points and parameters along lines, arcs, circles and
//! polylines, for an entity name or a `vla-object`.
//!
//! Parameters follow AutoCAD's curves: a line's parameter is the distance from its start point;
//! an arc's and a circle's is the angle in radians (an arc runs from its start angle to its start
//! angle plus its sweep, a circle from 0 to 2π); a polyline's is the vertex number, each segment
//! (a straight one or a bulge arc) spanning one, with the fraction inside a segment proportional
//! to its length. Points are 3D: a lightweight polyline's at its elevation, an arc's and a
//! circle's at its centre's z. The object coordinate system of an extruded entity is not applied.

use std::sync::Arc;

use cadcraft_doc::{Entity, EntityKind};
use cadcraft_geom::{Arc as GArc, PolyVertex, Polyline, Segment, TAU, Vec3, bulge_to_arc, norm_angle};

use super::LispError;
use super::activex;
use super::eval::{Ctx, Flow};
use super::value::Value;

/// One piece of a polyline: a straight segment, or an arc made from a bulge.
#[derive(Clone, Debug)]
pub(crate) enum Piece {
    Straight(Vec3, Vec3),
    Bulge { arc: GArc, ccw: bool, z: f64 },
}

impl Piece {
    fn len(&self) -> f64 {
        match self {
            Piece::Straight(a, b) => dist3(*a, *b),
            Piece::Bulge { arc, .. } => arc.len(),
        }
    }
    /// The point a fraction `f` (0..1) along the piece.
    fn at(&self, f: f64) -> Vec3 {
        match self {
            Piece::Straight(a, b) => lerp3(*a, *b, f),
            Piece::Bulge { arc, ccw, z } => Segment::Arc { arc: *arc, ccw: *ccw }.at(f).to3(*z),
        }
    }
    /// The derivative with respect to the fraction (the piece spans one unit of parameter).
    fn deriv(&self, f: f64) -> Vec3 {
        match self {
            Piece::Straight(a, b) => sub3(*b, *a),
            Piece::Bulge { arc, ccw, .. } => {
                let t = Segment::Arc { arc: *arc, ccw: *ccw }.tangent(f) * arc.len();
                Vec3::new(t.x, t.y, 0.0)
            }
        }
    }
    /// The fraction of the point on the piece nearest `p`, and that point.
    fn closest(&self, p: Vec3) -> (f64, Vec3) {
        let f = match self {
            Piece::Straight(a, b) => {
                let d = sub3(*b, *a);
                let l2 = dot3(d, d);
                if l2 <= f64::EPSILON { 0.0 } else { (dot3(sub3(p, *a), d) / l2).clamp(0.0, 1.0) }
            }
            Piece::Bulge { arc, ccw, .. } => {
                let sweep = arc.sweep();
                let d = norm_angle(arc.center.angle_to(p.xy()) - arc.start);
                let t = if d <= sweep {
                    d / sweep
                } else if (d - sweep) < (TAU - d) {
                    1.0
                } else {
                    0.0
                };
                if *ccw { t } else { 1.0 - t }
            }
        };
        (f, self.at(f))
    }
}

/// A curve as AutoCAD parametrizes it.
#[derive(Clone, Debug)]
pub(crate) enum Curve {
    /// Parameter: the distance from `a`.
    Line { a: Vec3, b: Vec3 },
    /// Parameter: the angle, from `start` to `start + sweep` (a circle: 0 to 2π).
    Arc { center: Vec3, radius: f64, start: f64, sweep: f64, circle: bool },
    /// Parameter: the vertex number. `vertices` keeps the 2D outline for the area.
    Poly { pieces: Vec<Piece>, closed: bool, vertices: Vec<PolyVertex> },
}

/// Parameters within this much (relative) of an end still count as on the curve.
const PARAM_TOL: f64 = 1e-9;

impl Curve {
    /// The curve of an entity, or `None` for an entity that isn't one of the curves measured
    /// here.
    pub(crate) fn of(kind: &EntityKind) -> Option<Curve> {
        Some(match kind {
            EntityKind::Line(l) => Curve::Line { a: l.a, b: l.b },
            EntityKind::Arc(a) => {
                let g = GArc::new(a.center.xy(), a.radius, a.start, a.end);
                Curve::Arc { center: a.center, radius: a.radius.abs(), start: g.start, sweep: g.sweep(), circle: false }
            }
            EntityKind::Circle(c) => Curve::Arc { center: c.center, radius: c.radius.abs(), start: 0.0, sweep: TAU, circle: true },
            EntityKind::LwPolyline(p) => {
                let n = p.vertices.len();
                let count = if p.closed { n } else { n.saturating_sub(1) };
                let z = p.elevation;
                let mut pieces = Vec::with_capacity(count);
                for i in 0..count {
                    let (Some(v), Some(w)) = (p.vertices.get(i), p.vertices.get((i + 1) % n.max(1))) else { continue };
                    pieces.push(match bulge_to_arc(v.p, w.p, v.bulge) {
                        Some((arc, ccw)) => Piece::Bulge { arc, ccw, z },
                        None => Piece::Straight(v.p.to3(z), w.p.to3(z)),
                    });
                }
                if n == 0 {
                    return None;
                }
                Curve::Poly { pieces, closed: p.closed, vertices: p.vertices.clone() }
            }
            EntityKind::Polyline3d(p) => {
                let n = p.points.len();
                let count = if p.closed { n } else { n.saturating_sub(1) };
                let pieces = (0..count).filter_map(|i| Some(Piece::Straight(*p.points.get(i)?, *p.points.get((i + 1) % n.max(1))?))).collect();
                if n == 0 {
                    return None;
                }
                Curve::Poly { pieces, closed: p.closed, vertices: p.points.iter().map(|q| PolyVertex::new(q.xy())).collect() }
            }
            _ => return None,
        })
    }

    pub(crate) fn start_param(&self) -> f64 {
        match self {
            Curve::Line { .. } | Curve::Poly { .. } => 0.0,
            Curve::Arc { start, circle, .. } => {
                if *circle {
                    0.0
                } else {
                    *start
                }
            }
        }
    }

    pub(crate) fn end_param(&self) -> f64 {
        match self {
            Curve::Line { a, b } => dist3(*a, *b),
            Curve::Arc { start, sweep, circle, .. } => {
                if *circle {
                    TAU
                } else {
                    start + sweep
                }
            }
            Curve::Poly { pieces, .. } => pieces.len() as f64,
        }
    }

    pub(crate) fn length(&self) -> f64 {
        match self {
            Curve::Line { a, b } => dist3(*a, *b),
            Curve::Arc { radius, sweep, .. } => radius * sweep,
            Curve::Poly { pieces, .. } => pieces.iter().map(Piece::len).sum(),
        }
    }

    /// `param` clamped to the curve when it is within tolerance of it; `None` when outside.
    fn on_curve(&self, param: f64) -> Option<f64> {
        if !param.is_finite() {
            return None;
        }
        let (s, e) = (self.start_param(), self.end_param());
        let tol = PARAM_TOL * s.abs().max(e.abs()).max(1.0);
        (param >= s - tol && param <= e + tol).then(|| param.clamp(s, e))
    }

    /// The polyline piece a parameter falls in and the fraction inside it.
    fn piece_at(pieces: &[Piece], param: f64) -> Option<(&Piece, f64)> {
        let last = pieces.len().checked_sub(1)?;
        let i = (param.floor().max(0.0) as usize).min(last);
        Some((pieces.get(i)?, param - i as f64))
    }

    pub(crate) fn point_at_param(&self, param: f64) -> Option<Vec3> {
        let t = self.on_curve(param)?;
        Some(match self {
            Curve::Line { a, b } => {
                let l = dist3(*a, *b);
                if l <= f64::EPSILON { *a } else { lerp3(*a, *b, t / l) }
            }
            Curve::Arc { center, radius, .. } => Vec3::new(center.x + radius * t.cos(), center.y + radius * t.sin(), center.z),
            Curve::Poly { pieces, vertices, .. } => match Self::piece_at(pieces, t) {
                Some((p, f)) => p.at(f),
                // A single vertex: the curve is that point.
                None => vertices.first().map(|v| v.p.to3(self.z()))?,
            },
        })
    }

    /// The z of a polyline's points (its elevation; a 3D polyline's first point).
    fn z(&self) -> f64 {
        match self {
            Curve::Poly { pieces, .. } => pieces.first().map(|p| p.at(0.0).z).unwrap_or(0.0),
            Curve::Line { a, .. } => a.z,
            Curve::Arc { center, .. } => center.z,
        }
    }

    pub(crate) fn dist_at_param(&self, param: f64) -> Option<f64> {
        let t = self.on_curve(param)?;
        Some(match self {
            Curve::Line { .. } => t,
            Curve::Arc { radius, .. } => radius * (t - self.start_param()),
            Curve::Poly { pieces, .. } => {
                let Some((_, f)) = Self::piece_at(pieces, t) else { return Some(0.0) };
                let i = (t - f).round().max(0.0) as usize;
                pieces.iter().take(i).map(Piece::len).sum::<f64>() + pieces.get(i).map_or(0.0, |p| p.len() * f)
            }
        })
    }

    pub(crate) fn param_at_dist(&self, dist: f64) -> Option<f64> {
        let len = self.length();
        let tol = PARAM_TOL * len.max(1.0);
        if !dist.is_finite() || dist < -tol || dist > len + tol {
            return None;
        }
        let d = dist.clamp(0.0, len);
        Some(match self {
            Curve::Line { .. } => d,
            Curve::Arc { radius, .. } => {
                if *radius <= f64::EPSILON {
                    self.start_param()
                } else {
                    self.start_param() + d / radius
                }
            }
            Curve::Poly { pieces, .. } => {
                let mut left = d;
                for (i, p) in pieces.iter().enumerate() {
                    let l = p.len();
                    if left <= l || i + 1 == pieces.len() {
                        let f = if l <= f64::EPSILON { 0.0 } else { (left / l).clamp(0.0, 1.0) };
                        return Some(i as f64 + f);
                    }
                    left -= l;
                }
                0.0
            }
        })
    }

    /// The parameter of the point on the curve nearest `p`, that point, and how far `p` is from it.
    pub(crate) fn closest(&self, p: Vec3) -> (f64, Vec3, f64) {
        let (param, q) = match self {
            Curve::Line { a, b } => {
                let piece = Piece::Straight(*a, *b);
                let (f, q) = piece.closest(p);
                (f * dist3(*a, *b), q)
            }
            Curve::Arc { center, radius, start, sweep, circle } => {
                let ang = norm_angle(center.xy().angle_to(p.xy()) - start);
                let d = if ang <= *sweep {
                    ang
                } else if (ang - sweep) < (TAU - ang) {
                    *sweep
                } else {
                    0.0
                };
                let param = if *circle { norm_angle(start + d) } else { start + d };
                (param, Vec3::new(center.x + radius * (start + d).cos(), center.y + radius * (start + d).sin(), center.z))
            }
            Curve::Poly { pieces, vertices, .. } => {
                let mut best: Option<(f64, f64, Vec3)> = None;
                for (i, piece) in pieces.iter().enumerate() {
                    let (f, q) = piece.closest(p);
                    let d = dist3(p, q);
                    if best.is_none_or(|(bd, _, _)| d < bd) {
                        best = Some((d, i as f64 + f, q));
                    }
                }
                match best {
                    Some((_, param, q)) => (param, q),
                    None => (0.0, vertices.first().map(|v| v.p.to3(self.z())).unwrap_or(p)),
                }
            }
        };
        (param, q, dist3(p, q))
    }

    /// The first derivative of the point with respect to the parameter.
    pub(crate) fn deriv_at_param(&self, param: f64) -> Option<Vec3> {
        let t = self.on_curve(param)?;
        Some(match self {
            Curve::Line { a, b } => {
                let l = dist3(*a, *b);
                if l <= f64::EPSILON { Vec3::new(0.0, 0.0, 0.0) } else { scale3(sub3(*b, *a), 1.0 / l) }
            }
            Curve::Arc { radius, .. } => Vec3::new(-radius * t.sin(), radius * t.cos(), 0.0),
            Curve::Poly { pieces, .. } => match Self::piece_at(pieces, t) {
                Some((p, f)) => p.deriv(f),
                None => Vec3::new(0.0, 0.0, 0.0),
            },
        })
    }

    /// Closed: a circle, a polyline marked closed, or a curve whose ends meet.
    pub(crate) fn is_closed(&self) -> bool {
        match self {
            Curve::Arc { circle: true, .. } => true,
            Curve::Poly { closed: true, .. } => true,
            Curve::Line { .. } | Curve::Arc { .. } => false,
            Curve::Poly { pieces, .. } => {
                let (Some(s), Some(e)) = (self.point_at_param(self.start_param()), self.point_at_param(self.end_param())) else { return false };
                !pieces.is_empty() && dist3(s, e) <= 1e-9 * self.length().max(1.0)
            }
        }
    }

    /// The area AutoCAD reports: a circle's disc; an arc's region between it and its chord; a
    /// polyline's as if closed (bulges included); none for a line.
    pub(crate) fn area(&self) -> f64 {
        match self {
            Curve::Line { .. } => 0.0,
            Curve::Arc { radius, sweep, circle, .. } => {
                if *circle {
                    std::f64::consts::PI * radius * radius
                } else {
                    0.5 * radius * radius * (sweep - sweep.sin())
                }
            }
            Curve::Poly { vertices, .. } => Polyline { vertices: vertices.clone(), closed: true }.area().abs(),
        }
    }
}

fn sub3(a: Vec3, b: Vec3) -> Vec3 {
    Vec3::new(a.x - b.x, a.y - b.y, a.z - b.z)
}
fn dot3(a: Vec3, b: Vec3) -> f64 {
    a.x * b.x + a.y * b.y + a.z * b.z
}
fn scale3(a: Vec3, k: f64) -> Vec3 {
    Vec3::new(a.x * k, a.y * k, a.z * k)
}
fn dist3(a: Vec3, b: Vec3) -> f64 {
    let d = sub3(a, b);
    dot3(d, d).sqrt()
}
fn lerp3(a: Vec3, b: Vec3, f: f64) -> Vec3 {
    Vec3::new(a.x + (b.x - a.x) * f, a.y + (b.y - a.y) * f, a.z + (b.z - a.z) * f)
}

// ---------------- the built-ins ----------------

fn arity(a: &[Value], min: usize, max: usize) -> Result<(), LispError> {
    if a.len() < min {
        return Err(super::builtins::args_err(true));
    }
    if a.len() > max {
        return Err(super::builtins::args_err(false));
    }
    Ok(())
}

/// The entity an entity name or `vla-object` names.
fn entity_arg(ctx: &Ctx<'_>, v: &Value) -> Result<Arc<Entity>, LispError> {
    let d = ctx.s.doc().map_err(|e| LispError::new(e.to_string()))?;
    let found = match v {
        Value::Ename(h) => activex::find_entity(d, *h).map(|(e, _)| e),
        Value::Vla(o) => activex::entity_of(d, o),
        _ => None,
    };
    found.ok_or_else(|| LispError::bad_arg("lentityp", v))
}

/// The curve of the first argument (an entity name or a `vla-object`).
fn curve_arg(ctx: &Ctx<'_>, v: &Value) -> Result<Curve, LispError> {
    let e = entity_arg(ctx, v)?;
    Curve::of(&e.kind).ok_or_else(|| LispError::new(format!("bad argument type: curvep: {}", super::value::to_text(v, true))))
}

fn number(v: &Value) -> Result<f64, LispError> {
    v.as_number().ok_or_else(|| LispError::bad_arg("numberp", v))
}

fn point3(v: &Value) -> Result<Vec3, LispError> {
    let p = activex::point_value(v).ok_or_else(|| LispError::bad_arg("2D/3D point", v))?;
    Ok(Vec3::new(p[0], p[1], p[2]))
}

fn point_value(p: Vec3) -> Value {
    Value::point(&[p.x, p.y, p.z])
}

fn opt<T>(v: Option<T>, f: impl FnOnce(T) -> Value) -> Flow {
    Flow::Value(v.map(f).unwrap_or_default())
}

pub fn get_start_param(ctx: &mut Ctx<'_>, a: Vec<Value>) -> Result<Flow, LispError> {
    arity(&a, 1, 1)?;
    Ok(Flow::Value(Value::Real(curve_arg(ctx, &a[0])?.start_param())))
}

pub fn get_end_param(ctx: &mut Ctx<'_>, a: Vec<Value>) -> Result<Flow, LispError> {
    arity(&a, 1, 1)?;
    Ok(Flow::Value(Value::Real(curve_arg(ctx, &a[0])?.end_param())))
}

pub fn get_start_point(ctx: &mut Ctx<'_>, a: Vec<Value>) -> Result<Flow, LispError> {
    arity(&a, 1, 1)?;
    let c = curve_arg(ctx, &a[0])?;
    Ok(opt(c.point_at_param(c.start_param()), point_value))
}

pub fn get_end_point(ctx: &mut Ctx<'_>, a: Vec<Value>) -> Result<Flow, LispError> {
    arity(&a, 1, 1)?;
    let c = curve_arg(ctx, &a[0])?;
    Ok(opt(c.point_at_param(c.end_param()), point_value))
}

pub fn get_point_at_param(ctx: &mut Ctx<'_>, a: Vec<Value>) -> Result<Flow, LispError> {
    arity(&a, 2, 2)?;
    let c = curve_arg(ctx, &a[0])?;
    Ok(opt(c.point_at_param(number(&a[1])?), point_value))
}

pub fn get_dist_at_param(ctx: &mut Ctx<'_>, a: Vec<Value>) -> Result<Flow, LispError> {
    arity(&a, 2, 2)?;
    let c = curve_arg(ctx, &a[0])?;
    Ok(opt(c.dist_at_param(number(&a[1])?), Value::Real))
}

pub fn get_param_at_dist(ctx: &mut Ctx<'_>, a: Vec<Value>) -> Result<Flow, LispError> {
    arity(&a, 2, 2)?;
    let c = curve_arg(ctx, &a[0])?;
    Ok(opt(c.param_at_dist(number(&a[1])?), Value::Real))
}

pub fn get_point_at_dist(ctx: &mut Ctx<'_>, a: Vec<Value>) -> Result<Flow, LispError> {
    arity(&a, 2, 2)?;
    let c = curve_arg(ctx, &a[0])?;
    Ok(opt(c.param_at_dist(number(&a[1])?).and_then(|t| c.point_at_param(t)), point_value))
}

/// How near a point must be to count as on the curve (`getParamAtPoint`, `getDistAtPoint`).
fn on_curve_tol(c: &Curve) -> f64 {
    1e-8 * c.length().max(1.0)
}

pub fn get_param_at_point(ctx: &mut Ctx<'_>, a: Vec<Value>) -> Result<Flow, LispError> {
    arity(&a, 2, 2)?;
    let c = curve_arg(ctx, &a[0])?;
    let (param, _, d) = c.closest(point3(&a[1])?);
    Ok(Flow::Value(if d <= on_curve_tol(&c) { Value::Real(param) } else { Value::Nil }))
}

pub fn get_dist_at_point(ctx: &mut Ctx<'_>, a: Vec<Value>) -> Result<Flow, LispError> {
    arity(&a, 2, 2)?;
    let c = curve_arg(ctx, &a[0])?;
    let (param, _, d) = c.closest(point3(&a[1])?);
    Ok(opt((d <= on_curve_tol(&c)).then(|| c.dist_at_param(param)).flatten(), Value::Real))
}

/// `(vlax-curve-getClosestPointTo curve point [extend])`: the extension flag is ignored (the
/// curve itself is searched).
pub fn get_closest_point_to(ctx: &mut Ctx<'_>, a: Vec<Value>) -> Result<Flow, LispError> {
    arity(&a, 2, 3)?;
    let c = curve_arg(ctx, &a[0])?;
    Ok(Flow::Value(point_value(c.closest(point3(&a[1])?).1)))
}

pub fn get_first_deriv(ctx: &mut Ctx<'_>, a: Vec<Value>) -> Result<Flow, LispError> {
    arity(&a, 2, 2)?;
    let c = curve_arg(ctx, &a[0])?;
    Ok(opt(c.deriv_at_param(number(&a[1])?), point_value))
}

pub fn is_closed(ctx: &mut Ctx<'_>, a: Vec<Value>) -> Result<Flow, LispError> {
    arity(&a, 1, 1)?;
    Ok(Flow::Value(Value::bool(curve_arg(ctx, &a[0])?.is_closed())))
}

pub fn get_area(ctx: &mut Ctx<'_>, a: Vec<Value>) -> Result<Flow, LispError> {
    arity(&a, 1, 1)?;
    Ok(Flow::Value(Value::Real(curve_arg(ctx, &a[0])?.area())))
}
