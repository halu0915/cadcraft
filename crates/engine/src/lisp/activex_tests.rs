//! Visual LISP's ActiveX layer (`activex.rs`) and curve measurements (`curve.rs`), checked against
//! values worked out by hand: a 3-4-5 line, quarter and half arcs, a unit circle, polylines with
//! positive and negative bulges.

#![allow(clippy::unwrap_used)]

use cadcraft_doc::{Arc as DArc, Circle, Common, EntityKind, Line, LwPolyline, Space, Text};
use cadcraft_geom::{PolyVertex, Vec2, Vec3};

use super::tests::session;
use super::value::{Value, to_text};
use crate::Session;

fn eval(s: &mut Session, code: &str) -> Result<String, String> {
    match s.lisp_eval(code, false, true, Vec::<Value>::new()) {
        Ok(Some(Ok(v))) => Ok(to_text(&v, true)),
        Ok(Some(Err(e))) => Err(e),
        Ok(None) => Err("did not finish".into()),
        Err(e) => Err(e.to_string()),
    }
}

fn ok(s: &mut Session, code: &str) -> String {
    match eval(s, code) {
        Ok(v) => v,
        Err(e) => panic!("{code}: {e}"),
    }
}

/// The number a LISP expression evaluates to, through `rtos` with ten decimals (a printed real
/// keeps six significant figures only).
fn num(s: &mut Session, code: &str) -> f64 {
    let v = ok(s, &format!("(rtos {code} 2 10)"));
    v.trim_matches('"').parse().unwrap_or_else(|_| panic!("{code}: not a number: {v}"))
}

fn near(s: &mut Session, code: &str, want: f64) {
    let got = num(s, code);
    assert!((got - want).abs() < 1e-6, "{code}: {got} (want {want})");
}

/// The three coordinates of a point a LISP expression evaluates to.
fn pt(s: &mut Session, code: &str, want: [f64; 3]) {
    for (i, w) in want.iter().enumerate() {
        near(s, &format!("(nth {i} {code})"), *w);
    }
}

/// Add an entity to model space; the LISP expression for its name.
fn add(s: &mut Session, kind: EntityKind) -> String {
    let h = s.doc_mut().unwrap().add(&Space::Model, Common::default(), kind).unwrap();
    s.touch();
    format!("(handent \"{}\")", h.hex())
}

fn line(s: &mut Session, a: [f64; 2], b: [f64; 2]) -> String {
    add(s, EntityKind::Line(Line { a: Vec3::new(a[0], a[1], 0.0), b: Vec3::new(b[0], b[1], 0.0) }))
}

fn pline(s: &mut Session, verts: &[([f64; 2], f64)], closed: bool) -> String {
    let vertices = verts.iter().map(|(p, b)| PolyVertex::with_bulge(Vec2::new(p[0], p[1]), *b)).collect();
    add(s, EntityKind::LwPolyline(LwPolyline { vertices, closed, const_width: 0.0, elevation: 0.0, plinegen: false }))
}

const PI: f64 = std::f64::consts::PI;

// ---------------- curves ----------------

/// A line's parameter is the distance from its start: 0 to its length.
#[test]
fn line_parameters_are_distances() {
    let mut s = session();
    let e = line(&mut s, [0.0, 0.0], [3.0, 4.0]);
    ok(&mut s, &format!("(setq e {e})"));
    near(&mut s, "(vlax-curve-getStartParam e)", 0.0);
    near(&mut s, "(vlax-curve-getEndParam e)", 5.0);
    pt(&mut s, "(vlax-curve-getStartPoint e)", [0.0, 0.0, 0.0]);
    pt(&mut s, "(vlax-curve-getEndPoint e)", [3.0, 4.0, 0.0]);
    near(&mut s, "(vlax-curve-getDistAtParam e 2.5)", 2.5);
    pt(&mut s, "(vlax-curve-getPointAtDist e 2.5)", [1.5, 2.0, 0.0]);
    pt(&mut s, "(vlax-curve-getPointAtParam e 5)", [3.0, 4.0, 0.0]);
    near(&mut s, "(vlax-curve-getParamAtPoint e '(1.5 2.0))", 2.5);
    near(&mut s, "(vlax-curve-getParamAtDist e 4)", 4.0);
    near(&mut s, "(vlax-curve-getDistAtPoint e '(3 4 0))", 5.0);
    pt(&mut s, "(vlax-curve-getClosestPointTo e '(3 0))", [1.08, 1.44, 0.0]);
    pt(&mut s, "(vlax-curve-getFirstDeriv e 1)", [0.6, 0.8, 0.0]);
    assert_eq!(ok(&mut s, "(vlax-curve-isClosed e)"), "nil");
    near(&mut s, "(vlax-curve-getArea e)", 0.0);
    // Off the curve: nil, as AutoCAD answers.
    assert_eq!(ok(&mut s, "(vlax-curve-getPointAtDist e 6)"), "nil");
    assert_eq!(ok(&mut s, "(vlax-curve-getDistAtParam e -1)"), "nil");
    assert_eq!(ok(&mut s, "(vlax-curve-getParamAtPoint e '(1 0))"), "nil");
    // A vla-object measures as its entity name does.
    near(&mut s, "(vlax-curve-getEndParam (vlax-ename->vla-object e))", 5.0);
}

/// An arc's parameter is the angle: from its start angle to start + sweep.
#[test]
fn arc_parameters_are_angles() {
    let mut s = session();
    let e = add(&mut s, EntityKind::Arc(DArc { center: Vec3::new(0.0, 0.0, 0.0), radius: 2.0, start: 0.0, end: PI / 2.0 }));
    ok(&mut s, &format!("(setq e {e})"));
    near(&mut s, "(vlax-curve-getStartParam e)", 0.0);
    near(&mut s, "(vlax-curve-getEndParam e)", PI / 2.0);
    pt(&mut s, "(vlax-curve-getStartPoint e)", [2.0, 0.0, 0.0]);
    pt(&mut s, "(vlax-curve-getEndPoint e)", [0.0, 2.0, 0.0]);
    near(&mut s, "(vlax-curve-getDistAtParam e (vlax-curve-getEndParam e))", PI);
    near(&mut s, "(vlax-curve-getDistAtParam e (/ pi 4))", PI / 2.0);
    pt(&mut s, "(vlax-curve-getPointAtDist e (/ pi 2))", [2f64.sqrt(), 2f64.sqrt(), 0.0]);
    near(&mut s, "(vlax-curve-getParamAtPoint e '(0 2))", PI / 2.0);
    near(&mut s, "(vlax-curve-getParamAtDist e pi)", PI / 2.0);
    pt(&mut s, "(vlax-curve-getFirstDeriv e 0)", [0.0, 2.0, 0.0]);
    assert_eq!(ok(&mut s, "(vlax-curve-isClosed e)"), "nil");
    // The region between the arc and its chord: r²/2 (θ - sin θ).
    near(&mut s, "(vlax-curve-getArea e)", 2.0 * (PI / 2.0 - 1.0));
    assert_eq!(ok(&mut s, "(vlax-curve-getPointAtParam e 3.0)"), "nil", "past the end angle");
}

/// An arc across angle 0 (270° to 90°): the end parameter is past 2π's turn, not below the start.
#[test]
fn an_arc_across_zero_has_its_end_after_its_start() {
    let mut s = session();
    let e = add(&mut s, EntityKind::Arc(DArc { center: Vec3::new(0.0, 0.0, 0.0), radius: 2.0, start: 1.5 * PI, end: 0.5 * PI }));
    ok(&mut s, &format!("(setq e {e})"));
    near(&mut s, "(vlax-curve-getStartParam e)", 1.5 * PI);
    near(&mut s, "(vlax-curve-getEndParam e)", 2.5 * PI);
    pt(&mut s, "(vlax-curve-getPointAtParam e (* 2 pi))", [2.0, 0.0, 0.0]);
    near(&mut s, "(vlax-curve-getDistAtParam e (vlax-curve-getEndParam e))", 2.0 * PI);
    pt(&mut s, "(vlax-curve-getPointAtDist e pi)", [2.0, 0.0, 0.0]);
    near(&mut s, "(vlax-curve-getParamAtPoint e '(2 0))", 2.0 * PI);
}

/// A circle runs from 0 to 2π, starting at angle 0; its area is the disc.
#[test]
fn circle_parameters_and_area() {
    let mut s = session();
    let e = add(&mut s, EntityKind::Circle(Circle { center: Vec3::new(1.0, 1.0, 0.0), radius: 1.0 }));
    ok(&mut s, &format!("(setq e {e})"));
    near(&mut s, "(vlax-curve-getStartParam e)", 0.0);
    near(&mut s, "(vlax-curve-getEndParam e)", 2.0 * PI);
    pt(&mut s, "(vlax-curve-getStartPoint e)", [2.0, 1.0, 0.0]);
    pt(&mut s, "(vlax-curve-getEndPoint e)", [2.0, 1.0, 0.0]);
    near(&mut s, "(vlax-curve-getDistAtParam e pi)", PI);
    pt(&mut s, "(vlax-curve-getPointAtDist e pi)", [0.0, 1.0, 0.0]);
    near(&mut s, "(vlax-curve-getParamAtPoint e '(1 2))", PI / 2.0);
    assert_eq!(ok(&mut s, "(vlax-curve-isClosed e)"), "T");
    near(&mut s, "(vlax-curve-getArea e)", PI);
}

/// A polyline's parameter is the vertex number; a bulge segment counts one like a straight one.
/// (0,0) -> (10,0) straight, then bulge 1 to (10,10): a half circle of radius 5 about (10,5),
/// counter-clockwise through (15,5).
#[test]
fn polyline_with_a_bulge() {
    let mut s = session();
    let e = pline(&mut s, &[([0.0, 0.0], 0.0), ([10.0, 0.0], 1.0), ([10.0, 10.0], 0.0)], false);
    ok(&mut s, &format!("(setq e {e})"));
    let half = 5.0 * PI;
    near(&mut s, "(vlax-curve-getStartParam e)", 0.0);
    near(&mut s, "(vlax-curve-getEndParam e)", 2.0);
    pt(&mut s, "(vlax-curve-getStartPoint e)", [0.0, 0.0, 0.0]);
    pt(&mut s, "(vlax-curve-getEndPoint e)", [10.0, 10.0, 0.0]);
    near(&mut s, "(vlax-curve-getDistAtParam e 1)", 10.0);
    near(&mut s, "(vlax-curve-getDistAtParam e 1.5)", 10.0 + half / 2.0);
    near(&mut s, "(vlax-curve-getDistAtParam e (vlax-curve-getEndParam e))", 10.0 + half);
    pt(&mut s, "(vlax-curve-getPointAtParam e 1.5)", [15.0, 5.0, 0.0]);
    pt(&mut s, "(vlax-curve-getPointAtParam e 0.25)", [2.5, 0.0, 0.0]);
    pt(&mut s, "(vlax-curve-getPointAtDist e 5)", [5.0, 0.0, 0.0]);
    pt(&mut s, &format!("(vlax-curve-getPointAtDist e {})", 10.0 + half / 2.0), [15.0, 5.0, 0.0]);
    near(&mut s, "(vlax-curve-getParamAtPoint e '(15 5))", 1.5);
    near(&mut s, &format!("(vlax-curve-getParamAtDist e {})", 10.0 + half / 4.0), 1.25);
    near(&mut s, "(vlax-curve-getDistAtPoint e '(15 5))", 10.0 + half / 2.0);
    // At the half circle's start, (10,0), it heads +x (counter-clockwise about (10,5)); the
    // derivative is per unit of parameter, so its length is the segment's, 5π.
    pt(&mut s, "(vlax-curve-getFirstDeriv e 1)", [half, 0.0, 0.0]);
    assert_eq!(ok(&mut s, "(vlax-curve-isClosed e)"), "nil");
    // As if closed: the triangle (50) plus the half disc bulging outside it (25π/2).
    near(&mut s, "(vlax-curve-getArea e)", 50.0 + 12.5 * PI);
    assert_eq!(ok(&mut s, "(vlax-curve-getPointAtParam e 2.5)"), "nil");
}

/// A negative bulge turns clockwise: (0,0) -> (10,0) with bulge -1 passes over (5,5).
#[test]
fn polyline_with_a_negative_bulge() {
    let mut s = session();
    let e = pline(&mut s, &[([0.0, 0.0], -1.0), ([10.0, 0.0], 0.0)], false);
    ok(&mut s, &format!("(setq e {e})"));
    near(&mut s, "(vlax-curve-getEndParam e)", 1.0);
    pt(&mut s, "(vlax-curve-getPointAtParam e 0.5)", [5.0, 5.0, 0.0]);
    pt(&mut s, "(vlax-curve-getPointAtParam e 0.25)", [5.0 - 5.0 / 2f64.sqrt(), 5.0 / 2f64.sqrt(), 0.0]);
    near(&mut s, "(vlax-curve-getDistAtParam e 1)", 5.0 * PI);
    near(&mut s, "(vlax-curve-getParamAtPoint e '(5 5))", 0.5);
    near(&mut s, "(vlax-curve-getArea e)", 12.5 * PI);
    // Positive bulge on the same chord goes under it, through (5,-5).
    let f = pline(&mut s, &[([0.0, 0.0], 1.0), ([10.0, 0.0], 0.0)], false);
    pt(&mut s, &format!("(vlax-curve-getPointAtParam {f} 0.5)"), [5.0, -5.0, 0.0]);
}

/// A closed polyline adds its closing segment (one more parameter) and is closed.
#[test]
fn closed_polyline_square_with_a_quarter_bulge() {
    let mut s = session();
    let e = pline(&mut s, &[([0.0, 0.0], 0.0), ([10.0, 0.0], 0.0), ([10.0, 10.0], 0.0), ([0.0, 10.0], 0.0)], true);
    ok(&mut s, &format!("(setq e {e})"));
    near(&mut s, "(vlax-curve-getEndParam e)", 4.0);
    near(&mut s, "(vlax-curve-getDistAtParam e 4)", 40.0);
    pt(&mut s, "(vlax-curve-getPointAtParam e 3.5)", [0.0, 5.0, 0.0]);
    pt(&mut s, "(vlax-curve-getEndPoint e)", [0.0, 0.0, 0.0]);
    assert_eq!(ok(&mut s, "(vlax-curve-isClosed e)"), "T");
    near(&mut s, "(vlax-curve-getArea e)", 100.0);
    // The triangle of the bulge test, closed: length adds the 10√2 diagonal.
    let t = pline(&mut s, &[([0.0, 0.0], 0.0), ([10.0, 0.0], 1.0), ([10.0, 10.0], 0.0)], true);
    near(&mut s, &format!("(vlax-curve-getEndParam {t})"), 3.0);
    near(&mut s, &format!("(vlax-curve-getDistAtParam {t} 3)"), 10.0 + 5.0 * PI + 200f64.sqrt());
    assert_eq!(ok(&mut s, &format!("(vlax-curve-isClosed {t})")), "T");
}

/// An open polyline whose ends meet is closed too.
#[test]
fn polyline_ending_where_it_starts_is_closed() {
    let mut s = session();
    let e = pline(&mut s, &[([0.0, 0.0], 0.0), ([4.0, 0.0], 0.0), ([4.0, 3.0], 0.0), ([0.0, 0.0], 0.0)], false);
    assert_eq!(ok(&mut s, &format!("(vlax-curve-isClosed {e})")), "T");
    near(&mut s, &format!("(vlax-curve-getArea {e})"), 6.0);
}

/// Something that isn't a curve, or isn't an entity, is an error, never a crash.
#[test]
fn curves_refuse_what_they_cannot_measure() {
    let mut s = session();
    let t = add(&mut s, EntityKind::Text(Text { value: "x".into(), ..text() }));
    assert!(eval(&mut s, &format!("(vlax-curve-getStartPoint {t})")).unwrap_err().contains("curvep"));
    assert!(eval(&mut s, "(vlax-curve-getStartPoint nil)").unwrap_err().contains("bad argument type"));
    assert!(eval(&mut s, "(vlax-curve-getEndParam 3)").is_err());
    let e = line(&mut s, [0.0, 0.0], [1.0, 0.0]);
    assert!(eval(&mut s, &format!("(vlax-curve-getDistAtParam {e} \"x\")")).is_err());
    assert_eq!(ok(&mut s, &format!("(vlax-curve-getPointAtParam {e} 1e400)")), "nil");
    assert!(eval(&mut s, &format!("(vlax-curve-getEndParam {e} 1)")).unwrap_err().contains("too many arguments"));
}

fn text() -> Text {
    let mut s = session();
    // A text entity as the drawing makes one, from entmake.
    ok(&mut s, r#"(entmake '((0 . "TEXT") (10 0.0 0.0 0.0) (40 . 1.0) (1 . "x")))"#);
    let d = s.doc().unwrap();
    match &d.model.last().unwrap().kind {
        EntityKind::Text(t) => t.clone(),
        other => panic!("not text: {other:?}"),
    }
}

// ---------------- objects and properties ----------------

const DOC: &str = "(setq doc (vla-get-ActiveDocument (vlax-get-acad-object)))";

#[test]
fn ename_and_object_convert_both_ways() {
    let mut s = session();
    let e = line(&mut s, [0.0, 0.0], [3.0, 4.0]);
    ok(&mut s, &format!("(setq e {e} o (vlax-ename->vla-object e))"));
    assert_eq!(ok(&mut s, "(type o)"), "VLA-OBJECT");
    assert!(ok(&mut s, "(vl-prin1-to-string o)").starts_with("\"#<VLA-OBJECT IAcadLine "));
    assert_eq!(ok(&mut s, "(equal e (vlax-vla-object->ename o))"), "T");
    assert_eq!(ok(&mut s, "(equal o (vlax-ename->vla-object e))"), "T", "the same entity is the same object");
    assert_eq!(ok(&mut s, "(vla-get-ObjectName o)"), "\"AcDbLine\"");
    assert!(eval(&mut s, "(vlax-ename->vla-object nil)").unwrap_err().contains("lentityp"));
    assert!(eval(&mut s, "(vla-get-Layer nil)").unwrap_err().contains("VLA-OBJECT"));
}

#[test]
fn layer_and_color_go_to_the_entity() {
    let mut s = session();
    let e = line(&mut s, [0.0, 0.0], [3.0, 4.0]);
    ok(&mut s, DOC);
    ok(&mut s, &format!("(setq e {e} o (vlax-ename->vla-object e))"));
    assert_eq!(ok(&mut s, "(vla-get-Layer o)"), "\"0\"");
    // vla-add makes the layer and gives its object.
    assert_eq!(ok(&mut s, "(vla-get-Name (setq lay (vla-add (vla-get-Layers doc) \"WALL\")))"), "\"WALL\"");
    assert_eq!(ok(&mut s, "(vla-put-Color lay 3)"), "nil");
    assert_eq!(ok(&mut s, "(vla-get-Color lay)"), "3");
    assert_eq!(ok(&mut s, "(vla-put-Layer o \"WALL\")"), "nil");
    assert_eq!(ok(&mut s, "(cdr (assoc 8 (entget e)))"), "\"WALL\"");
    ok(&mut s, "(vla-put-Color o 1)");
    assert_eq!(ok(&mut s, "(cdr (assoc 62 (entget e)))"), "1");
    assert_eq!(ok(&mut s, "(vla-get-Color o)"), "1");
    ok(&mut s, "(vla-put-Color o 256)");
    assert_eq!(ok(&mut s, "(vla-get-Color o)"), "256", "ByLayer");
    ok(&mut s, "(vla-put-Color o acByBlock)");
    assert_eq!(ok(&mut s, "(vla-get-Color o)"), "0", "ByBlock");
    ok(&mut s, "(vla-put-Linetype o \"byblock\")");
    assert_eq!(ok(&mut s, "(vla-get-Linetype o)"), "\"ByBlock\"");
    ok(&mut s, "(vla-put-Lineweight o -2)");
    assert_eq!(ok(&mut s, "(vla-get-Lineweight o)"), "-2");
    ok(&mut s, "(vla-put-PlotStyleName o \"ByBlock\")");
    assert_eq!(ok(&mut s, "(vla-get-PlotStyleName o)"), "\"ByBlock\"");
    // A layer that doesn't exist, a colour out of range: automation errors a routine can catch.
    assert_eq!(eval(&mut s, "(vla-put-Layer o \"NOPE\")"), Err("Automation Error. Key not found".into()));
    assert_eq!(ok(&mut s, "(vl-catch-all-error-p (vl-catch-all-apply 'vla-put-Color (list o 300)))"), "T");
    assert_eq!(ok(&mut s, "(vla-get-Color o)"), "0", "a refused colour changes nothing");
    // A line has no block name.
    assert_eq!(eval(&mut s, "(vla-get-Name o)"), Err("ActiveX Server returned the error: unknown name: NAME".into()));
    // vlax-put / vlax-get-property by name.
    ok(&mut s, "(vlax-put-property o 'Color 5)");
    assert_eq!(ok(&mut s, "(vlax-get-property o \"color\")"), "5");
    ok(&mut s, "(vlax-put o 'Layer \"0\")");
    assert_eq!(ok(&mut s, "(vlax-get o 'Layer)"), "\"0\"");
}

#[test]
fn a_locked_layer_refuses_changes() {
    let mut s = session();
    let e = line(&mut s, [0.0, 0.0], [3.0, 4.0]);
    ok(&mut s, DOC);
    ok(&mut s, &format!("(setq o (vlax-ename->vla-object {e}) lay (vla-item (vla-get-Layers doc) \"0\"))"));
    assert_eq!(ok(&mut s, "(vla-get-Lock lay)"), ":VLAX-FALSE");
    assert_eq!(ok(&mut s, "(vlax-write-enabled-p o)"), "T");
    ok(&mut s, "(vla-put-Lock lay :vlax-true)");
    assert_eq!(ok(&mut s, "(vla-get-Lock lay)"), ":VLAX-TRUE");
    assert_eq!(ok(&mut s, "(= :vlax-true (vla-get-Lock lay))"), "T");
    assert!(s.doc().unwrap().layer("0").unwrap().locked);
    assert_eq!(ok(&mut s, "(vlax-write-enabled-p o)"), "nil");
    assert_eq!(eval(&mut s, "(vla-put-Color o 2)"), Err("Automation Error. On locked layer".into()));
    assert!(eval(&mut s, "(vlax-invoke o 'Move '(0 0) '(1 1))").is_err());
    ok(&mut s, "(vla-put-Lock lay :vlax-false)");
    assert_eq!(ok(&mut s, "(vla-put-Color o 2)"), "nil");
    ok(&mut s, "(vla-put-Lock lay T)");
    assert_eq!(ok(&mut s, "(vla-get-Lock lay)"), ":VLAX-TRUE", "T locks too");
    ok(&mut s, "(vla-put-Lock lay nil)");
    assert_eq!(ok(&mut s, "(vla-get-Lock lay)"), ":VLAX-FALSE");
}

#[test]
fn bounding_box_through_symbols() {
    let mut s = session();
    let e = line(&mut s, [1.0, 2.0], [4.0, 6.0]);
    ok(&mut s, &format!("(setq o (vlax-ename->vla-object {e}))"));
    // vla-GetBoundingBox sets the symbols to safearrays.
    assert_eq!(ok(&mut s, "(vla-GetBoundingBox o 'mn 'mx)"), "nil");
    assert_eq!(ok(&mut s, "(type mn)"), "SAFEARRAY");
    assert_eq!(ok(&mut s, "(vlax-safearray->list mn)"), "(1.0 2.0 0.0)");
    assert_eq!(ok(&mut s, "(vlax-safearray->list mx)"), "(4.0 6.0 0.0)");
    // vlax-invoke sets them to lists.
    ok(&mut s, "(vlax-invoke o 'GetBoundingBox 'a 'b)");
    assert_eq!(ok(&mut s, "(list a b)"), "((1.0 2.0 0.0) (4.0 6.0 0.0))");
    // The k_nb.lsp idiom: vlax-invoke-method with symbols holding safearrays.
    ok(&mut s, "(setq lo (vlax-make-safearray vlax-vbDouble '(0 . 2)) hi (vlax-make-safearray vlax-vbDouble '(0 . 2)))");
    ok(&mut s, "(vl-catch-all-apply 'vlax-invoke-method (list o 'GetBoundingBox 'lo 'hi))");
    assert_eq!(ok(&mut s, "(vlax-safearray->list hi)"), "(4.0 6.0 0.0)");
    // A local symbol (a defun's) gets the value as setq would.
    ok(&mut s, "(defun bb (ob / p q) (vla-getboundingbox ob 'p 'q) (list (vlax-safearray->list p) (vlax-safearray->list q)))");
    assert_eq!(ok(&mut s, "(bb o)"), "((1.0 2.0 0.0) (4.0 6.0 0.0))");
    assert!(eval(&mut s, "(vla-GetBoundingBox o nil 'q)").unwrap_err().contains("symbolp"));
}

#[test]
fn safearrays_and_points() {
    let mut s = session();
    ok(&mut s, "(setq sa (vlax-make-safearray vlax-vbDouble '(0 . 2)))");
    assert_eq!(ok(&mut s, "(vlax-safearray->list sa)"), "(0.0 0.0 0.0)");
    ok(&mut s, "(vlax-safearray-fill sa '(1 2 3))");
    assert_eq!(ok(&mut s, "(vlax-safearray->list sa)"), "(1.0 2.0 3.0)");
    assert_eq!(ok(&mut s, "(vlax-safearray-get-element sa 1)"), "2.0");
    ok(&mut s, "(vlax-safearray-put-element sa 2 9)");
    assert_eq!(ok(&mut s, "(vlax-safearray->list sa)"), "(1.0 2.0 9.0)");
    assert_eq!(ok(&mut s, "(list (vlax-safearray-get-l-bound sa 1) (vlax-safearray-get-u-bound sa 1) (vlax-safearray-type sa))"), "(0 2 5)");
    assert!(eval(&mut s, "(vlax-safearray-get-element sa 3)").is_err());
    assert!(eval(&mut s, "(vlax-safearray-fill sa '(1 2 3 4))").is_err());
    // Strings, a lower bound of 1.
    ok(&mut s, "(setq st (vlax-make-safearray vlax-vbString '(1 . 2)))");
    ok(&mut s, "(vlax-safearray-put-element st 1 \"a\")");
    assert_eq!(ok(&mut s, "(vlax-safearray->list st)"), "(\"a\" \"\")");
    assert_eq!(ok(&mut s, "(vlax-safearray->list (vlax-3d-point 1 2))"), "(1.0 2.0 0.0)");
    assert_eq!(ok(&mut s, "(vlax-safearray->list (vlax-3d-point '(4 5 6)))"), "(4.0 5.0 6.0)");
    assert_eq!(ok(&mut s, "(vlax-variant-type (vlax-3d-point 1 2))"), "8197");
    assert_eq!(ok(&mut s, "(vlax-variant-value (vlax-make-variant 3 vlax-vbDouble))"), "3.0");
    // Hostile bounds are refused, not allocated.
    assert!(eval(&mut s, "(vlax-make-safearray vlax-vbDouble '(0 . 2000000000))").is_err());
    assert!(eval(&mut s, "(vlax-make-safearray vlax-vbDouble '(5 . 1))").is_err());
    assert!(eval(&mut s, "(vlax-safearray->list 3)").unwrap_err().contains("safearrayp"));
}

#[test]
fn move_copy_delete_and_intersect() {
    let mut s = session();
    let a = line(&mut s, [0.0, 0.0], [10.0, 10.0]);
    let b = line(&mut s, [0.0, 10.0], [10.0, 0.0]);
    let c = line(&mut s, [20.0, 0.0], [20.0, 1.0]);
    let v = line(&mut s, [5.0, 0.0], [5.0, 1.0]);
    ok(
        &mut s,
        &format!(
            "(setq a (vlax-ename->vla-object {a}) b (vlax-ename->vla-object {b}) c (vlax-ename->vla-object {c}) v (vlax-ename->vla-object {v}))"
        ),
    );
    pt(&mut s, "(vlax-invoke a 'IntersectWith b acExtendNone)", [5.0, 5.0, 0.0]);
    assert_eq!(ok(&mut s, "(vlax-safearray->list (vla-IntersectWith a b 0))"), "(5.0 5.0 0.0)");
    assert_eq!(ok(&mut s, "(vlax-invoke a 'IntersectWith c acExtendNone)"), "nil");
    // Only both extended meet at (20,20), past both ends.
    pt(&mut s, "(vlax-invoke c 'IntersectWith a acExtendBoth)", [20.0, 20.0, 0.0]);
    assert_eq!(ok(&mut s, "(vlax-invoke c 'IntersectWith a acExtendThisEntity)"), "nil");
    assert_eq!(ok(&mut s, "(vlax-invoke c 'IntersectWith a acExtendOtherEntity)"), "nil");
    // The short line at x=5 extended up reaches the diagonal at (5,5); the diagonal extended
    // doesn't reach the short line.
    pt(&mut s, "(vlax-invoke v 'IntersectWith a acExtendThisEntity)", [5.0, 5.0, 0.0]);
    assert_eq!(ok(&mut s, "(vlax-invoke v 'IntersectWith a acExtendOtherEntity)"), "nil");
    pt(&mut s, "(vlax-invoke a 'IntersectWith v acExtendOtherEntity)", [5.0, 5.0, 0.0]);
    ok(&mut s, "(vlax-invoke a 'Move '(0 0 0) '(1 2 0))");
    pt(&mut s, "(vlax-curve-getStartPoint a)", [1.0, 2.0, 0.0]);
    pt(&mut s, "(vlax-get a 'EndPoint)", [11.0, 12.0, 0.0]);
    ok(&mut s, "(vla-Move a (vlax-3d-point 1 2 0) (vlax-3d-point 0 0 0))");
    pt(&mut s, "(vlax-safearray->list (vla-get-StartPoint a))", [0.0, 0.0, 0.0]);
    let before = s.doc().unwrap().model.len();
    ok(&mut s, "(setq d (vla-Copy a))");
    assert_eq!(s.doc().unwrap().model.len(), before + 1);
    assert_eq!(ok(&mut s, "(equal d a)"), "nil");
    ok(&mut s, "(vla-Delete d)");
    assert_eq!(s.doc().unwrap().model.len(), before);
    assert_eq!(ok(&mut s, "(vlax-erased-p d)"), "T");
    assert!(eval(&mut s, "(vla-get-Layer d)").unwrap_err().contains("erased"));
    // A property through vlax-invoke.
    assert_eq!(ok(&mut s, "(vlax-invoke a 'Layer)"), "\"0\"");
    assert_eq!(ok(&mut s, "(vlax-method-applicable-p a 'Move)"), "T");
    assert_eq!(ok(&mut s, "(vlax-method-applicable-p a 'Explode)"), "nil");
    assert!(eval(&mut s, "(vlax-invoke a 'Explode 1 2)").unwrap_err().contains("unknown name"));
}

/// Blocks: a reference's name and effective name, iterating the definition's objects, changing
/// them (SETPLOTBK's layer 0 / ByBlock idiom), `IsXRef`, and `vlax-for` over layers.
#[test]
fn blocks_and_collections() {
    let mut s = session();
    ok(&mut s, r#"(entmake '((0 . "BLOCK") (2 . "B1") (70 . 0) (10 0.0 0.0 0.0)))"#);
    ok(&mut s, r#"(entmake '((0 . "LINE") (8 . "PARTS") (62 . 1) (10 0.0 0.0 0.0) (11 1.0 0.0 0.0)))"#);
    ok(&mut s, r#"(entmake '((0 . "CIRCLE") (8 . "PARTS") (10 0.0 0.0 0.0) (40 . 0.5)))"#);
    ok(&mut s, r#"(entmake '((0 . "ENDBLK")))"#);
    ok(&mut s, r#"(entmake '((0 . "INSERT") (2 . "B1") (10 5.0 5.0 0.0)))"#);
    ok(&mut s, DOC);
    ok(&mut s, "(setq r (vlax-ename->vla-object (entlast)))");
    assert_eq!(ok(&mut s, "(vla-get-ObjectName r)"), "\"AcDbBlockReference\"");
    assert_eq!(ok(&mut s, "(vla-get-Name r)"), "\"B1\"");
    assert_eq!(ok(&mut s, "(vla-get-EffectiveName r)"), "\"B1\"");
    assert_eq!(ok(&mut s, "(vlax-property-available-p r 'EffectiveName)"), "T");
    assert_eq!(ok(&mut s, "(vlax-property-available-p r 'EffectiveName T)"), "nil", "read-only");
    assert_eq!(ok(&mut s, "(vlax-property-available-p r 'Layer T)"), "T");
    assert_eq!(ok(&mut s, "(vlax-property-available-p (vlax-ename->vla-object (entlast)) 'Radius)"), "nil");
    assert_eq!(ok(&mut s, "(vlax-safearray->list (vla-get-InsertionPoint r))"), "(5.0 5.0 0.0)");
    // The definition: its objects, changed in place.
    ok(&mut s, "(setq blk (vla-item (vla-get-Blocks doc) \"b1\"))");
    assert_eq!(ok(&mut s, "(vla-get-Name blk)"), "\"B1\"");
    assert_eq!(ok(&mut s, "(vla-get-IsXRef blk)"), ":VLAX-FALSE");
    assert_eq!(ok(&mut s, "(vla-get-Count blk)"), "2");
    assert_eq!(
        ok(&mut s, "(setq names nil) (vlax-for x blk (setq names (cons (vla-get-ObjectName x) names))) (reverse names)"),
        "(\"AcDbLine\" \"AcDbCircle\")"
    );
    ok(&mut s, "(vlax-for x blk (vla-put-Layer x \"0\") (vla-put-Color x 0))");
    let d = s.doc().unwrap();
    for e in d.block("B1").unwrap().entities.iter() {
        assert_eq!(e.common.layer, "0");
        assert_eq!(e.common.color, cadcraft_color::Color::ByBlock);
    }
    assert_eq!(ok(&mut s, "(vlax-for x blk (setq l (vla-get-Layer x))) l"), "\"0\"");
    // The blocks collection starts with model space, which is a layout and no xref.
    assert_eq!(ok(&mut s, "(vla-get-Name (vla-item (vla-get-Blocks doc) 0))"), "\"*Model_Space\"");
    assert_eq!(ok(&mut s, "(setq n 0) (vlax-for b (vla-get-Blocks doc) (if (= :vlax-false (vla-get-IsXRef b)) (setq n (1+ n)))) n"), "2");
    assert_eq!(ok(&mut s, "(setq k 0) (vlax-for o (vla-get-ModelSpace doc) (setq k (1+ k))) k"), "1");
    assert_eq!(ok(&mut s, "(vla-get-Count (vla-get-ModelSpace doc))"), "1");
    // Layers.
    assert_eq!(ok(&mut s, "(setq ls nil) (vlax-for l (vla-get-Layers doc) (setq ls (cons (vla-get-Name l) ls))) (vl-position \"PARTS\" ls)"), "0");
    assert_eq!(ok(&mut s, "(vla-get-Name (vla-item (vla-get-Layers doc) \"parts\"))"), "\"PARTS\"");
    assert_eq!(eval(&mut s, "(vla-item (vla-get-Layers doc) \"NOPE\")"), Err("Automation Error. Key not found".into()));
    assert!(eval(&mut s, "(vlax-for x 5 (print x))").is_err());
    assert!(eval(&mut s, "(vlax-for x r (print x))").unwrap_err().contains("collection"));
    // vlax-vla-object->ename of a definition's object; refgeom-style code reads it back.
    assert_eq!(ok(&mut s, "(vlax-for x blk (setq en (vlax-vla-object->ename x))) (type en)"), "ENAME");
}

/// The application's environment objects are there and answer without changing anything:
/// HY-Key.lsp's support path, BlockNBase.lsp's undo marks and regen.
#[test]
fn environment_objects() {
    let mut s = session();
    ok(&mut s, "(setq acad (vlax-get-acad-object) doc (vla-get-ActiveDocument acad))");
    assert_eq!(ok(&mut s, "(type acad)"), "VLA-OBJECT");
    ok(&mut s, "(setq files (vla-get-Files (vla-get-Preferences acad)))");
    assert_eq!(ok(&mut s, "(type (vla-get-SupportPath files))"), "STR");
    assert_eq!(ok(&mut s, "(vla-put-SupportPath files \"C:\\\\HY\")"), "nil");
    assert_eq!(ok(&mut s, "(type (vla-get-Profiles (vla-get-Preferences acad)))"), "VLA-OBJECT");
    assert_eq!(ok(&mut s, "(vla-get-ActiveProfile (vla-get-Profiles (vla-get-Preferences acad)))"), "\"<<Unnamed Profile>>\"");
    assert_eq!(ok(&mut s, "(list (vla-StartUndoMark doc) (vla-EndUndoMark doc) (vla-Regen doc acAllViewports))"), "(nil nil nil)");
    assert_eq!(ok(&mut s, "(type (vla-get-ModelSpace doc))"), "VLA-OBJECT");
    assert_eq!(ok(&mut s, "(vla-get-Name (vla-get-ActiveLayer doc))"), "\"0\"");
    ok(&mut s, "(vla-add (vla-get-Layers doc) \"NEW\")");
    ok(&mut s, "(vla-put-ActiveLayer doc (vla-item (vla-get-Layers doc) \"NEW\"))");
    assert_eq!(ok(&mut s, "(getvar \"CLAYER\")"), "\"NEW\"");
    // LM:acdoc's idiom: an object as a constant in a function body.
    ok(&mut s, "(eval (list 'defun 'LM:acdoc 'nil (vla-get-activedocument (vlax-get-acad-object))))");
    assert_eq!(ok(&mut s, "(equal (LM:acdoc) doc)"), "T");
    assert_eq!(ok(&mut s, "(vlax-write-enabled-p doc)"), "T");
    assert_eq!(ok(&mut s, "(vlax-property-available-p doc 'ModelSpace)"), "T");
    assert_eq!(ok(&mut s, "(vlax-property-available-p doc 'Bogus)"), "nil");
    assert_eq!(ok(&mut s, "(vlax-release-object doc)"), "nil");
    assert_eq!(ok(&mut s, "(vla-get-Name (vla-add (vla-get-Blocks doc) (vlax-3d-point 0 0) \"EMPTY\"))"), "\"EMPTY\"");
    assert!(s.doc().unwrap().block("EMPTY").is_some());
}

/// A run's changes through objects are one undo step, like the rest of a routine's.
#[test]
fn object_changes_undo_with_the_routine() {
    let mut s = session();
    let e = line(&mut s, [0.0, 0.0], [1.0, 0.0]);
    ok(&mut s, &format!("(vla-put-Color (vlax-ename->vla-object {e}) 4)"));
    let h = s.doc().unwrap().model.last().unwrap().handle;
    assert_eq!(s.doc().unwrap().entity(h).unwrap().common.color, cadcraft_color::Color::Index(4));
    s.undo().unwrap();
    assert_eq!(s.doc().unwrap().entity(h).unwrap().common.color, cadcraft_color::Color::ByLayer);
}

/// `getvar` and `setvar` take a symbol for the name too (Lee Mac's `(setvar 'cmdecho 0)`, used by
/// BlockNBase.lsp), as AutoLISP does.
#[test]
fn sysvars_by_symbol() {
    let mut s = session();
    ok(&mut s, "(setvar 'cmdecho 0)");
    assert_eq!(ok(&mut s, "(getvar 'cmdecho)"), "0");
    assert_eq!(ok(&mut s, "(getvar \"CMDECHO\")"), "0");
    assert!(eval(&mut s, "(getvar 1)").unwrap_err().contains("stringp"));
}

/// `entlast` and `entnext` see every entity a run makes, not only the first (the order cache was
/// keyed on the drawing's address, which stays the same when a run changes it in place).
#[test]
fn entlast_follows_every_entmake_in_one_run() {
    let mut s = session();
    let v = ok(
        &mut s,
        r#"(entmake '((0 . "LINE") (10 0.0 0.0 0.0) (11 1.0 0.0 0.0))) (setq a (entlast))
           (entmake '((0 . "LINE") (10 0.0 1.0 0.0) (11 1.0 1.0 0.0))) (setq b (entlast))
           (entmake '((0 . "LINE") (10 0.0 2.0 0.0) (11 1.0 2.0 0.0))) (setq c (entlast))
           (list (equal a b) (equal b c) (equal (entnext a) b) (equal (entnext b) c) (entnext c))"#,
    );
    assert_eq!(v, "(nil nil T T nil)");
}

/// `entmakex` makes what `entmake` does and answers the new entity's name (k_area.lsp).
#[test]
fn entmakex_answers_the_new_name() {
    let mut s = session();
    let v = ok(
        &mut s,
        r#"(setq e (entmakex '((0 . "TEXT") (10 1.0 2.0 0.0) (40 . 2.5) (1 . "12.5")))) (list (type e) (equal e (entlast)) (cdr (assoc 1 (entget e))))"#,
    );
    assert_eq!(v, "(ENAME T \"12.5\")");
    assert_eq!(ok(&mut s, r#"(entmakex '((0 . "NOSUCHTHING")))"#), "nil");
}

/// Moving along Z lifts the object; a 3D line's bounding box spans its lowest and highest z.
#[test]
fn move_and_bounding_box_in_z() {
    let mut s = session();
    let e = add(&mut s, EntityKind::Line(Line { a: Vec3::new(0.0, 0.0, 0.0), b: Vec3::new(10.0, 10.0, 5.0) }));
    ok(&mut s, &format!("(setq o (vlax-ename->vla-object {e}))"));
    ok(&mut s, "(vla-GetBoundingBox o 'mn 'mx)");
    assert_eq!(ok(&mut s, "(list (vlax-safearray->list mn) (vlax-safearray->list mx))"), "((0.0 0.0 0.0) (10.0 10.0 5.0))");
    ok(&mut s, "(vlax-invoke o 'Move '(0 0 0) '(1 0 2))");
    assert_eq!(ok(&mut s, "(list (vlax-curve-getStartPoint o) (vlax-curve-getEndPoint o))"), "((1.0 0.0 2.0) (11.0 10.0 7.0))");
    let c = add(&mut s, EntityKind::Circle(Circle { center: Vec3::new(0.0, 0.0, 0.0), radius: 1.0 }));
    ok(&mut s, &format!("(vla-Move (vlax-ename->vla-object {c}) (vlax-3d-point 0 0 0) (vlax-3d-point 0 0 3))"));
    assert_eq!(ok(&mut s, &format!("(vlax-curve-getStartPoint {c})")), "(1.0 0.0 3.0)");
}

/// A chain of arrays holding lists holding arrays, a hundred thousand deep, drops without
/// overflowing the stack.
#[test]
fn a_deep_chain_of_safearrays_drops() {
    let _heavy = super::value::HEAVY_CELLS.lock().unwrap_or_else(|e| e.into_inner());
    let mut s = session();
    let code = "(setq a nil) (repeat 100000 (setq x (vlax-make-safearray vlax-vbVariant '(0 . 0))) \
                (vlax-safearray-put-element x 0 (list a)) (setq a x)) (setq a nil x nil) 'done";
    assert_eq!(ok(&mut s, code), "DONE");
}

/// Large safearrays a routine keeps count against its memory budget: it stops with an error
/// instead of taking all memory.
#[test]
fn safearrays_count_against_the_memory_budget() {
    let mut s = session();
    s.lisp.limits.string_bytes = 64 << 20;
    let code = "(setq keep nil) (repeat 1000 (setq keep (cons (vlax-make-safearray vlax-vbDouble '(0 . 999999)) keep))) 'kept";
    let e = eval(&mut s, code).unwrap_err();
    assert!(e.contains("out of memory"), "{e}");
}

/// An object found in a block definition converts to an entity name and back, and `entget` and
/// the curve functions read that name.
#[test]
fn block_definition_objects_round_trip_through_entity_names() {
    let mut s = session();
    ok(&mut s, r#"(entmake '((0 . "BLOCK") (2 . "RT") (70 . 0) (10 0.0 0.0 0.0)))"#);
    ok(&mut s, r#"(entmake '((0 . "LINE") (8 . "0") (10 0.0 0.0 0.0) (11 3.0 4.0 0.0)))"#);
    ok(&mut s, r#"(entmake '((0 . "ENDBLK")))"#);
    ok(
        &mut s,
        "(vlax-for x (vla-item (vla-get-Blocks (vla-get-ActiveDocument (vlax-get-acad-object))) \"RT\") (setq o x en (vlax-vla-object->ename x)))",
    );
    assert_eq!(ok(&mut s, "(equal o (vlax-ename->vla-object en))"), "T", "the object keeps its block");
    assert_eq!(ok(&mut s, "(cdr (assoc 0 (entget en)))"), "\"LINE\"");
    assert_eq!(ok(&mut s, "(assoc 410 (entget en))"), "nil", "a block's object has no layout");
    assert_eq!(ok(&mut s, "(cdr (assoc 11 (entget en)))"), "(3.0 4.0 0.0)");
    assert_eq!(ok(&mut s, "(vlax-curve-getEndParam en)"), "5.0");
    ok(&mut s, "(vla-put-Color (vlax-ename->vla-object en) 3)");
    assert_eq!(ok(&mut s, "(cdr (assoc 62 (entget en)))"), "3");
}

/// Lines crossing in plan but at different heights don't intersect; a sloped line meets a flat
/// one at the height where it crosses.
#[test]
fn intersections_are_checked_in_z() {
    let mut s = session();
    let flat = add(&mut s, EntityKind::Line(Line { a: Vec3::new(0.0, 10.0, 5.0), b: Vec3::new(10.0, 0.0, 5.0) }));
    let high = add(&mut s, EntityKind::Line(Line { a: Vec3::new(0.0, 0.0, 10.0), b: Vec3::new(10.0, 10.0, 10.0) }));
    let slope = add(&mut s, EntityKind::Line(Line { a: Vec3::new(0.0, 0.0, 0.0), b: Vec3::new(10.0, 10.0, 10.0) }));
    ok(&mut s, &format!("(setq f (vlax-ename->vla-object {flat}) h (vlax-ename->vla-object {high}) sl (vlax-ename->vla-object {slope}))"));
    assert_eq!(ok(&mut s, "(vlax-invoke f 'IntersectWith h acExtendNone)"), "nil");
    assert_eq!(ok(&mut s, "(vlax-invoke f 'IntersectWith sl acExtendNone)"), "(5.0 5.0 5.0)");
    assert_eq!(ok(&mut s, "(vlax-invoke sl 'IntersectWith f acExtendNone)"), "(5.0 5.0 5.0)");
}

/// Filling a string safearray shares the strings given (copies would escape the memory budget).
#[test]
fn string_safearrays_share_their_strings() {
    let mut s = session();
    ok(&mut s, "(setq str (strcat \"abc\" \"def\") sa (vlax-make-safearray vlax-vbString '(0 . 2)))");
    ok(&mut s, "(vlax-safearray-fill sa (list str str))");
    ok(&mut s, "(vlax-safearray-put-element sa 2 str)");
    let (Value::Str(want), Value::SafeArray(sa)) = (s.lisp_var("STR"), s.lisp_var("SA")) else { panic!("not set") };
    for v in sa.to_vec() {
        let Value::Str(have) = v else { panic!("not a string") };
        assert!(std::sync::Arc::ptr_eq(&have, &want));
    }
}
