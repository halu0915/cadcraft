//! Visual LISP's ActiveX functions (`vla-*`, `vlax-*`) over CADCraft's own drawing: the
//! application and document objects, the layer and block collections, drawing objects with their
//! common properties and methods, safearrays, and the `vlax-curve-*` measurements (`curve.rs`).
//!
//! There is no COM here. A `vla-object` ([`Vla`]) names what it stands for (an entity by handle,
//! a layer or block by name, a collection) and every property read or write goes to the drawing,
//! so an object stays in step with `entget`/`entmod` and the commands. What a routine can't do
//! here fails with an error a `vl-catch-all-apply` catches, as AutoCAD's automation errors do.
//!
//! Properties: `(vla-get-Layer obj)` / `(vla-put-Layer obj "WALL")`, or by name with
//! `vlax-get-property` / `vlax-put-property` (`vlax-get` / `vlax-put` give points as lists).
//! Methods: `(vla-GetBoundingBox obj 'mn 'mx)`, `(vlax-invoke obj 'Move from to)`… Points read as
//! safearrays through `vla-get-…` and as lists through `vlax-get` and `vlax-invoke`, as in
//! AutoCAD.

use std::collections::HashMap;
use std::sync::Arc;

use cadcraft_color::Color;
use cadcraft_doc::{Drawing, Entity, EntityKind, Handle, Lineweight};
use cadcraft_geom::{Arc as GArc, Mat3, Segment, Vec2, Vec3, intersect_ext};

use super::Env;
use super::LispError;
use super::builtins::args_err;
use super::curve;
use super::drawing::color_index;
use super::eval::{Ctx, Flow, Special};
use super::value::{Builtin, BuiltinKind, SafeArray, Sym, Value, Vla, to_text};

type F = Result<Flow, LispError>;

const VLAX_TRUE: &str = ":VLAX-TRUE";
const VLAX_FALSE: &str = ":VLAX-FALSE";
/// `vlax-vbDouble` and friends: safearray and variant element types.
const VB_INTEGER: i32 = 2;
const VB_LONG: i32 = 3;
const VB_SINGLE: i32 = 4;
const VB_DOUBLE: i32 = 5;
const VB_STRING: i32 = 8;
const VB_OBJECT: i32 = 9;
const VB_BOOLEAN: i32 = 11;
const VB_VARIANT: i32 = 12;
const VB_ARRAY: i32 = 8192;
/// Most elements one safearray may have (a hostile bound can't take all memory).
const MAX_SAFEARRAY: i64 = 1_000_000;

fn arity(a: &[Value], min: usize, max: usize) -> Result<(), LispError> {
    if a.len() < min {
        return Err(args_err(true));
    }
    if a.len() > max {
        return Err(args_err(false));
    }
    Ok(())
}

fn vbool(b: bool) -> Value {
    Value::sym(if b { VLAX_TRUE } else { VLAX_FALSE })
}

/// A boolean argument: nil, `:vlax-false` and 0 are false.
fn truthy(v: &Value) -> bool {
    match v {
        Value::Nil => false,
        Value::Sym(s) => s.name() != VLAX_FALSE,
        Value::Int(i) => *i != 0,
        Value::Real(r) => *r != 0.0,
        _ => true,
    }
}

fn automation(msg: &str) -> LispError {
    LispError::new(format!("Automation Error. {msg}"))
}

fn unknown_name(name: &str) -> LispError {
    LispError::new(format!("ActiveX Server returned the error: unknown name: {name}"))
}

fn obj(v: &Value) -> Result<&Vla, LispError> {
    match v {
        Value::Vla(o) => Ok(o),
        other => Err(LispError::new(format!("bad argument type: VLA-OBJECT {}", to_text(other, true)))),
    }
}

fn string(v: &Value) -> Result<String, LispError> {
    match v {
        Value::Str(s) => Ok(s.to_string()),
        other => Err(LispError::bad_arg("stringp", other)),
    }
}

fn number(v: &Value) -> Result<f64, LispError> {
    match v.as_number() {
        Some(n) if n.is_finite() => Ok(n),
        _ => Err(LispError::bad_arg("numberp", v)),
    }
}

/// A property or method name: a symbol or a string, upper case.
fn member_name(v: &Value) -> Result<String, LispError> {
    match v {
        Value::Sym(s) => Ok(s.name().to_string()),
        Value::Str(s) => Ok(s.to_ascii_uppercase()),
        other => Err(LispError::bad_arg("symbolp", other)),
    }
}

fn doc<'a>(ctx: &'a Ctx<'_>) -> Result<&'a Drawing, LispError> {
    ctx.s.doc().map_err(|e| LispError::new(e.to_string()))
}

fn doc_mut<'a>(ctx: &'a mut Ctx<'_>) -> Result<&'a mut Drawing, LispError> {
    ctx.s.doc_mut().map_err(|e| LispError::new(e.to_string()))
}

/// A point given as a list of two or three numbers or as a safearray of them.
pub(crate) fn point_value(v: &Value) -> Option<[f64; 3]> {
    match v {
        Value::SafeArray(sa) => Value::list(sa.to_vec()).as_point(),
        other => other.as_point(),
    }
    .filter(|p| p.iter().all(|c| c.is_finite()))
}

fn point_arg(v: &Value) -> Result<Vec3, LispError> {
    let p = point_value(v).ok_or_else(|| LispError::bad_arg("2D/3D point", v))?;
    Ok(Vec3::new(p[0], p[1], p[2]))
}

/// A point as `vla-get-…` returns it: a safearray of three doubles.
fn point_sa(p: Vec3) -> Value {
    SafeArray::value(VB_DOUBLE, 0, vec![Value::Real(p.x), Value::Real(p.y), Value::Real(p.z)])
}

fn doubles_sa(v: impl IntoIterator<Item = f64>) -> Value {
    SafeArray::value(VB_DOUBLE, 0, v.into_iter().map(Value::Real).collect())
}

/// A safearray as a list (`vlax-get`, `vlax-invoke`); anything else as it is.
fn listed(v: Value) -> Value {
    match v {
        Value::SafeArray(sa) => Value::list(sa.to_vec()),
        other => other,
    }
}

// ---------------- objects ----------------

/// The ActiveX interface an entity's object has.
pub(crate) fn iface(kind: &EntityKind) -> &'static str {
    match kind {
        EntityKind::Line(_) => "IAcadLine",
        EntityKind::Point(_) => "IAcadPoint",
        EntityKind::Circle(_) => "IAcadCircle",
        EntityKind::Arc(_) => "IAcadArc",
        EntityKind::Ellipse(_) => "IAcadEllipse",
        EntityKind::LwPolyline(_) => "IAcadLWPolyline",
        EntityKind::Polyline3d(_) => "IAcad3DPolyline",
        EntityKind::Spline(_) => "IAcadSpline",
        EntityKind::Text(_) => "IAcadText",
        EntityKind::MText(_) => "IAcadMText",
        EntityKind::AttDef(_) => "IAcadAttribute",
        EntityKind::Insert(_) => "IAcadBlockReference",
        EntityKind::Dimension(_) => "IAcadDimension",
        EntityKind::Leader(_) => "IAcadLeader",
        EntityKind::MLeader(_) => "IAcadMLeader",
        EntityKind::Hatch(_) => "IAcadHatch",
        EntityKind::Solid(_) => "IAcadSolid",
        EntityKind::Region(_) => "IAcadRegion",
        EntityKind::Viewport(_) => "IAcadPViewport",
        EntityKind::Table(_) => "IAcadTable",
        _ => "IAcadEntity",
    }
}

/// The `ObjectName` (the database class name) of an entity.
fn object_name(kind: &EntityKind) -> &'static str {
    match kind {
        EntityKind::Line(_) => "AcDbLine",
        EntityKind::Point(_) => "AcDbPoint",
        EntityKind::Circle(_) => "AcDbCircle",
        EntityKind::Arc(_) => "AcDbArc",
        EntityKind::Ellipse(_) => "AcDbEllipse",
        EntityKind::LwPolyline(_) => "AcDbPolyline",
        EntityKind::Polyline3d(_) => "AcDb3dPolyline",
        EntityKind::Spline(_) => "AcDbSpline",
        EntityKind::Helix(_) => "AcDbHelix",
        EntityKind::Ray(_) => "AcDbRay",
        EntityKind::XLine(_) => "AcDbXline",
        EntityKind::Text(_) => "AcDbText",
        EntityKind::MText(_) => "AcDbMText",
        EntityKind::AttDef(_) => "AcDbAttributeDefinition",
        EntityKind::Insert(_) => "AcDbBlockReference",
        EntityKind::Dimension(_) => "AcDbRotatedDimension",
        EntityKind::MLine(_) => "AcDbMline",
        EntityKind::Leader(_) => "AcDbLeader",
        EntityKind::MLeader(_) => "AcDbMLeader",
        EntityKind::Hatch(_) => "AcDbHatch",
        EntityKind::Solid(_) => "AcDbSolid",
        EntityKind::Trace(_) => "AcDbTrace",
        EntityKind::Face3d(_) => "AcDbFace",
        EntityKind::Region(_) => "AcDbRegion",
        EntityKind::Viewport(_) => "AcDbViewport",
        EntityKind::Image(_) => "AcDbRasterImage",
        EntityKind::Wipeout(_) => "AcDbWipeout",
        EntityKind::Table(_) => "AcDbTable",
        EntityKind::Tolerance(_) => "AcDbFcf",
        EntityKind::Unknown(_) => "AcDbEntity",
    }
}

/// The object of an entity: in a space (`block` `None`) or in the named block definition.
fn entity_value(e: &Entity, block: Option<&str>) -> Value {
    Value::Vla(Vla::Entity { handle: e.handle, block: block.map(Arc::from), iface: iface(&e.kind) })
}

/// A block definition's key in the drawing (block names are case-insensitive).
fn block_key(d: &Drawing, name: &str) -> Option<String> {
    if d.blocks.contains_key(name) {
        return Some(name.to_string());
    }
    d.blocks.keys().find(|k| k.eq_ignore_ascii_case(name)).cloned()
}

fn is_model_space(name: &str) -> bool {
    name.eq_ignore_ascii_case("*Model_Space")
}

/// The entity an object stands for, if it still exists.
pub(crate) fn entity_of(d: &Drawing, o: &Vla) -> Option<Arc<Entity>> {
    let Vla::Entity { handle, block, .. } = o else { return None };
    match block {
        None => d.entity(*handle).cloned(),
        Some(b) => d.block(b)?.entities.get(*handle).cloned(),
    }
}

/// Replace the entity an object stands for. `false` when it no longer exists.
fn replace_entity(d: &mut Drawing, o: &Vla, new: Entity) -> bool {
    let Vla::Entity { handle, block, .. } = o else { return false };
    match block {
        None => d.modify_entity(*handle, |e| *e = new).is_ok(),
        Some(b) => {
            let Some(key) = block_key(d, b) else { return false };
            let Some(def) = d.blocks.get_mut(&key) else { return false };
            Arc::make_mut(def).entities.modify(*handle, |e| *e = new)
        }
    }
}

fn layer_locked(d: &Drawing, e: &Entity) -> bool {
    d.layer(&e.common.layer).is_some_and(|l| l.locked)
}

/// The entity of an object, refused when it is on a locked layer (it can't be changed).
fn writable_entity(d: &Drawing, o: &Vla) -> Result<Arc<Entity>, LispError> {
    let e = entity_of(d, o).ok_or_else(|| automation("Object was erased"))?;
    if layer_locked(d, &e) {
        return Err(automation("On locked layer"));
    }
    Ok(e)
}

/// A block reference's block name as the user knows it: a dynamic block's own name for the
/// anonymous block (`*U12`) a reference with values of its own shows.
fn effective_name(d: &Drawing, h: Handle, block: &str) -> String {
    if let Some(r) = cadcraft_doc::dynblock::reference(d, h) {
        return r.definition;
    }
    crate::cmd::blocks::bedit::dynamic_definition(d, block).unwrap_or_else(|| d.block(block).map_or_else(|| block.to_string(), |b| b.name.clone()))
}

/// The members of a collection, for `vlax-for`.
pub(crate) fn collection_items(ctx: &Ctx<'_>, v: &Value) -> Result<Vec<Value>, LispError> {
    let o = obj(v)?;
    let d = doc(ctx)?;
    Ok(match o {
        Vla::Layers => d.layers.iter().map(|l| Value::Vla(Vla::Layer(Arc::from(l.name.as_str())))).collect(),
        Vla::Blocks => {
            std::iter::once(Value::Vla(Vla::ModelSpace)).chain(d.blocks.keys().map(|k| Value::Vla(Vla::Block(Arc::from(k.as_str()))))).collect()
        }
        Vla::ModelSpace => d.model.iter().map(|e| entity_value(e, None)).collect(),
        Vla::Block(name) => {
            let def = d.block(name).ok_or_else(|| automation("Object was erased"))?;
            def.entities.iter().map(|e| entity_value(e, Some(def.name.as_str()))).collect()
        }
        other => return Err(LispError::new(format!("bad argument type: collection: {}", to_text(&Value::Vla(other.clone()), true)))),
    })
}

/// `(vla-item collection index-or-name)`.
fn item(ctx: &Ctx<'_>, o: &Vla, key: &Value) -> Result<Value, LispError> {
    let items = collection_items(ctx, &Value::Vla(o.clone()))?;
    let found = match key {
        Value::Int(_) | Value::Real(_) => {
            let i = number(key)?;
            (i >= 0.0 && i.fract() == 0.0).then(|| items.get(i as usize).cloned()).flatten()
        }
        Value::Str(name) => {
            let d = doc(ctx)?;
            items.into_iter().find(|it| match it {
                Value::Vla(Vla::Layer(n)) => n.eq_ignore_ascii_case(name),
                Value::Vla(Vla::Block(n)) => n.eq_ignore_ascii_case(name),
                Value::Vla(Vla::ModelSpace) => is_model_space(name),
                Value::Vla(e @ Vla::Entity { .. }) => entity_of(d, e).is_some_and(|e| e.handle.hex().eq_ignore_ascii_case(name)),
                _ => false,
            })
        }
        other => return Err(LispError::bad_arg("index", other)),
    };
    found.ok_or_else(|| automation("Key not found"))
}

// ---------------- properties ----------------

/// A property read or write that failed: the object has no such property, or another error.
enum PropErr {
    Unknown,
    Lisp(LispError),
}

impl From<LispError> for PropErr {
    fn from(e: LispError) -> Self {
        PropErr::Lisp(e)
    }
}

type P = Result<Value, PropErr>;

fn str_sysvar(ctx: &Ctx<'_>, name: &str) -> String {
    crate::sysvars::get(ctx.s, name).and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
}

fn support_path(ctx: &Ctx<'_>) -> String {
    ctx.s.lisp_support_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Read property `p` (upper case) of an object.
fn get_prop(ctx: &Ctx<'_>, o: &Vla, p: &str) -> P {
    let d = doc(ctx)?;
    let v = match (o, p) {
        (_, "APPLICATION") => Value::Vla(Vla::Application),
        (Vla::Application, "ACTIVEDOCUMENT") => Value::Vla(Vla::Document),
        (Vla::Application, "PREFERENCES") => Value::Vla(Vla::Preferences),
        (Vla::Application, "NAME") => Value::str("CADCraft"),
        (Vla::Application, "VERSION") => Value::str(env!("CARGO_PKG_VERSION")),
        (Vla::Document, "MODELSPACE") => Value::Vla(Vla::ModelSpace),
        (Vla::Document, "LAYERS") => Value::Vla(Vla::Layers),
        (Vla::Document, "BLOCKS") => Value::Vla(Vla::Blocks),
        (Vla::Document, "NAME") => Value::str(&str_sysvar(ctx, "DWGNAME")),
        (Vla::Document, "PATH") => Value::str(str_sysvar(ctx, "DWGPREFIX").trim_end_matches(['/', '\\'])),
        (Vla::Document, "FULLNAME") => Value::str(&format!("{}{}", str_sysvar(ctx, "DWGPREFIX"), str_sysvar(ctx, "DWGNAME"))),
        (Vla::Document, "ACTIVELAYER") => Value::Vla(Vla::Layer(Arc::from(str_sysvar(ctx, "CLAYER").as_str()))),
        (Vla::Preferences, "FILES") => Value::Vla(Vla::PreferencesFiles),
        (Vla::Preferences, "PROFILES") => Value::Vla(Vla::PreferencesProfiles),
        (Vla::PreferencesFiles, "SUPPORTPATH") => Value::str(&support_path(ctx)),
        (Vla::PreferencesProfiles, "ACTIVEPROFILE") => Value::str("<<Unnamed Profile>>"),
        (Vla::Layers, "COUNT") => Value::Int(i32::try_from(d.layers.len()).unwrap_or(i32::MAX)),
        (Vla::Layers, "OBJECTNAME") => Value::str("AcDbLayerTable"),
        (Vla::Blocks, "COUNT") => Value::Int(i32::try_from(d.blocks.len().saturating_add(1)).unwrap_or(i32::MAX)),
        (Vla::Blocks, "OBJECTNAME") => Value::str("AcDbBlockTable"),
        (Vla::Layers | Vla::Blocks | Vla::ModelSpace | Vla::Layer(_) | Vla::Block(_) | Vla::Entity { .. }, "DOCUMENT") => Value::Vla(Vla::Document),
        (Vla::ModelSpace, "NAME") => Value::str("*Model_Space"),
        (Vla::ModelSpace, "COUNT") => Value::Int(i32::try_from(d.model.len()).unwrap_or(i32::MAX)),
        (Vla::ModelSpace, "ISXREF" | "ISDYNAMICBLOCK") => vbool(false),
        (Vla::ModelSpace, "ISLAYOUT") => vbool(true),
        (Vla::ModelSpace, "OBJECTNAME") => Value::str("AcDbBlockTableRecord"),
        (Vla::ModelSpace, "ORIGIN") => point_sa(Vec3::new(0.0, 0.0, 0.0)),
        (Vla::Layer(name), _) => {
            let l = d.layer(name).ok_or_else(|| automation("Object was erased"))?;
            match p {
                "NAME" => Value::str(&l.name),
                "LOCK" => vbool(l.locked),
                "LAYERON" => vbool(l.on),
                "FREEZE" => vbool(l.frozen),
                "PLOTTABLE" => vbool(l.plot),
                "COLOR" => Value::Int(color_index(l.color)),
                "LINETYPE" => Value::str(&l.linetype),
                "LINEWEIGHT" => Value::Int(i32::from(l.lineweight.to_dxf())),
                "DESCRIPTION" => Value::str(&l.description),
                "PLOTSTYLENAME" => Value::str(&l.plot_style),
                "OBJECTNAME" => Value::str("AcDbLayerTableRecord"),
                _ => return Err(PropErr::Unknown),
            }
        }
        (Vla::Block(name), _) => {
            let b = d.block(name).ok_or_else(|| automation("Object was erased"))?;
            match p {
                "NAME" => Value::str(&b.name),
                "ISXREF" => vbool(b.xref_path.is_some()),
                "ISLAYOUT" => vbool(false),
                "ISDYNAMICBLOCK" => vbool(cadcraft_doc::dynblock::dynamic_block(d, &b.name).is_some()),
                "COUNT" => Value::Int(i32::try_from(b.entities.len()).unwrap_or(i32::MAX)),
                "ORIGIN" => point_sa(b.base),
                "COMMENTS" => Value::str(&b.description),
                "OBJECTNAME" => Value::str("AcDbBlockTableRecord"),
                _ => return Err(PropErr::Unknown),
            }
        }
        (Vla::Entity { .. }, _) => {
            let e = entity_of(d, o).ok_or_else(|| automation("Object was erased"))?;
            return entity_get(d, &e, p);
        }
        _ => return Err(PropErr::Unknown),
    };
    Ok(v)
}

/// The curve measurements of an entity (`Length`, `Area` and the like).
fn curve_of(e: &Entity) -> Result<curve::Curve, PropErr> {
    curve::Curve::of(&e.kind).ok_or(PropErr::Unknown)
}

fn entity_get(d: &Drawing, e: &Entity, p: &str) -> P {
    let c = &e.common;
    Ok(match p {
        "LAYER" => Value::str(&c.layer),
        "COLOR" => Value::Int(color_index(c.color)),
        "LINETYPE" => Value::str(&c.linetype),
        "LINEWEIGHT" => Value::Int(i32::from(c.lineweight.to_dxf())),
        "LINETYPESCALE" => Value::Real(c.ltscale),
        "PLOTSTYLENAME" => Value::str(&c.plot_style),
        "THICKNESS" => Value::Real(c.thickness),
        "VISIBLE" => vbool(c.visible),
        "OBJECTNAME" => Value::str(object_name(&e.kind)),
        "HANDLE" => Value::str(&e.handle.hex()),
        "OBJECTID" => Value::Int(e.handle.0 as i32),
        _ => match (&e.kind, p) {
            (EntityKind::Insert(i), "NAME") => Value::str(&i.block),
            (EntityKind::Insert(i), "EFFECTIVENAME") => Value::str(&effective_name(d, e.handle, &i.block)),
            (EntityKind::Insert(i), "INSERTIONPOINT") => point_sa(i.insert),
            (EntityKind::Insert(i), "ROTATION") => Value::Real(i.rotation),
            (EntityKind::Insert(i), "XSCALEFACTOR") => Value::Real(i.scale.x),
            (EntityKind::Insert(i), "YSCALEFACTOR") => Value::Real(i.scale.y),
            (EntityKind::Insert(i), "ZSCALEFACTOR") => Value::Real(i.scale.z),
            (EntityKind::Insert(i), "HASATTRIBUTES") => vbool(!i.attribs.is_empty()),
            (EntityKind::Insert(_), "ISDYNAMICBLOCK") => vbool(cadcraft_doc::dynblock::reference(d, e.handle).is_some()),
            (EntityKind::Line(l), "STARTPOINT") => point_sa(l.a),
            (EntityKind::Line(l), "ENDPOINT") => point_sa(l.b),
            (EntityKind::Line(l), "ANGLE") => Value::Real(cadcraft_geom::norm_angle(l.a.xy().angle_to(l.b.xy()))),
            (EntityKind::Circle(k), "CENTER") => point_sa(k.center),
            (EntityKind::Circle(k), "RADIUS") => Value::Real(k.radius),
            (EntityKind::Circle(k), "DIAMETER") => Value::Real(2.0 * k.radius),
            (EntityKind::Circle(k), "CIRCUMFERENCE") => Value::Real(std::f64::consts::TAU * k.radius),
            (EntityKind::Arc(a), "CENTER") => point_sa(a.center),
            (EntityKind::Arc(a), "RADIUS") => Value::Real(a.radius),
            (EntityKind::Arc(a), "STARTANGLE") => Value::Real(cadcraft_geom::norm_angle(a.start)),
            (EntityKind::Arc(a), "ENDANGLE") => Value::Real(cadcraft_geom::norm_angle(a.end)),
            (EntityKind::Arc(a), "TOTALANGLE") => Value::Real(cadcraft_geom::ccw_sweep(a.start, a.end)),
            (EntityKind::Arc(_), "ARCLENGTH") => Value::Real(curve_of(e)?.length()),
            (EntityKind::Arc(_), "STARTPOINT" | "ENDPOINT") => {
                let k = curve_of(e)?;
                let t = if p == "STARTPOINT" { k.start_param() } else { k.end_param() };
                k.point_at_param(t).map(point_sa).unwrap_or_default()
            }
            (EntityKind::LwPolyline(l), "CLOSED") => vbool(l.closed),
            (EntityKind::LwPolyline(l), "ELEVATION") => Value::Real(l.elevation),
            (EntityKind::LwPolyline(l), "CONSTANTWIDTH") => Value::Real(l.const_width),
            (EntityKind::LwPolyline(l), "COORDINATES") => doubles_sa(l.vertices.iter().flat_map(|v| [v.p.x, v.p.y])),
            (EntityKind::Polyline3d(l), "CLOSED") => vbool(l.closed),
            (EntityKind::Polyline3d(l), "COORDINATES") => doubles_sa(l.points.iter().flat_map(|v| [v.x, v.y, v.z])),
            (EntityKind::Line(_) | EntityKind::LwPolyline(_) | EntityKind::Polyline3d(_), "LENGTH") => Value::Real(curve_of(e)?.length()),
            (EntityKind::Circle(_) | EntityKind::Arc(_) | EntityKind::LwPolyline(_), "AREA") => Value::Real(curve_of(e)?.area()),
            (EntityKind::Text(t), "TEXTSTRING") => Value::str(&t.value),
            (EntityKind::Text(t), "HEIGHT") => Value::Real(t.height),
            (EntityKind::Text(t), "INSERTIONPOINT") => point_sa(t.insert),
            (EntityKind::Text(t), "ROTATION") => Value::Real(t.rotation),
            (EntityKind::Text(t), "STYLENAME") => Value::str(&t.style),
            (EntityKind::MText(t), "TEXTSTRING") => Value::str(&t.contents),
            (EntityKind::MText(t), "HEIGHT") => Value::Real(t.height),
            (EntityKind::MText(t), "INSERTIONPOINT") => point_sa(t.insert),
            (EntityKind::MText(t), "ROTATION") => Value::Real(t.rotation),
            (EntityKind::MText(t), "STYLENAME") => Value::str(&t.style),
            _ => return Err(PropErr::Unknown),
        },
    })
}

/// A colour number as `put Color` takes it: 0 ByBlock, 256 ByLayer, 1–255 an index.
fn color_arg(v: &Value, by_allowed: bool) -> Result<Color, LispError> {
    let n = number(v)?;
    if n.fract() != 0.0 {
        return Err(automation("Invalid argument"));
    }
    match n as i64 {
        0 if by_allowed => Ok(Color::ByBlock),
        256 if by_allowed => Ok(Color::ByLayer),
        i @ 1..=255 => Ok(Color::Index(i as u8)),
        _ => Err(automation("Invalid argument")),
    }
}

fn lineweight_arg(v: &Value, by_allowed: bool) -> Result<Lineweight, LispError> {
    let n = number(v)?;
    // ByLayer (-1) and ByBlock (-2) are an object's only; Default (-3) and the standard weights
    // (hundredths of a millimetre) anyone's.
    let ok = n.fract() == 0.0
        && match n as i64 {
            -2 | -1 => by_allowed,
            -3 => true,
            w => u16::try_from(w).is_ok_and(|w| Lineweight::STANDARD.contains(&w)),
        };
    if !ok {
        return Err(automation("Invalid argument"));
    }
    Ok(Lineweight::from_dxf(n as i16))
}

/// A linetype name the drawing has (ByLayer and ByBlock always), as the drawing spells it.
fn linetype_arg(d: &Drawing, v: &Value) -> Result<String, LispError> {
    let name = string(v)?;
    for fixed in ["ByLayer", "ByBlock", "Continuous"] {
        if name.eq_ignore_ascii_case(fixed) {
            return Ok(d.linetype(&name).map_or_else(|| fixed.to_string(), |l| l.name.clone()));
        }
    }
    d.linetype(&name).map(|l| l.name.clone()).ok_or_else(|| automation("Key not found"))
}

/// Write property `p` of an object.
fn put_prop(ctx: &mut Ctx<'_>, o: &Vla, p: &str, v: &Value) -> Result<(), PropErr> {
    match (o, p) {
        (Vla::Document, "ACTIVELAYER") => {
            let name = match v {
                Value::Vla(Vla::Layer(n)) => n.to_string(),
                other => string(other)?,
            };
            crate::sysvars::set(ctx.s, "CLAYER", &serde_json::json!(name)).map_err(|e| PropErr::Lisp(automation(&e.to_string())))?;
            Ok(())
        }
        // Support paths and profiles are the application's: read, never changed from here.
        (Vla::PreferencesFiles, "SUPPORTPATH") | (Vla::PreferencesProfiles, "ACTIVEPROFILE") => {
            string(v)?;
            Ok(())
        }
        (Vla::Layer(name), _) => {
            // As LAYER does: the current layer can't be frozen.
            if p == "FREEZE" && truthy(v) && str_sysvar(ctx, "CLAYER").eq_ignore_ascii_case(name) {
                return Err(PropErr::Lisp(automation("Invalid layer: the current layer can't be frozen")));
            }
            let d = doc_mut(ctx)?;
            let lt = if p == "LINETYPE" { Some(linetype_arg(d, v)?) } else { None };
            let l = d.layer_mut(name).ok_or_else(|| automation("Object was erased"))?;
            match p {
                "LOCK" => l.locked = truthy(v),
                "LAYERON" => l.on = truthy(v),
                "FREEZE" => l.frozen = truthy(v),
                "PLOTTABLE" => l.plot = truthy(v),
                "COLOR" => l.color = color_arg(v, false)?,
                "LINEWEIGHT" => l.lineweight = lineweight_arg(v, false)?,
                "LINETYPE" => l.linetype = lt.unwrap_or_default(),
                "DESCRIPTION" => l.description = string(v)?,
                "NAME" => return Err(PropErr::Lisp(LispError::new("renaming a layer through ActiveX is not available yet"))),
                _ => return Err(PropErr::Unknown),
            }
            ctx.s.touch();
            Ok(())
        }
        (Vla::Block(name), "COMMENTS") => {
            let text = string(v)?;
            let d = doc_mut(ctx)?;
            let key = block_key(d, name).ok_or_else(|| automation("Object was erased"))?;
            if let Some(b) = d.blocks.get_mut(&key) {
                Arc::make_mut(b).description = text;
            }
            ctx.s.touch();
            Ok(())
        }
        (Vla::Entity { .. }, _) => {
            let d = doc(ctx)?;
            // An unknown property is reported as such before a locked layer is.
            let e = entity_of(d, o).ok_or_else(|| automation("Object was erased"))?;
            if !entity_writable(&e.kind, p) {
                return Err(PropErr::Unknown);
            }
            let e = writable_entity(d, o)?;
            let new = entity_put(d, &e, p, v)?;
            let d = doc_mut(ctx)?;
            if !replace_entity(d, o, new) {
                return Err(PropErr::Lisp(automation("Object was erased")));
            }
            ctx.s.touch();
            Ok(())
        }
        _ => Err(PropErr::Unknown),
    }
}

/// Whether an entity's property can be written here.
fn entity_writable(kind: &EntityKind, p: &str) -> bool {
    matches!(p, "LAYER" | "COLOR" | "LINETYPE" | "LINEWEIGHT" | "LINETYPESCALE" | "PLOTSTYLENAME" | "THICKNESS" | "VISIBLE")
        || matches!(
            (kind, p),
            (EntityKind::Insert(_), "NAME" | "INSERTIONPOINT" | "ROTATION" | "XSCALEFACTOR" | "YSCALEFACTOR" | "ZSCALEFACTOR")
                | (EntityKind::Line(_), "STARTPOINT" | "ENDPOINT")
                | (EntityKind::Circle(_), "CENTER" | "RADIUS" | "DIAMETER")
                | (EntityKind::Arc(_), "CENTER" | "RADIUS" | "STARTANGLE" | "ENDANGLE")
                | (EntityKind::LwPolyline(_), "CLOSED" | "ELEVATION" | "CONSTANTWIDTH")
                | (EntityKind::Polyline3d(_), "CLOSED")
                | (EntityKind::Text(_) | EntityKind::MText(_), "TEXTSTRING" | "HEIGHT" | "INSERTIONPOINT" | "ROTATION")
        )
}

fn positive(v: &Value) -> Result<f64, LispError> {
    let n = number(v)?;
    if n > 0.0 { Ok(n) } else { Err(automation("Invalid argument")) }
}

fn entity_put(d: &Drawing, e: &Entity, p: &str, v: &Value) -> Result<Entity, PropErr> {
    let mut n = e.clone();
    if p == "INSERTIONPOINT" {
        // The whole object moves with it: a reference's attributes, a text's alignment point.
        let at = match &n.kind {
            EntityKind::Insert(i) => i.insert,
            EntityKind::Text(t) => t.insert,
            EntityKind::MText(t) => t.insert,
            _ => return Err(PropErr::Unknown),
        };
        let to = point_arg(v)?;
        n.kind.transform(&Mat3::translate(Vec2::new(to.x - at.x, to.y - at.y)));
        if to.z != at.z && !shift_z(&mut n.kind, to.z - at.z) {
            return Err(PropErr::Unknown);
        }
        return Ok(n);
    }
    let c = &mut n.common;
    match p {
        "LAYER" => {
            let name = string(v)?;
            c.layer = d.layer(&name).map(|l| l.name.clone()).ok_or_else(|| automation("Key not found"))?;
        }
        "COLOR" => c.color = color_arg(v, true)?,
        "LINETYPE" => c.linetype = linetype_arg(d, v)?,
        "LINEWEIGHT" => c.lineweight = lineweight_arg(v, true)?,
        "LINETYPESCALE" => c.ltscale = positive(v)?,
        "PLOTSTYLENAME" => c.plot_style = string(v)?,
        "THICKNESS" => c.thickness = number(v)?,
        "VISIBLE" => c.visible = truthy(v),
        _ => match (&mut n.kind, p) {
            (EntityKind::Insert(i), "NAME") => {
                let name = string(v)?;
                i.block = d.block(&name).map(|b| b.name.clone()).ok_or_else(|| automation("Key not found"))?;
            }
            (EntityKind::Insert(i), "ROTATION") => {
                let to = number(v)?;
                let turn = to - i.rotation;
                i.rotation = to;
                // The attached text turns with the reference, around its insertion point (as the
                // Properties rotation and ROTATE do).
                let base = i.insert.xy();
                for a in &mut i.attribs {
                    let t = &mut a.text;
                    for q in std::iter::once(&mut t.insert).chain(t.align_pt.as_mut()) {
                        let r = q.xy().rotate_about(base, turn);
                        q.x = r.x;
                        q.y = r.y;
                    }
                    t.rotation = cadcraft_geom::norm_angle(t.rotation + turn);
                    if let Some(ml) = &mut a.multiline {
                        let r = ml.mtext.insert.xy().rotate_about(base, turn);
                        ml.mtext.insert.x = r.x;
                        ml.mtext.insert.y = r.y;
                        ml.mtext.rotation = cadcraft_geom::norm_angle(ml.mtext.rotation + turn);
                    }
                }
            }
            (EntityKind::Insert(i), "XSCALEFACTOR" | "YSCALEFACTOR" | "ZSCALEFACTOR") => {
                // Attribute text can't follow a scale along one axis: refused rather than left
                // behind.
                if !i.attribs.is_empty() {
                    return Err(PropErr::Lisp(LispError::new("scaling a block reference with attributes along one axis is not available yet")));
                }
                let k = nonzero(v)?;
                match p {
                    "XSCALEFACTOR" => i.scale.x = k,
                    "YSCALEFACTOR" => i.scale.y = k,
                    _ => i.scale.z = k,
                }
            }
            (EntityKind::Line(l), "STARTPOINT") => l.a = point_arg(v)?,
            (EntityKind::Line(l), "ENDPOINT") => l.b = point_arg(v)?,
            (EntityKind::Circle(k), "CENTER") => k.center = point_arg(v)?,
            (EntityKind::Circle(k), "RADIUS") => k.radius = positive(v)?,
            (EntityKind::Circle(k), "DIAMETER") => k.radius = positive(v)? / 2.0,
            (EntityKind::Arc(a), "CENTER") => a.center = point_arg(v)?,
            (EntityKind::Arc(a), "RADIUS") => a.radius = positive(v)?,
            (EntityKind::Arc(a), "STARTANGLE") => a.start = cadcraft_geom::norm_angle(number(v)?),
            (EntityKind::Arc(a), "ENDANGLE") => a.end = cadcraft_geom::norm_angle(number(v)?),
            (EntityKind::LwPolyline(l), "CLOSED") => l.closed = truthy(v),
            (EntityKind::LwPolyline(l), "ELEVATION") => l.elevation = number(v)?,
            (EntityKind::LwPolyline(l), "CONSTANTWIDTH") => {
                let w = number(v)?;
                if w < 0.0 {
                    return Err(PropErr::Lisp(automation("Invalid argument")));
                }
                l.const_width = w;
                // One width for the whole polyline: the per-vertex widths go.
                for v in &mut l.vertices {
                    v.start_width = 0.0;
                    v.end_width = 0.0;
                }
            }
            (EntityKind::Polyline3d(l), "CLOSED") => l.closed = truthy(v),
            (EntityKind::Text(t), "TEXTSTRING") => t.value = string(v)?,
            (EntityKind::Text(t), "HEIGHT") => t.height = positive(v)?,
            (EntityKind::Text(t), "ROTATION") => t.rotation = number(v)?,
            (EntityKind::MText(t), "TEXTSTRING") => t.contents = string(v)?,
            (EntityKind::MText(t), "HEIGHT") => t.height = positive(v)?,
            (EntityKind::MText(t), "ROTATION") => t.rotation = number(v)?,
            _ => return Err(PropErr::Unknown),
        },
    }
    Ok(n)
}

fn nonzero(v: &Value) -> Result<f64, LispError> {
    let n = number(v)?;
    if n != 0.0 { Ok(n) } else { Err(automation("Invalid argument")) }
}

fn prop_result(p: &str, r: P) -> Result<Value, LispError> {
    match r {
        Ok(v) => Ok(v),
        Err(PropErr::Unknown) => Err(unknown_name(p)),
        Err(PropErr::Lisp(e)) => Err(e),
    }
}

fn put_result(p: &str, r: Result<(), PropErr>) -> Result<Value, LispError> {
    prop_result(p, r.map(|()| Value::Nil))
}

/// `(vla-get-<property> object)`.
fn vla_get(ctx: &mut Ctx<'_>, name: &'static str, a: Vec<Value>) -> F {
    arity(&a, 1, 1)?;
    let p = name.strip_prefix("VLA-GET-").unwrap_or(name);
    let o = obj(&a[0])?;
    Ok(Flow::Value(prop_result(p, get_prop(ctx, o, p))?))
}

/// `(vla-put-<property> object value)`.
fn vla_put(ctx: &mut Ctx<'_>, name: &'static str, a: Vec<Value>) -> F {
    arity(&a, 2, 2)?;
    let p = name.strip_prefix("VLA-PUT-").unwrap_or(name);
    let o = obj(&a[0])?.clone();
    Ok(Flow::Value(put_result(p, put_prop(ctx, &o, p, &a[1]))?))
}

/// `(vlax-get-property object property)`; `(vlax-get object property)` gives arrays as lists.
fn vlax_get_property(ctx: &mut Ctx<'_>, name: &'static str, a: Vec<Value>) -> F {
    arity(&a, 2, 2)?;
    let o = obj(&a[0])?;
    let p = member_name(&a[1])?;
    let v = prop_result(&p, get_prop(ctx, o, &p))?;
    Ok(Flow::Value(if name == "VLAX-GET" { listed(v) } else { v }))
}

/// `(vlax-put-property object property value)`, `(vlax-put object property value)`.
fn vlax_put_property(ctx: &mut Ctx<'_>, _name: &'static str, a: Vec<Value>) -> F {
    arity(&a, 3, 3)?;
    let o = obj(&a[0])?.clone();
    let p = member_name(&a[1])?;
    Ok(Flow::Value(put_result(&p, put_prop(ctx, &o, &p, &a[2]))?))
}

/// `(vlax-property-available-p object property [check-modify])`.
fn vlax_property_available_p(ctx: &mut Ctx<'_>, a: Vec<Value>) -> F {
    arity(&a, 2, 3)?;
    let o = obj(&a[0])?;
    let p = member_name(&a[1])?;
    let readable = get_prop(ctx, o, &p).is_ok();
    let modify = a.get(2).is_some_and(|v| !v.is_nil());
    if !readable || !modify {
        return Ok(Flow::Value(Value::bool(readable)));
    }
    let d = doc(ctx)?;
    let writable = match o {
        Vla::Entity { .. } => entity_of(d, o).is_some_and(|e| entity_writable(&e.kind, &p)),
        Vla::Layer(_) => matches!(p.as_str(), "LOCK" | "LAYERON" | "FREEZE" | "PLOTTABLE" | "COLOR" | "LINEWEIGHT" | "LINETYPE" | "DESCRIPTION"),
        Vla::Block(_) => p == "COMMENTS",
        Vla::Document => p == "ACTIVELAYER",
        Vla::PreferencesFiles => p == "SUPPORTPATH",
        Vla::PreferencesProfiles => p == "ACTIVEPROFILE",
        _ => false,
    };
    Ok(Flow::Value(Value::bool(writable)))
}

// ---------------- methods ----------------

/// The methods an object answers (`vlax-method-applicable-p`).
fn has_method(o: &Vla, m: &str) -> bool {
    match o {
        Vla::Entity { .. } => matches!(m, "GETBOUNDINGBOX" | "MOVE" | "DELETE" | "UPDATE" | "COPY" | "INTERSECTWITH"),
        Vla::Layers | Vla::Blocks => matches!(m, "ITEM" | "ADD"),
        Vla::ModelSpace | Vla::Block(_) => m == "ITEM",
        Vla::Document => matches!(m, "REGEN" | "STARTUNDOMARK" | "ENDUNDOMARK"),
        _ => false,
    }
}

/// Call method `m` of an object. `listed`: arrays come back as lists (`vlax-invoke`) instead of
/// safearrays (`vla-<method>`, `vlax-invoke-method`); output arguments (`GetBoundingBox`'s
/// symbols) are set the same way.
fn invoke(ctx: &mut Ctx<'_>, o: &Vla, m: &str, args: &[Value], as_lists: bool) -> Result<Value, LispError> {
    let out = |v: Value| if as_lists { listed(v) } else { v };
    match (o, m) {
        (Vla::Entity { .. }, "GETBOUNDINGBOX") => {
            arity(args, 2, 2)?;
            let (Value::Sym(lo), Value::Sym(hi)) = (&args[0], &args[1]) else {
                return Err(LispError::bad_arg("symbolp", if matches!(args[0], Value::Sym(_)) { &args[1] } else { &args[0] }));
            };
            let d = doc(ctx)?;
            let e = entity_of(d, o).ok_or_else(|| automation("Object was erased"))?;
            let b = cadcraft_doc::entity_bounds(d, &e, 0);
            if b.is_empty() || !b.min.is_finite() || !b.max.is_finite() {
                return Err(automation("Null extents"));
            }
            let (z0, z1) = z_range(d, &e, 0);
            ctx.env.set(lo, out(point_sa(Vec3::new(b.min.x, b.min.y, z0))));
            ctx.env.set(hi, out(point_sa(Vec3::new(b.max.x, b.max.y, z1))));
            Ok(Value::Nil)
        }
        (Vla::Entity { .. }, "MOVE") => {
            arity(args, 2, 2)?;
            let (from, to) = (point_arg(&args[0])?, point_arg(&args[1])?);
            let d = doc(ctx)?;
            let mut e = (*writable_entity(d, o)?).clone();
            e.kind.transform(&Mat3::translate(Vec2::new(to.x - from.x, to.y - from.y)));
            let dz = to.z - from.z;
            if dz != 0.0 && !shift_z(&mut e.kind, dz) {
                return Err(LispError::new(format!("moving {} along Z is not available yet", object_name(&e.kind))));
            }
            let d = doc_mut(ctx)?;
            if !replace_entity(d, o, e) {
                return Err(automation("Object was erased"));
            }
            ctx.s.touch();
            Ok(Value::Nil)
        }
        (Vla::Entity { handle, block, .. }, "DELETE") => {
            arity(args, 0, 0)?;
            let d = doc(ctx)?;
            writable_entity(d, o)?;
            let d = doc_mut(ctx)?;
            match block {
                None => {
                    d.remove_entity(*handle);
                }
                Some(b) => {
                    if let Some(key) = block_key(d, b)
                        && let Some(def) = d.blocks.get_mut(&key)
                    {
                        Arc::make_mut(def).entities.remove(*handle);
                    }
                }
            }
            ctx.s.touch();
            Ok(Value::Nil)
        }
        (Vla::Entity { .. }, "UPDATE") => {
            arity(args, 0, 0)?;
            ctx.s.touch();
            Ok(Value::Nil)
        }
        (Vla::Entity { handle, block: None, .. }, "COPY") => {
            arity(args, 0, 0)?;
            let d = doc_mut(ctx)?;
            let e = d.entity(*handle).map(|e| (**e).clone()).ok_or_else(|| automation("Object was erased"))?;
            let space = d.space_of(*handle).ok_or_else(|| automation("Object was erased"))?;
            let h = d.new_handle();
            let copy = Entity { handle: h, ..e };
            let v = entity_value(&copy, None);
            d.space_mut(&space).ok_or_else(|| automation("Object was erased"))?.push(copy);
            ctx.s.touch();
            Ok(v)
        }
        (Vla::Entity { .. }, "INTERSECTWITH") => {
            arity(args, 1, 2)?;
            let other = obj(&args[0])?;
            let mode = match args.get(1) {
                Some(v) => number(v)? as i64,
                None => 0,
            };
            let d = doc(ctx)?;
            let a = entity_of(d, o).ok_or_else(|| automation("Object was erased"))?;
            let b = entity_of(d, other).ok_or_else(|| automation("Object was erased"))?;
            // Crossings in plan that are also at the same height on both objects.
            let pts: Vec<Vec3> = intersections(&a, &b, mode)
                .into_iter()
                .filter_map(|p| {
                    let (za, zb) = (z_at(&a, p), z_at(&b, p));
                    ((za - zb).abs() <= 1e-9 * (1.0 + za.abs().max(zb.abs()))).then(|| Vec3::new(p.x, p.y, za))
                })
                .collect();
            if pts.is_empty() && as_lists {
                return Ok(Value::Nil);
            }
            Ok(out(doubles_sa(pts.iter().flat_map(|p| [p.x, p.y, p.z]))))
        }
        (Vla::Layers | Vla::Blocks | Vla::ModelSpace | Vla::Block(_), "ITEM") => {
            arity(args, 1, 1)?;
            item(ctx, o, &args[0])
        }
        (Vla::Layers, "ADD") => {
            arity(args, 1, 1)?;
            let name = string(&args[0])?;
            if !crate::cmd::layer::valid_name(&name) {
                return Err(automation("Invalid input"));
            }
            let d = doc_mut(ctx)?;
            d.ensure_layer(&name);
            let name = d.layer(&name).map_or(name, |l| l.name.clone());
            ctx.s.touch();
            Ok(Value::Vla(Vla::Layer(Arc::from(name.as_str()))))
        }
        (Vla::Blocks, "ADD") => {
            arity(args, 2, 2)?;
            let base = point_arg(&args[0])?;
            let name = string(&args[1])?;
            if !crate::cmd::blocks::valid_block_name(&name) {
                return Err(automation("Invalid input"));
            }
            let d = doc_mut(ctx)?;
            let key = match block_key(d, &name) {
                Some(k) => k,
                None => {
                    let mut b = cadcraft_doc::Block::new(&name);
                    b.base = base;
                    d.blocks.insert(name.clone(), Arc::new(b));
                    ctx.s.touch();
                    name
                }
            };
            Ok(Value::Vla(Vla::Block(Arc::from(key.as_str()))))
        }
        // The whole routine is one undo step already; the view redraws on its own.
        (Vla::Document, "REGEN" | "STARTUNDOMARK" | "ENDUNDOMARK") => Ok(Value::Nil),
        _ => {
            // A property through `vlax-invoke`: read with no argument, write with one.
            match args {
                [] => prop_result(m, get_prop(ctx, o, m)).map(out),
                [v] => put_result(m, put_prop(ctx, &o.clone(), m, v)),
                _ => Err(unknown_name(m)),
            }
        }
    }
}

/// The z of an entity at plan point `p` on it: along a sloped line or 3D polyline segment, the
/// height there; any other object is flat at its elevation.
fn z_at(e: &Entity, p: Vec2) -> f64 {
    let on_line = |a: Vec3, b: Vec3| {
        let d = b.xy() - a.xy();
        let l2 = d.x * d.x + d.y * d.y;
        let t = if l2 <= f64::EPSILON { 0.0 } else { ((p.x - a.x) * d.x + (p.y - a.y) * d.y) / l2 };
        a.z + (b.z - a.z) * t
    };
    match &e.kind {
        EntityKind::Line(l) => on_line(l.a, l.b),
        EntityKind::Polyline3d(pl) => {
            let n = pl.points.len();
            let count = if pl.closed { n } else { n.saturating_sub(1) };
            (0..count)
                .filter_map(|i| Some((*pl.points.get(i)?, *pl.points.get((i + 1) % n.max(1))?)))
                .map(|(a, b)| (cadcraft_geom::Line::new(a.xy(), b.xy()).closest(p).dist(p), on_line(a, b)))
                .fold(None, |best: Option<(f64, f64)>, c| if best.is_none_or(|b| c.0 < b.0) { Some(c) } else { best })
                .map_or_else(|| entity_z(e), |(_, z)| z)
        }
        _ => entity_z(e),
    }
}

/// The lowest and highest z of an entity's geometry (bounding boxes are measured in plan). A
/// block reference spans its contents' z, scaled and lifted to where it is inserted; nesting is
/// followed to [`cadcraft_doc::MAX_BLOCK_DEPTH`]. Each block's contents are measured once per
/// depth, so many references to shared nested blocks stay linear.
fn z_range(d: &Drawing, e: &Entity, depth: usize) -> (f64, f64) {
    z_range_cached(d, e, depth, &mut HashMap::new())
}

/// A block's contents' z span, by block name and nesting depth.
type ZCache = HashMap<(String, usize), Option<(f64, f64)>>;

fn z_range_cached(d: &Drawing, e: &Entity, depth: usize, cache: &mut ZCache) -> (f64, f64) {
    let span = |zs: &mut dyn Iterator<Item = f64>| zs.fold(None, |r: Option<(f64, f64)>, z| Some(r.map_or((z, z), |(a, b)| (a.min(z), b.max(z)))));
    let r = match &e.kind {
        EntityKind::Line(l) => span(&mut [l.a.z, l.b.z].into_iter()),
        EntityKind::Polyline3d(p) => span(&mut p.points.iter().map(|q| q.z)),
        EntityKind::Solid(s) | EntityKind::Trace(s) => span(&mut s.corners.iter().map(|q| q.z)),
        EntityKind::Face3d(f) => span(&mut f.corners.iter().map(|q| q.z)),
        EntityKind::Insert(i) if depth < cadcraft_doc::MAX_BLOCK_DEPTH => d
            .block(&i.block)
            .and_then(|b| {
                let key = (i.block.clone(), depth);
                let inner = match cache.get(&key) {
                    Some(r) => *r,
                    None => {
                        let r = b
                            .entities
                            .iter()
                            .map(|c| z_range_cached(d, c, depth + 1, cache))
                            .fold(None, |r: Option<(f64, f64)>, (lo, hi)| Some(r.map_or((lo, hi), |(a, z)| (a.min(lo), z.max(hi)))));
                        cache.insert(key, r);
                        r
                    }
                }?;
                let place = |z: f64| i.insert.z + i.scale.z * (z - b.base.z);
                let (p, q) = (place(inner.0), place(inner.1));
                Some((p.min(q), p.max(q)))
            })
            .map(|(lo, hi)| {
                // Attached attributes sit where they are, not in the definition.
                i.attribs.iter().filter(|a| !a.invisible).fold((lo, hi), |(lo, hi), a| {
                    let z = a.multiline.as_ref().map_or(a.text.insert.z, |m| m.mtext.insert.z);
                    (lo.min(z), hi.max(z))
                })
            }),
        _ => None,
    };
    r.filter(|(a, b)| a.is_finite() && b.is_finite()).unwrap_or_else(|| {
        let z = entity_z(e);
        (z, z)
    })
}

/// Move an entity along Z by `dz` (`EntityKind::transform` is 2D). `false` for a kind this
/// doesn't know how to lift.
fn shift_z(kind: &mut EntityKind, dz: f64) -> bool {
    match kind {
        EntityKind::Line(l) => {
            l.a.z += dz;
            l.b.z += dz;
        }
        EntityKind::Point(p) => p.p.z += dz,
        EntityKind::Circle(c) => c.center.z += dz,
        EntityKind::Arc(a) => a.center.z += dz,
        EntityKind::Ellipse(e) => e.center.z += dz,
        EntityKind::LwPolyline(p) => p.elevation += dz,
        EntityKind::Polyline3d(p) => p.points.iter_mut().for_each(|q| q.z += dz),
        EntityKind::Insert(i) => {
            i.insert.z += dz;
            for a in &mut i.attribs {
                a.text.insert.z += dz;
                if let Some(p) = a.text.align_pt.as_mut() {
                    p.z += dz;
                }
                // A multiline attribute draws and saves from its multiline text.
                if let Some(ml) = a.multiline.as_mut() {
                    ml.mtext.insert.z += dz;
                }
            }
        }
        EntityKind::Text(t) => {
            t.insert.z += dz;
            if let Some(p) = t.align_pt.as_mut() {
                p.z += dz;
            }
        }
        EntityKind::MText(t) => t.insert.z += dz,
        _ => return false,
    }
    true
}

/// The z of an entity's geometry (for bounding boxes and intersections, which are 2D).
fn entity_z(e: &Entity) -> f64 {
    match &e.kind {
        EntityKind::Line(l) => l.a.z,
        EntityKind::Circle(c) => c.center.z,
        EntityKind::Arc(a) => a.center.z,
        EntityKind::LwPolyline(p) => p.elevation,
        EntityKind::Polyline3d(p) => p.points.first().map_or(0.0, |q| q.z),
        EntityKind::Point(p) => p.p.z,
        EntityKind::Ellipse(e) => e.center.z,
        EntityKind::Insert(i) => i.insert.z,
        EntityKind::Text(t) => t.insert.z,
        EntityKind::MText(t) => t.insert.z,
        EntityKind::Solid(s) | EntityKind::Trace(s) => s.corners[0].z,
        EntityKind::Face3d(f) => f.corners[0].z,
        _ => 0.0,
    }
}

/// The line and arc pieces of an entity (a circle is a full arc).
fn pieces(e: &Entity) -> Vec<Segment> {
    e.kind
        .prims()
        .into_iter()
        .filter_map(|p| match p {
            cadcraft_doc::Prim::Seg(s) => Some(s),
            cadcraft_doc::Prim::Circle(c) => Some(Segment::Arc { arc: GArc::new(c.center, c.radius, 0.0, 0.0), ccw: true }),
            _ => None,
        })
        .collect()
}

/// Where two entities cross. `mode`: 0 neither extended (acExtendNone), 1 this one extended, 2
/// the other, 3 both (lines become infinite, arcs full circles).
fn intersections(a: &Entity, b: &Entity, mode: i64) -> Vec<Vec2> {
    const MAX_PIECES: usize = 100_000;
    let (pa, pb) = (pieces(a), pieces(b));
    if pa.len().saturating_mul(pb.len()) > MAX_PIECES * 10 {
        return Vec::new();
    }
    let near = |segs: &[Segment], p: Vec2| segs.iter().any(|s| s.closest(p).dist(p) <= 1e-7 * (1.0 + p.x.abs().max(p.y.abs())));
    let mut raw: Vec<Vec2> = Vec::new();
    for s in &pa {
        for t in &pb {
            for p in intersect_ext(s, t, mode != 0) {
                let keep = match mode {
                    1 => near(&pb, p),
                    2 => near(&pa, p),
                    _ => true,
                };
                if keep && p.is_finite() {
                    raw.push(p);
                }
            }
        }
    }
    dedup_points(raw)
}

/// Drop repeated points (within a tolerance relative to the coordinates' size), keeping the
/// first of each in order. A grid of tolerance-sized cells keeps this linear: each point looks
/// only at its own and the eight neighbouring cells.
pub(super) fn dedup_points(raw: Vec<Vec2>) -> Vec<Vec2> {
    let size = raw.iter().fold(0.0f64, |m, p| m.max(p.x.abs()).max(p.y.abs()));
    let tol = 1e-9 * (1.0 + size);
    let cell = |v: f64| (v / tol).floor() as i64;
    let mut grid: HashMap<(i64, i64), Vec<Vec2>> = HashMap::new();
    let mut out = Vec::new();
    for p in raw {
        let (cx, cy) = (cell(p.x), cell(p.y));
        let seen = (-1..=1).any(|dx| {
            (-1..=1).any(|dy| grid.get(&(cx.saturating_add(dx), cy.saturating_add(dy))).is_some_and(|v| v.iter().any(|q| q.dist(p) <= tol)))
        });
        if !seen {
            grid.entry((cx, cy)).or_default().push(p);
            out.push(p);
        }
    }
    out
}

/// `(vla-<method> object args…)`.
fn vla_method(ctx: &mut Ctx<'_>, name: &'static str, a: Vec<Value>) -> F {
    let m = name.strip_prefix("VLA-").unwrap_or(name);
    let Some((first, rest)) = a.split_first() else { return Err(args_err(true)) };
    let o = obj(first)?.clone();
    Ok(Flow::Value(invoke(ctx, &o, m, rest, false)?))
}

/// `(vlax-invoke object method args…)` (arrays as lists) and `(vlax-invoke-method …)`.
fn vlax_invoke(ctx: &mut Ctx<'_>, name: &'static str, a: Vec<Value>) -> F {
    if a.len() < 2 {
        return Err(args_err(true));
    }
    let o = obj(&a[0])?.clone();
    let m = member_name(&a[1])?;
    Ok(Flow::Value(invoke(ctx, &o, &m, a.get(2..).unwrap_or(&[]), name == "VLAX-INVOKE")?))
}

fn vlax_method_applicable_p(_ctx: &mut Ctx<'_>, a: Vec<Value>) -> F {
    arity(&a, 2, 2)?;
    let o = obj(&a[0])?;
    Ok(Flow::Value(Value::bool(has_method(o, &member_name(&a[1])?))))
}

// ---------------- conversions and object state ----------------

fn vlax_get_acad_object(_ctx: &mut Ctx<'_>, a: Vec<Value>) -> F {
    arity(&a, 0, 0)?;
    Ok(Flow::Value(Value::Vla(Vla::Application)))
}

fn vlax_ename_to_vla_object(ctx: &mut Ctx<'_>, a: Vec<Value>) -> F {
    arity(&a, 1, 1)?;
    let Value::Ename(h) = &a[0] else { return Err(LispError::bad_arg("lentityp", &a[0])) };
    let d = doc(ctx)?;
    let (e, block) = find_entity(d, *h).ok_or_else(|| LispError::bad_arg("lentityp", &a[0]))?;
    Ok(Flow::Value(entity_value(&e, block.as_deref())))
}

/// The entity with handle `h`: in model space or a layout (`None` block), or in a block
/// definition (handles are unique across the drawing).
pub(crate) fn find_entity(d: &Drawing, h: Handle) -> Option<(Arc<Entity>, Option<String>)> {
    if let Some(e) = d.entity(h) {
        return Some((e.clone(), None));
    }
    d.blocks.values().find_map(|b| b.entities.get(h).map(|e| (e.clone(), Some(b.name.clone()))))
}

fn vlax_vla_object_to_ename(_ctx: &mut Ctx<'_>, a: Vec<Value>) -> F {
    arity(&a, 1, 1)?;
    match obj(&a[0])? {
        Vla::Entity { handle, .. } => Ok(Flow::Value(Value::Ename(*handle))),
        _ => Ok(Flow::Value(Value::Nil)),
    }
}

/// `(vlax-write-enabled-p object)`: it exists and isn't on a locked layer.
fn vlax_write_enabled_p(ctx: &mut Ctx<'_>, a: Vec<Value>) -> F {
    arity(&a, 1, 1)?;
    let o = obj(&a[0])?;
    let d = doc(ctx)?;
    Ok(Flow::Value(Value::bool(match o {
        Vla::Entity { .. } => entity_of(d, o).is_some_and(|e| !layer_locked(d, &e)),
        Vla::Layer(n) => d.layer(n).is_some(),
        Vla::Block(n) => d.block(n).is_some(),
        _ => true,
    })))
}

fn vlax_read_enabled_p(ctx: &mut Ctx<'_>, a: Vec<Value>) -> F {
    arity(&a, 1, 1)?;
    let erased = erased(ctx, obj(&a[0])?)?;
    Ok(Flow::Value(Value::bool(!erased)))
}

fn erased(ctx: &Ctx<'_>, o: &Vla) -> Result<bool, LispError> {
    let d = doc(ctx)?;
    Ok(match o {
        Vla::Entity { .. } => entity_of(d, o).is_none(),
        Vla::Layer(n) => d.layer(n).is_none(),
        Vla::Block(n) => d.block(n).is_none(),
        _ => false,
    })
}

fn vlax_erased_p(ctx: &mut Ctx<'_>, a: Vec<Value>) -> F {
    arity(&a, 1, 1)?;
    let erased = erased(ctx, obj(&a[0])?)?;
    Ok(Flow::Value(Value::bool(erased)))
}

/// `vlax-release-object`: objects here hold nothing to release.
fn vlax_release_object(_ctx: &mut Ctx<'_>, a: Vec<Value>) -> F {
    arity(&a, 1, 1)?;
    Ok(Flow::Value(Value::Nil))
}

// ---------------- safearrays and variants ----------------

/// The starting value of a safearray element of type `vtype`.
fn blank(vtype: i32) -> Value {
    match vtype {
        VB_INTEGER | VB_LONG => Value::Int(0),
        VB_SINGLE | VB_DOUBLE => Value::Real(0.0),
        VB_STRING => Value::str(""),
        VB_BOOLEAN => vbool(false),
        _ => Value::Nil,
    }
}

/// A value converted to safearray element type `vtype`.
fn coerce(vtype: i32, v: &Value) -> Result<Value, LispError> {
    Ok(match vtype {
        VB_INTEGER | VB_LONG => match v {
            Value::Int(_) => v.clone(),
            Value::Real(r) if r.is_finite() && r.abs() < 2_147_483_648.0 => Value::Int(r.round() as i32),
            other => return Err(LispError::bad_arg("numberp", other)),
        },
        VB_SINGLE | VB_DOUBLE => Value::Real(number(v)?),
        // The string itself is shared, not copied (a copy would escape the memory budget).
        VB_STRING => {
            string(v)?;
            v.clone()
        }
        VB_BOOLEAN => vbool(truthy(v)),
        VB_OBJECT => {
            obj(v)?;
            v.clone()
        }
        _ => {
            holds_no_safearray(v)?;
            v.clone()
        }
    })
}

/// A value a variant safearray may hold: one with no safearray anywhere in it. An array holding
/// itself (through a list) would be a reference cycle never freed; cons cells can't form one on
/// their own, so this is the only way. The search is bounded.
fn holds_no_safearray(v: &Value) -> Result<(), LispError> {
    const MAX_VISIT: usize = 1_000_000;
    let mut work = vec![v.clone()];
    let mut seen = 0usize;
    while let Some(x) = work.pop() {
        seen += 1;
        if seen > MAX_VISIT {
            return Err(LispError::new("too large to store in a safearray"));
        }
        match x {
            Value::SafeArray(_) => return Err(LispError::new("a safearray can't hold a safearray here")),
            Value::Cons(c) => {
                work.push(c.car.clone());
                work.push(c.cdr.clone());
            }
            // A function's body can quote an array too.
            Value::Func(f) => work.push(f.body.clone()),
            _ => {}
        }
    }
    Ok(())
}

fn safearray(v: &Value) -> Result<&Arc<SafeArray>, LispError> {
    match v {
        Value::SafeArray(sa) => Ok(sa),
        other => Err(LispError::bad_arg("safearrayp", other)),
    }
}

/// `(vlax-make-safearray type '(lower . upper))`: one dimension.
fn vlax_make_safearray(_ctx: &mut Ctx<'_>, a: Vec<Value>) -> F {
    if a.len() < 2 {
        return Err(args_err(true));
    }
    if a.len() > 2 {
        return Err(LispError::new("safearrays of more than one dimension are not available yet"));
    }
    let Value::Int(vtype) = a[0] else { return Err(LispError::bad_arg("fixnump", &a[0])) };
    if ![VB_INTEGER, VB_LONG, VB_SINGLE, VB_DOUBLE, VB_STRING, VB_OBJECT, VB_BOOLEAN, VB_VARIANT].contains(&vtype) {
        return Err(LispError::new(format!("bad argument value: safearray type {vtype}")));
    }
    let (Value::Int(lo), Value::Int(hi)) = (a[1].car(), a[1].cdr()) else { return Err(LispError::bad_arg("safearray bounds", &a[1])) };
    let n = i64::from(hi) - i64::from(lo) + 1;
    if !(0..=MAX_SAFEARRAY).contains(&n) {
        return Err(LispError::new(format!("bad argument value: safearray bounds {lo} .. {hi}")));
    }
    Ok(Flow::Value(SafeArray::value(vtype, lo, (0..n).map(|_| blank(vtype)).collect())))
}

/// `(vlax-safearray->list array)`: also takes a list (already one) as it is.
fn vlax_safearray_to_list(_ctx: &mut Ctx<'_>, a: Vec<Value>) -> F {
    arity(&a, 1, 1)?;
    match &a[0] {
        Value::SafeArray(sa) => Ok(Flow::Value(Value::list(sa.to_vec()))),
        v if v.is_list() && !v.is_nil() => Ok(Flow::Value(v.clone())),
        other => Err(LispError::bad_arg("safearrayp", other)),
    }
}

fn vlax_safearray_fill(_ctx: &mut Ctx<'_>, a: Vec<Value>) -> F {
    arity(&a, 2, 2)?;
    let sa = safearray(&a[0])?;
    let values: Vec<Value> = a[1].to_vec();
    let mut items = sa.items.lock().map_err(|_| LispError::new("safearray unavailable"))?;
    if values.len() > items.len() {
        return Err(LispError::new("too many elements for this safearray"));
    }
    for (slot, v) in items.iter_mut().zip(&values) {
        *slot = coerce(sa.vtype, v)?;
    }
    drop(items);
    Ok(Flow::Value(a[0].clone()))
}

/// The element index of a safearray for a `vlax-safearray-…-element` index.
fn element_index(sa: &SafeArray, v: &Value, len: usize) -> Result<usize, LispError> {
    let Value::Int(i) = v else { return Err(LispError::bad_arg("fixnump", v)) };
    usize::try_from(i64::from(*i) - i64::from(sa.lower)).ok().filter(|k| *k < len).ok_or_else(|| automation("Invalid index"))
}

fn vlax_safearray_get_element(_ctx: &mut Ctx<'_>, a: Vec<Value>) -> F {
    arity(&a, 2, 2)?;
    let sa = safearray(&a[0])?;
    let items = sa.items.lock().map_err(|_| LispError::new("safearray unavailable"))?;
    let k = element_index(sa, &a[1], items.len())?;
    Ok(Flow::Value(items.get(k).cloned().unwrap_or_default()))
}

fn vlax_safearray_put_element(_ctx: &mut Ctx<'_>, a: Vec<Value>) -> F {
    arity(&a, 3, 3)?;
    let sa = safearray(&a[0])?;
    let v = coerce(sa.vtype, &a[2])?;
    let mut items = sa.items.lock().map_err(|_| LispError::new("safearray unavailable"))?;
    let k = element_index(sa, &a[1], items.len())?;
    if let Some(slot) = items.get_mut(k) {
        *slot = v.clone();
    }
    Ok(Flow::Value(v))
}

fn bound(a: &[Value], upper: bool) -> F {
    arity(a, 2, 2)?;
    let sa = safearray(&a[0])?;
    if !matches!(a[1], Value::Int(1)) {
        return Err(automation("Invalid dimension"));
    }
    let len = i64::try_from(sa.element_count()).unwrap_or(i64::MAX);
    let upper_bound = i64::from(sa.lower).saturating_add(len).saturating_sub(1);
    Ok(Flow::Value(Value::Int(if upper { i32::try_from(upper_bound).unwrap_or(i32::MAX) } else { sa.lower })))
}

fn vlax_safearray_get_l_bound(_ctx: &mut Ctx<'_>, a: Vec<Value>) -> F {
    bound(&a, false)
}

fn vlax_safearray_get_u_bound(_ctx: &mut Ctx<'_>, a: Vec<Value>) -> F {
    bound(&a, true)
}

fn vlax_safearray_type(_ctx: &mut Ctx<'_>, a: Vec<Value>) -> F {
    arity(&a, 1, 1)?;
    Ok(Flow::Value(Value::Int(safearray(&a[0])?.vtype)))
}

/// `(vlax-3d-point x y [z])` or `(vlax-3d-point list)`: a safearray of three doubles.
fn vlax_3d_point(_ctx: &mut Ctx<'_>, a: Vec<Value>) -> F {
    arity(&a, 1, 3)?;
    let p = if a.len() == 1 {
        point_arg(&a[0])?
    } else {
        let z = match a.get(2) {
            Some(v) => number(v)?,
            None => 0.0,
        };
        Vec3::new(number(&a[0])?, number(&a[1])?, z)
    };
    Ok(Flow::Value(point_sa(p)))
}

/// `(vlax-make-variant [value [type]])`: values are their own variants here; a type converts.
fn vlax_make_variant(_ctx: &mut Ctx<'_>, a: Vec<Value>) -> F {
    arity(&a, 0, 2)?;
    let v = a.first().cloned().unwrap_or_default();
    Ok(Flow::Value(match a.get(1) {
        Some(Value::Int(t)) if !v.is_nil() && *t & VB_ARRAY == 0 && *t != VB_VARIANT => coerce(*t, &v)?,
        _ => v,
    }))
}

fn vlax_variant_value(_ctx: &mut Ctx<'_>, a: Vec<Value>) -> F {
    arity(&a, 1, 1)?;
    Ok(Flow::Value(a[0].clone()))
}

fn vlax_variant_type(_ctx: &mut Ctx<'_>, a: Vec<Value>) -> F {
    arity(&a, 1, 1)?;
    Ok(Flow::Value(Value::Int(match &a[0] {
        Value::Nil => 0,
        Value::Int(_) => VB_LONG,
        Value::Real(_) => VB_DOUBLE,
        Value::Str(_) => VB_STRING,
        Value::Vla(_) => VB_OBJECT,
        Value::SafeArray(sa) => VB_ARRAY.checked_add(sa.vtype).unwrap_or(VB_ARRAY),
        Value::Sym(s) if s.name() == VLAX_TRUE || s.name() == VLAX_FALSE => VB_BOOLEAN,
        _ => VB_VARIANT,
    })))
}

fn vlax_variant_change_type(_ctx: &mut Ctx<'_>, a: Vec<Value>) -> F {
    arity(&a, 2, 2)?;
    let Value::Int(t) = a[1] else { return Err(LispError::bad_arg("fixnump", &a[1])) };
    Ok(Flow::Value(if a[0].is_nil() || t & VB_ARRAY != 0 || t == VB_VARIANT { a[0].clone() } else { coerce(t, &a[0])? }))
}

// ---------------- the table ----------------

macro_rules! ctx {
    ($name:literal, $f:path) => {
        Builtin { name: $name, kind: BuiltinKind::Ctx($f) }
    };
}
macro_rules! named {
    ($name:literal, $f:path) => {
        Builtin { name: $name, kind: BuiltinKind::Named($f) }
    };
}
/// `vla-get-<p>` and `vla-put-<p>` for each property name.
macro_rules! props {
    ($($p:literal),* $(,)?) => {
        &[$(
            Builtin { name: concat!("VLA-GET-", $p), kind: BuiltinKind::Named(vla_get) },
            Builtin { name: concat!("VLA-PUT-", $p), kind: BuiltinKind::Named(vla_put) },
        )*]
    };
}
/// `vla-<m>` for each method name.
macro_rules! methods {
    ($($m:literal),* $(,)?) => {
        &[$( Builtin { name: concat!("VLA-", $m), kind: BuiltinKind::Named(vla_method) }, )*]
    };
}

static PROPERTIES: &[Builtin] = props!(
    "LAYER",
    "COLOR",
    "LINETYPE",
    "LINEWEIGHT",
    "LINETYPESCALE",
    "PLOTSTYLENAME",
    "THICKNESS",
    "VISIBLE",
    "OBJECTNAME",
    "HANDLE",
    "OBJECTID",
    "APPLICATION",
    "DOCUMENT",
    "NAME",
    "EFFECTIVENAME",
    "INSERTIONPOINT",
    "ROTATION",
    "XSCALEFACTOR",
    "YSCALEFACTOR",
    "ZSCALEFACTOR",
    "HASATTRIBUTES",
    "ISDYNAMICBLOCK",
    "STARTPOINT",
    "ENDPOINT",
    "LENGTH",
    "ANGLE",
    "CENTER",
    "RADIUS",
    "DIAMETER",
    "CIRCUMFERENCE",
    "AREA",
    "STARTANGLE",
    "ENDANGLE",
    "TOTALANGLE",
    "ARCLENGTH",
    "CLOSED",
    "ELEVATION",
    "CONSTANTWIDTH",
    "COORDINATES",
    "TEXTSTRING",
    "HEIGHT",
    "STYLENAME",
    "LOCK",
    "LAYERON",
    "FREEZE",
    "PLOTTABLE",
    "DESCRIPTION",
    "ISXREF",
    "ISLAYOUT",
    "COUNT",
    "ORIGIN",
    "COMMENTS",
    "ACTIVEDOCUMENT",
    "PREFERENCES",
    "VERSION",
    "MODELSPACE",
    "LAYERS",
    "BLOCKS",
    "PATH",
    "FULLNAME",
    "ACTIVELAYER",
    "FILES",
    "PROFILES",
    "SUPPORTPATH",
    "ACTIVEPROFILE",
);

static METHODS: &[Builtin] =
    methods!("GETBOUNDINGBOX", "MOVE", "DELETE", "UPDATE", "COPY", "INTERSECTWITH", "ITEM", "ADD", "REGEN", "STARTUNDOMARK", "ENDUNDOMARK");

static FUNCTIONS: &[Builtin] = &[
    Builtin { name: "VLAX-FOR", kind: BuiltinKind::Special(Special::VlaxFor) },
    ctx!("VLAX-GET-ACAD-OBJECT", vlax_get_acad_object),
    ctx!("VLAX-ENAME->VLA-OBJECT", vlax_ename_to_vla_object),
    ctx!("VLAX-VLA-OBJECT->ENAME", vlax_vla_object_to_ename),
    named!("VLAX-GET-PROPERTY", vlax_get_property),
    named!("VLAX-GET", vlax_get_property),
    named!("VLAX-PUT-PROPERTY", vlax_put_property),
    named!("VLAX-PUT", vlax_put_property),
    named!("VLAX-INVOKE", vlax_invoke),
    named!("VLAX-INVOKE-METHOD", vlax_invoke),
    ctx!("VLAX-PROPERTY-AVAILABLE-P", vlax_property_available_p),
    ctx!("VLAX-METHOD-APPLICABLE-P", vlax_method_applicable_p),
    ctx!("VLAX-WRITE-ENABLED-P", vlax_write_enabled_p),
    ctx!("VLAX-READ-ENABLED-P", vlax_read_enabled_p),
    ctx!("VLAX-ERASED-P", vlax_erased_p),
    ctx!("VLAX-RELEASE-OBJECT", vlax_release_object),
    ctx!("VLAX-MAKE-SAFEARRAY", vlax_make_safearray),
    ctx!("VLAX-SAFEARRAY->LIST", vlax_safearray_to_list),
    ctx!("VLAX-SAFEARRAY-FILL", vlax_safearray_fill),
    ctx!("VLAX-SAFEARRAY-GET-ELEMENT", vlax_safearray_get_element),
    ctx!("VLAX-SAFEARRAY-PUT-ELEMENT", vlax_safearray_put_element),
    ctx!("VLAX-SAFEARRAY-GET-L-BOUND", vlax_safearray_get_l_bound),
    ctx!("VLAX-SAFEARRAY-GET-U-BOUND", vlax_safearray_get_u_bound),
    ctx!("VLAX-SAFEARRAY-TYPE", vlax_safearray_type),
    ctx!("VLAX-3D-POINT", vlax_3d_point),
    ctx!("VLAX-MAKE-VARIANT", vlax_make_variant),
    ctx!("VLAX-VARIANT-VALUE", vlax_variant_value),
    ctx!("VLAX-VARIANT-TYPE", vlax_variant_type),
    ctx!("VLAX-VARIANT-CHANGE-TYPE", vlax_variant_change_type),
    ctx!("VLAX-CURVE-GETSTARTPARAM", curve::get_start_param),
    ctx!("VLAX-CURVE-GETENDPARAM", curve::get_end_param),
    ctx!("VLAX-CURVE-GETSTARTPOINT", curve::get_start_point),
    ctx!("VLAX-CURVE-GETENDPOINT", curve::get_end_point),
    ctx!("VLAX-CURVE-GETPOINTATPARAM", curve::get_point_at_param),
    ctx!("VLAX-CURVE-GETDISTATPARAM", curve::get_dist_at_param),
    ctx!("VLAX-CURVE-GETPARAMATDIST", curve::get_param_at_dist),
    ctx!("VLAX-CURVE-GETPOINTATDIST", curve::get_point_at_dist),
    ctx!("VLAX-CURVE-GETPARAMATPOINT", curve::get_param_at_point),
    ctx!("VLAX-CURVE-GETDISTATPOINT", curve::get_dist_at_point),
    ctx!("VLAX-CURVE-GETCLOSESTPOINTTO", curve::get_closest_point_to),
    ctx!("VLAX-CURVE-GETFIRSTDERIV", curve::get_first_deriv),
    ctx!("VLAX-CURVE-ISCLOSED", curve::is_closed),
    ctx!("VLAX-CURVE-GETAREA", curve::get_area),
];

/// Constants Visual LISP defines: `:vlax-true`, the variant types, the `ac…` enumerations
/// routines pass (extend modes, viewports, colours, lineweights).
const CONSTANTS: &[(&str, i32)] = &[
    ("VLAX-VBEMPTY", 0),
    ("VLAX-VBNULL", 1),
    ("VLAX-VBINTEGER", VB_INTEGER),
    ("VLAX-VBLONG", VB_LONG),
    ("VLAX-VBSINGLE", VB_SINGLE),
    ("VLAX-VBDOUBLE", VB_DOUBLE),
    ("VLAX-VBSTRING", VB_STRING),
    ("VLAX-VBOBJECT", VB_OBJECT),
    ("VLAX-VBBOOLEAN", VB_BOOLEAN),
    ("VLAX-VBVARIANT", VB_VARIANT),
    ("VLAX-VBARRAY", VB_ARRAY),
    ("ACEXTENDNONE", 0),
    ("ACEXTENDTHISENTITY", 1),
    ("ACEXTENDOTHERENTITY", 2),
    ("ACEXTENDBOTH", 3),
    ("ACACTIVEVIEWPORT", 0),
    ("ACALLVIEWPORTS", 1),
    ("ACBYBLOCK", 0),
    ("ACBYLAYER", 256),
    ("ACRED", 1),
    ("ACYELLOW", 2),
    ("ACGREEN", 3),
    ("ACCYAN", 4),
    ("ACBLUE", 5),
    ("ACMAGENTA", 6),
    ("ACWHITE", 7),
    ("ACLNWTBYLAYER", -1),
    ("ACLNWTBYBLOCK", -2),
    ("ACLNWTBYLWDEFAULT", -3),
];

/// Bind the ActiveX built-ins and constants in a new environment.
pub fn install(env: &mut Env) {
    for b in PROPERTIES.iter().chain(METHODS).chain(FUNCTIONS) {
        env.globals.insert(Sym::new(b.name), Value::Subr(b));
    }
    for name in [VLAX_TRUE, VLAX_FALSE] {
        env.globals.insert(Sym::new(name), Value::sym(name));
    }
    for (name, v) in CONSTANTS {
        env.globals.insert(Sym::new(name), Value::Int(*v));
    }
}
