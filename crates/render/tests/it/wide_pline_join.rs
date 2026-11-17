//! Wide polyline segments meet at their vertices: the outer corner is filled with a miter
//! (bevelled when very sharp), as AutoCAD joins wide polyline segments, instead of each segment
//! ending square and leaving a notch on the outside of every turn.

use cadcraft_doc::{Common, Drawing, EntityKind, LwPolyline, Space};
use cadcraft_geom::{PolyVertex, Vec2};
use cadcraft_render::{Kind, Options, build};

const W: f64 = 1.0;

fn pline(vs: Vec<PolyVertex>, closed: bool, const_width: f64) -> Drawing {
    let mut d = Drawing::new_imperial();
    let p = LwPolyline { vertices: vs, closed, const_width, elevation: 0.0, plinegen: false };
    d.add(&Space::Model, Common::default(), EntityKind::LwPolyline(p)).unwrap();
    d
}

fn pts(xy: &[(f64, f64)]) -> Vec<PolyVertex> {
    xy.iter().map(|&(x, y)| PolyVertex::new(Vec2::new(x, y))).collect()
}

fn tris(d: &Drawing) -> Vec<Vec2> {
    let l = build(d, &Space::Model, &Options::default());
    l.prims.iter().filter(|p| p.kind == Kind::Tris).flat_map(|p| l.points(p).to_vec()).collect()
}

fn covered(t: &[Vec2], p: Vec2) -> bool {
    t.chunks(3).any(|t| {
        let s = |a: Vec2, b: Vec2| (b - a).cross(p - a);
        let (u, v, w) = (s(t[0], t[1]), s(t[1], t[2]), s(t[2], t[0]));
        (u >= 0.0 && v >= 0.0 && w >= 0.0) || (u <= 0.0 && v <= 0.0 && w <= 0.0)
    })
}

/// Fraction of a 20 × 20 grid over the square `[c - h, c + h]` (cell centres) that is covered.
fn fill(t: &[Vec2], lo: Vec2, hi: Vec2) -> f64 {
    let n = 20;
    let mut f = 0;
    for i in 0..n {
        for j in 0..n {
            let p = Vec2::new(lo.x + (hi.x - lo.x) * (i as f64 + 0.5) / n as f64, lo.y + (hi.y - lo.y) * (j as f64 + 0.5) / n as f64);
            f += usize::from(covered(t, p));
        }
    }
    f as f64 / (n * n) as f64
}

/// The outer quadrant of a right-angle corner at `c`, outward along `o` (diagonal signs).
fn outer_quadrant(t: &[Vec2], c: Vec2, o: (f64, f64)) -> f64 {
    let h = W / 2.0;
    let q = c + Vec2::new(o.0 * h, o.1 * h);
    fill(t, Vec2::new(c.x.min(q.x), c.y.min(q.y)), Vec2::new(c.x.max(q.x), c.y.max(q.y)))
}

#[test]
fn l_corner_is_square() {
    let t = tris(&pline(pts(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)]), false, W));
    assert_eq!(outer_quadrant(&t, Vec2::new(10.0, 0.0), (1.0, -1.0)), 1.0, "outer corner filled");
    // A right-angle miter ends exactly at the band edges: nothing past them.
    assert!(!covered(&t, Vec2::new(10.55, -0.2)) && !covered(&t, Vec2::new(10.2, -0.55)));
}

#[test]
fn u_shape_both_corners() {
    let t = tris(&pline(pts(&[(0.0, 10.0), (0.0, 0.0), (10.0, 0.0), (10.0, 10.0)]), false, W));
    assert_eq!(outer_quadrant(&t, Vec2::new(0.0, 0.0), (-1.0, -1.0)), 1.0);
    assert_eq!(outer_quadrant(&t, Vec2::new(10.0, 0.0), (1.0, -1.0)), 1.0);
}

#[test]
fn closed_polyline_joins_its_closing_vertex() {
    let t = tris(&pline(pts(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)]), true, W));
    for (c, o) in [((0.0, 0.0), (-1.0, -1.0)), ((10.0, 0.0), (1.0, -1.0)), ((10.0, 10.0), (1.0, 1.0)), ((0.0, 10.0), (-1.0, 1.0))] {
        assert_eq!(outer_quadrant(&t, Vec2::new(c.0, c.1), o), 1.0, "corner {c:?}");
    }
}

#[test]
fn acute_corner_is_mitred_to_a_point() {
    // A 30° corner: the miter tip is 1 / sin(15°) ≈ 3.86 half widths out, under the limit of 10.
    let a = 30f64.to_radians();
    let t = tris(&pline(pts(&[(0.0, 0.0), (10.0, 0.0), (10.0 - 10.0 * a.cos(), 10.0 * a.sin())]), false, W));
    let tip_dist = (W / 2.0) / (a / 2.0).sin();
    let bis = Vec2::new((a / 2.0).cos(), -(a / 2.0).sin()); // outward, away from both legs
    let c = Vec2::new(10.0, 0.0);
    assert!(covered(&t, c + bis * (tip_dist * 0.95)), "the miter reaches towards its tip");
    assert!(!covered(&t, c + bis * (tip_dist * 1.05)), "and stops there");
}

#[test]
fn hairpin_past_the_miter_limit_is_bevelled() {
    // A 5° corner: the miter would reach ~23 half widths, past the limit of 10, so it is cut.
    let a = 5f64.to_radians();
    let t = tris(&pline(pts(&[(0.0, 0.0), (10.0, 0.0), (10.0 - 10.0 * a.cos(), 10.0 * a.sin())]), false, W));
    let bis = Vec2::new((a / 2.0).cos(), -(a / 2.0).sin());
    let c = Vec2::new(10.0, 0.0);
    // The bevel edge passes sin(2.5°) half widths out along the bisector; just inside it is filled.
    assert!(covered(&t, c + bis * (W / 2.0 * (a / 2.0).sin() * 0.9)), "the bevel fills the outer corner");
    assert!(!covered(&t, c + bis * (W / 2.0 * 3.0)), "no long spike");
}

#[test]
fn line_into_arc_corner_is_joined_and_the_arc_band_is_kept() {
    // A line along +x into a clockwise half circle from (0,0) to (10,0) that leaves along +y.
    let mut vs = pts(&[(-10.0, 0.0), (0.0, 0.0), (10.0, 0.0)]);
    vs[1].bulge = -1.0;
    let t = tris(&pline(vs, false, W));
    let q = outer_quadrant(&t, Vec2::new(0.0, 0.0), (1.0, -1.0));
    assert!(q >= 0.95, "outer corner filled: {q}");
    assert!(covered(&t, Vec2::new(5.0, 5.0)), "the arc band is there");
    assert!(!covered(&t, Vec2::new(5.0, 0.0)) && !covered(&t, Vec2::new(5.0, 5.6)), "and only the band");
}

#[test]
fn a_width_step_stays_a_step() {
    // Segment widths 1 then 2 at the shared vertex: no join is invented between unequal bands.
    let mut vs = pts(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)]);
    (vs[0].start_width, vs[0].end_width) = (1.0, 1.0);
    (vs[1].start_width, vs[1].end_width) = (2.0, 2.0);
    let t = tris(&pline(vs, false, 0.0));
    assert_eq!(t.len(), 12, "two quads only");
}

#[test]
fn straight_and_reversing_vertices_add_nothing() {
    let straight = tris(&pline(pts(&[(0.0, 0.0), (5.0, 0.0), (10.0, 0.0)]), false, W));
    assert_eq!(straight.len(), 12);
    let back = tris(&pline(pts(&[(0.0, 0.0), (10.0, 0.0), (5.0, 0.0)]), false, W));
    assert!(back.iter().all(|p| p.is_finite()));
    assert_eq!(back.len(), 12, "a 180° turn has no outside to fill");
}
