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

/// A layout viewport that sees only a sharp corner's miter tip still draws it: the polyline's
/// extents, which viewports cull by, contain the tip.
#[test]
fn viewport_showing_only_a_miter_tip_draws_it() {
    use cadcraft_doc::Viewport;
    use cadcraft_geom::Vec3;
    use cadcraft_render::{VIEWPORT_CONTENT, build_plot};
    // 30° corner at (10, 0), half width 0.5: the tip is at about (11.87, -0.5), far past both
    // bands (x ≤ 10.25).
    let a = 30f64.to_radians();
    let mut d = pline(pts(&[(0.0, 0.0), (10.0, 0.0), (10.0 - 10.0 * a.cos(), 10.0 * a.sin())]), false, W);
    let vp = Viewport {
        center: Vec3::new(5.0, 4.0, 0.0),
        width: 8.0,
        height: 6.0,
        view_center: Vec2::new(11.4, -0.4),
        view_height: 0.3,
        id: 2,
        locked: false,
        frozen_layers: Vec::new(),
        layer_colors: Vec::new(),
        clip: None,
        on: true,
        twist: 0.0,
    };
    d.add(&Space::Paper("Layout1".into()), Common::default(), EntityKind::Viewport(vp)).unwrap();
    let space = Space::Paper("Layout1".into());
    for l in [build(&d, &space, &Options::default()), build_plot(&d, &space, &Options::default())] {
        assert!(l.prims.iter().any(|p| p.handle == VIEWPORT_CONTENT && p.kind == Kind::Tris), "the tip is drawn");
    }
}

/// The drawn triangles of a single wide polyline and that polyline's extents.
fn drawn_and_bounds(vs: Vec<PolyVertex>, closed: bool, const_width: f64) -> (Vec<Vec2>, cadcraft_geom::Bounds2) {
    let mut d = Drawing::new_imperial();
    let p = LwPolyline { vertices: vs, closed, const_width, elevation: 0.0, plinegen: false };
    let h = d.add(&Space::Model, Common::default(), EntityKind::LwPolyline(p)).unwrap();
    let b = cadcraft_doc::entity_bounds(&d, d.entity(h).unwrap(), 0);
    (tris(&d), b)
}

/// How far the drawn triangles reach past the extents (0 when inside).
fn overshoot(t: &[Vec2], b: &cadcraft_geom::Bounds2) -> f64 {
    t.iter().map(|q| (b.min.x - q.x).max(q.x - b.max.x).max(b.min.y - q.y).max(q.y - b.max.y).max(0.0)).fold(0.0, f64::max)
}

/// A line into a counter-clockwise quarter arc of radius 5 that leaves at 150°: a sharp corner
/// next to an arc, whose tip (about (1.87, -0.5) at half width 0.5) is far past both bands.
fn line_into_sharp_arc() -> Vec<PolyVertex> {
    let mut vs = pts(&[(-10.0, 0.0), (0.0, 0.0), (-2.5 - 5.0 * 0.866_025_403_784_438_6, -4.330_127_018_922_193 + 2.5)]);
    vs[1].bulge = (std::f64::consts::FRAC_PI_2 / 4.0).tan();
    vs
}

#[test]
fn joins_next_to_arcs_stay_inside_the_extents() {
    // The arc band's own sampling may sit a hair outside (it predates the joins); a join tip
    // found along a different direction than the extents use would miss by tenths.
    let (t, b) = drawn_and_bounds(line_into_sharp_arc(), false, W);
    assert!(overshoot(&t, &b) < 0.01, "{} past {b:?}", overshoot(&t, &b));
    assert!(b.max.x > 1.8, "the tip is in the extents: {b:?}");
    // A tiny arc (radius 0.1) turning back 176°: past the miter limit along the tangent, so
    // bevelled in both, however finely the arc is cut into chords.
    let mut vs = pts(&[(-10.0, 0.0), (0.0, 0.0), (-0.092_780_757_651_569_87, 0.106_732_052_400_394_96)]);
    vs[1].bulge = (-std::f64::consts::FRAC_PI_2 / 4.0).tan();
    let (t, b) = drawn_and_bounds(vs, false, 0.1);
    assert!(overshoot(&t, &b) < 0.01, "{} past {b:?}", overshoot(&t, &b));
}

#[test]
fn viewport_showing_only_a_miter_tip_next_to_an_arc_draws_it() {
    use cadcraft_doc::Viewport;
    use cadcraft_geom::Vec3;
    use cadcraft_render::{VIEWPORT_CONTENT, build_plot};
    let mut d = pline(line_into_sharp_arc(), false, W);
    let vp = Viewport {
        center: Vec3::new(5.0, 4.0, 0.0),
        width: 8.0,
        height: 6.0,
        view_center: Vec2::new(1.4, -0.44),
        view_height: 0.1,
        id: 2,
        locked: false,
        frozen_layers: Vec::new(),
        layer_colors: Vec::new(),
        clip: None,
        on: true,
        twist: 0.0,
    };
    d.add(&Space::Paper("Layout1".into()), Common::default(), EntityKind::Viewport(vp)).unwrap();
    let space = Space::Paper("Layout1".into());
    for l in [build(&d, &space, &Options::default()), build_plot(&d, &space, &Options::default())] {
        assert!(l.prims.iter().any(|p| p.handle == VIEWPORT_CONTENT && p.kind == Kind::Tris), "the tip is drawn");
    }
}

#[test]
fn a_repeated_vertex_does_not_shift_the_widths() {
    // The repeated (10, 0) starts no segment; the 30° leg's widths are its own (2), not the
    // first leg's (1). Unequal widths at the corner: no join, and nothing past the extents.
    let mut vs = pts(&[(0.0, 0.0), (10.0, 0.0), (10.0, 0.0), (10.0 - 10.0 * 30f64.to_radians().cos(), 10.0 * 30f64.to_radians().sin())]);
    for (i, v) in vs.iter_mut().enumerate() {
        let w = if i == 2 { 2.0 } else { 1.0 };
        (v.start_width, v.end_width) = (w, w);
    }
    let (t, b) = drawn_and_bounds(vs.clone(), false, 0.0);
    assert!(overshoot(&t, &b) < 1e-9, "{} past {b:?}", overshoot(&t, &b));
    assert!(b.max.x < 10.6, "no tip: {b:?}");
    // The 30° leg is drawn 2 wide: a point 0.9 from its centre line is covered.
    let mid = Vec2::new(10.0 - 5.0 * 30f64.to_radians().cos(), 5.0 * 30f64.to_radians().sin());
    let nrm = Vec2::new(-30f64.to_radians().sin(), -30f64.to_radians().cos());
    assert!(covered(&t, mid + nrm * 0.9), "second leg keeps its own width");
    // The same with equal widths joins, and the tip is inside the extents; closed too.
    for v in &mut vs {
        (v.start_width, v.end_width) = (1.0, 1.0);
    }
    let (t, b) = drawn_and_bounds(vs.clone(), false, 0.0);
    assert!(overshoot(&t, &b) < 1e-9 && b.max.x > 11.8, "{b:?}");
    let (t, b) = drawn_and_bounds(vs, true, 0.0);
    assert!(overshoot(&t, &b) < 1e-9, "closed: {} past {b:?}", overshoot(&t, &b));
}
