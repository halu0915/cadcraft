//! Spike prototype (not shipped): turn an ordinary block into a dynamic block with a Flip
//! parameter + Flip action over all of its entities, written straight through acadrust.
//!
//! `cargo run -p cadcraft-dwg --example dynblock_author -- flip <in.dwg> <BLOCK> <out.dwg>`
//!
//! Object set (read off a minimal one-flip block, see the spike report):
//! BLOCK_RECORD xdata (AcDbDynamicBlockTrueName / AcDbDynamicBlockGUID / AcDbBlockRepETag)
//! BLOCK_RECORD xdict { ACAD_ENHANCEDBLOCK -> ACAD_EVALUATION_GRAPH,
//!                      AcDbDynamicBlockRoundTripPurgePreventer -> purge preventer }
//! graph nodes: FLIPPARAMETER, FLIPGRIP, 3x GRIPLOCATIONCOMPONENT, FLIPACTION; 5 edges.

use acadrust::classes::{DxfClass, ProxyFlags};
use acadrust::objects::{
    BlockAction, BlockActionOffsets, BlockConnection, BlockElement, BlockEvalExpression, BlockEvalValue, BlockEvaluationEdge, BlockEvaluationGraph,
    BlockEvaluationNode, BlockFlipAction, BlockFlipGrip, BlockFlipParameter, BlockGrip, BlockGripExpression, BlockLinearParameter, BlockOrientedGrip,
    BlockParameter, BlockParameterProperty, BlockParameterValueSet, BlockRepresentationData, BlockStretchAction, BlockStretchHandle,
    BlockTwoPointParameter, BlockVisibilityParameter, BlockVisibilityState, DynamicBlockData, DynamicBlockObject, ObjectType,
};
use acadrust::tables::AppId;
use acadrust::types::{Vector2, Vector3};
use acadrust::xdata::{ExtendedDataRecord, XDataValue};
use acadrust::{CadDocument, DwgReader, DwgWriter, Handle};

type Res<T> = Result<T, String>;

/// AutoCAD writes the dynamic-block classes as "ObjectDBX Classes" with proxy flags 1153
/// (erase|...|disables proxy warning) and R2010+ class version 25.
fn ensure_class(doc: &mut CadDocument, dxf: &str, cpp: &str, maint: i32) {
    if doc.classes.contains(dxf) {
        return;
    }
    let mut c = DxfClass::new(dxf, cpp);
    c.proxy_flags = ProxyFlags(1153);
    c.dwg_version = 25;
    c.maintenance_version = maint;
    doc.classes.add_or_update(c);
}

fn ensure_appid(doc: &mut CadDocument, name: &str) -> Res<()> {
    if doc.app_ids.iter().any(|a| a.name.eq_ignore_ascii_case(name)) {
        return Ok(());
    }
    let mut app = AppId::new(name);
    app.handle = doc.allocate_handle();
    doc.app_ids.add(app).map_err(|e| e.to_string())
}

/// AutoCAD's element header: major 33 / minor 329 (the class version pair seen on every node).
fn eval(node_id: i32, value_code: i16, value: BlockEvalValue) -> BlockEvalExpression {
    BlockEvalExpression { parent_id: -1, major: 33, minor: 329, value_code, value, node_id }
}

fn element(node_id: i32, name: &str) -> BlockElement {
    BlockElement { eval: eval(node_id, -9999, BlockEvalValue::None), name: name.into(), major: 33, minor: 329, eed_1071: 0 }
}

fn object(doc: &mut CadDocument, owner: Handle, dxf: &str, cpp: &str, data: DynamicBlockData) -> Handle {
    let handle = doc.allocate_handle();
    let mut o = DynamicBlockObject::new(dxf, cpp);
    o.handle = handle;
    o.owner = owner;
    o.data = data;
    doc.objects.insert(handle, ObjectType::DynamicBlock(o));
    handle
}

fn conn(name: &str) -> BlockConnection {
    BlockConnection { code: 1, name: name.into() }
}

fn edge(id: i32, src: i32, dst: i32, incoming: i32, prev_out: i32, next_out: i32) -> BlockEvaluationEdge {
    BlockEvaluationEdge {
        id,
        next_id: 0,
        incoming_edge: incoming,
        source_node: src,
        destination_node: dst,
        outgoing_edges: [-1, -1, prev_out, next_out, -1],
    }
}

/// Shared scaffolding: classes, APPIDs, block-record xdata and the extension dictionary.
fn prepare(doc: &mut CadDocument, block: &str, classes: &[(&str, &str, i32)]) -> Res<(Handle, Vec<Handle>, Handle)> {
    let (br, ents) = {
        let r = doc.block_records.get(block).ok_or(format!("no block {block}"))?;
        (r.handle, r.entity_handles.clone())
    };
    if ents.is_empty() {
        return Err(format!("block {block} has no entities "));
    }
    if doc
        .extension_dictionary_handle(br)
        .is_some_and(|x| matches!(doc.objects.get(&x), Some(ObjectType::Dictionary(d)) if d.entries.iter().any(|(k, _)| k == "ACAD_ENHANCEDBLOCK")))
    {
        return Err(format!("block {block} is already dynamic"));
    }
    for &(dxf, cpp, m) in [
        ("ACAD_EVALUATION_GRAPH", "AcDbEvalGraph", 55),
        ("BLOCKGRIPLOCATIONCOMPONENT", "AcDbBlockGripExpr", 20),
        ("ACDB_DYNAMICBLOCKPURGEPREVENTER_VERSION", "AcDbDynamicBlockPurgePreventer", 61),
    ]
    .iter()
    .chain(classes)
    {
        ensure_class(doc, dxf, cpp, m);
    }
    for app in ["AcDbDynamicBlockTrueName", "AcDbDynamicBlockGUID", "AcDbBlockRepETag"] {
        ensure_appid(doc, app)?;
    }

    // Block record xdata: true name, a GUID, representation tag (1, entity count as AutoCAD wrote it).
    let mut x = ExtendedDataRecord::new("AcDbDynamicBlockTrueName");
    x.add_value(XDataValue::String(block.into()));
    doc.set_object_xdata_record(br, x);
    let mut x = ExtendedDataRecord::new("AcDbDynamicBlockGUID");
    let seed = block.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3));
    x.add_value(XDataValue::String(format!(
        "{{{:08X}-{:04X}-4{:03X}-8{:03X}-{:012X}}}",
        seed >> 32,
        (seed >> 16) & 0xffff,
        seed & 0xfff,
        (seed >> 20) & 0xfff,
        seed & 0xffff_ffff_ffff
    )));
    doc.set_object_xdata_record(br, x);
    let mut x = ExtendedDataRecord::new("AcDbBlockRepETag");
    x.add_value(XDataValue::Integer16(1));
    x.add_value(XDataValue::Integer32(ents.len() as i32));
    doc.set_object_xdata_record(br, x);

    let xd = doc.ensure_extension_dictionary(br);
    Ok((br, ents, xd))
}

/// Make `block` dynamic with one flip parameter whose reflection line runs from `base` to `end`.
fn add_flip(doc: &mut CadDocument, block: &str, base: Vector3, end: Vector3) -> Res<()> {
    let (br, ents, xd) = prepare(
        doc,
        block,
        &[
            ("BLOCKFLIPPARAMETER", "AcDbBlockFlipParameter", 55),
            ("BLOCKFLIPGRIP", "AcDbBlockFlipGrip", 55),
            ("BLOCKFLIPACTION", "AcDbBlockFlipAction", 55),
        ],
    )?;
    let graph = doc.allocate_handle();
    let dir = Vector3::new(end.x - base.x, end.y - base.y, 0.0);
    let len = (dir.x * dir.x + dir.y * dir.y).sqrt();
    if len <= f64::EPSILON || !len.is_finite() {
        return Err("flip line has no length".into());
    }
    // Grip sits off the base point, perpendicular (clockwise) to the line, as AutoCAD places it.
    let off = len * 0.15;
    let grip_at = Vector3::new(base.x + dir.y / len * off, base.y - dir.x / len * off, 0.0);
    let mid = Vector3::new((base.x + end.x) / 2.0, (base.y + end.y) / 2.0, 0.0);

    let two_point = BlockTwoPointParameter {
        parameter: BlockParameter { element: element(1, "Flip"), show_properties: true, chain_actions: false },
        definition_base_point: base,
        definition_end_point: end,
        property_states: [2, 0, 0, 0],
        ..Default::default()
    };
    let param = object(
        doc,
        graph,
        "BLOCKFLIPPARAMETER",
        "AcDbBlockFlipParameter",
        DynamicBlockData::FlipParameter(BlockFlipParameter {
            parameter: two_point,
            flip_label: "Flip state1".into(),
            flip_label_description: String::new(),
            base_state_label: "Not flipped".into(),
            flipped_state_label: "Flipped".into(),
            definition_label_point: Vector3::new(mid.x + off * 0.3, mid.y, 0.0),
            flags_96: 2,
            tooltip: "UpdatedFlip".into(),
        }),
    );
    let grip = object(
        doc,
        graph,
        "BLOCKFLIPGRIP",
        "AcDbBlockFlipGrip",
        DynamicBlockData::FlipGrip(BlockFlipGrip {
            // flags_91/92 name the eval ids of the grip's X/Y location expressions (4, 5).
            grip: BlockGrip {
                element: element(2, "Grip"),
                flags_91: 4,
                flags_92: 5,
                location: grip_at,
                insert_cycling: false,
                insert_cycling_weight: -1,
            },
            combined_state: 3,
            orientation: dir,
        }),
    );
    let tiny = 2.2250738585072014e-307; // AutoCAD's "not evaluated yet" placeholder
    let mut exprs = Vec::new();
    for (id, code, value, name) in [
        (3, 70, BlockEvalValue::Short(0), "UpdatedFlip"),
        (4, 40, BlockEvalValue::Real(tiny), "UpdatedBaseX"),
        (5, 40, BlockEvalValue::Real(tiny), "UpdatedBaseY"),
    ] {
        exprs.push(object(
            doc,
            graph,
            "BLOCKGRIPLOCATIONCOMPONENT",
            "AcDbBlockGripExpr",
            DynamicBlockData::GripLocationComponent(BlockGripExpression { eval: eval(id, code, value), grip_type: 1, expression: name.into() }),
        ));
    }
    let action = object(
        doc,
        graph,
        "BLOCKFLIPACTION",
        "AcDbBlockFlipAction",
        DynamicBlockData::FlipAction(BlockFlipAction {
            action: BlockAction {
                element: element(7, "Flip1"),
                display_location: Vector3::new(grip_at.x, grip_at.y - off, 0.0),
                dependencies: ents.clone(),
                action_ids: vec![],
            },
            connections: [conn("Flip"), conn("UpdatedFlip"), conn("UpdatedBase"), conn("UpdatedEnd")],
        }),
    );

    // Graph: node 0 = parameter, 1 = grip, 2..4 = grip expressions, 5 = action.
    // node_data = [first in-edge, last in-edge, first out-edge, last out-edge].
    let node = |id: i32, next: i32, expression: Handle, data: [i32; 4]| BlockEvaluationNode {
        id,
        edge_flags: 32,
        next_id: next,
        expression,
        node_data: data,
        active_cycles: None,
    };
    let nodes = vec![
        node(0, 1, param, [3, 3, 0, 4]),
        node(1, 2, grip, [-1, -1, 3, 3]),
        node(2, 3, exprs[0], [2, 2, -1, -1]),
        node(3, 4, exprs[1], [0, 0, -1, -1]),
        node(4, 5, exprs[2], [1, 1, -1, -1]),
        node(5, 7, action, [4, 4, -1, -1]),
    ];
    let edges = vec![
        edge(0, 0, 3, 1, -1, 1),
        edge(1, 0, 4, 1, 0, 2),
        edge(2, 0, 2, 1, 1, 4),
        BlockEvaluationEdge { id: 3, next_id: 0, incoming_edge: 1, source_node: 1, destination_node: 0, outgoing_edges: [-1; 5] },
        edge(4, 0, 5, 4, 2, -1),
    ];
    finish(doc, br, xd, graph, 7, nodes, edges)
}

/// Shared tail: the evaluation graph object, the purge preventer and the xdict entries.
#[allow(clippy::too_many_arguments)]
fn finish(
    doc: &mut CadDocument,
    br: Handle,
    xd: Handle,
    graph: Handle,
    first_node_id: i32,
    nodes: Vec<BlockEvaluationNode>,
    edges: Vec<BlockEvaluationEdge>,
) -> Res<()> {
    let mut g = DynamicBlockObject::new("ACAD_EVALUATION_GRAPH", "AcDbEvalGraph");
    g.handle = graph;
    g.owner = xd;
    g.reactors = vec![xd];
    g.data = DynamicBlockData::EvaluationGraph(BlockEvaluationGraph { first_node_id, first_node_id_copy: first_node_id, nodes, edges });
    doc.objects.insert(graph, ObjectType::DynamicBlock(g));

    let pp = doc.allocate_handle();
    let mut p = DynamicBlockObject::new("ACDB_DYNAMICBLOCKPURGEPREVENTER_VERSION", "AcDbDynamicBlockPurgePreventer");
    p.handle = pp;
    p.owner = xd;
    p.reactors = vec![xd];
    p.data = DynamicBlockData::Representation(BlockRepresentationData { flags: 1, block: br });
    doc.objects.insert(pp, ObjectType::DynamicBlock(p));

    match doc.objects.get_mut(&xd) {
        Some(ObjectType::Dictionary(d)) => {
            d.duplicate_cloning = 1;
            d.hard_owner = true;
            d.entries.push(("ACAD_ENHANCEDBLOCK".into(), graph));
            d.entries.push(("AcDbDynamicBlockRoundTripPurgePreventer".into(), pp));
        }
        _ => return Err("extension dictionary missing".into()),
    }
    Ok(())
}

/// Make `block` dynamic with one visibility parameter. `states` = (name, indexes into the
/// block's entity list that are visible in that state); the first state is the default.
fn add_visibility(doc: &mut CadDocument, block: &str, states: &[(String, Vec<usize>)], grip_at: Vector3) -> Res<()> {
    if states.is_empty() {
        return Err("a visibility parameter needs at least one state".into());
    }
    let (br, ents, xd) = prepare(
        doc,
        block,
        &[("BLOCKVISIBILITYPARAMETER", "AcDbBlockVisibilityParameter", 55), ("BLOCKVISIBILITYGRIP", "AcDbBlockVisibilityGrip", 55)],
    )?;
    let mut vis_states = Vec::new();
    for (name, idx) in states {
        let mut visible = Vec::new();
        for i in idx {
            visible.push(*ents.get(*i).ok_or(format!("state {name}: no entity #{i} (block has {})", ents.len()))?);
        }
        vis_states.push(BlockVisibilityState { name: name.clone(), visible_blocks: visible, visible_params: vec![] });
    }
    // AutoCAD stores the definition in its default (first) state: entities hidden there carry
    // the entity "invisible" flag (DXF 60 = 1), seen on the sample block's hidden polyline.
    // Without it a named (non-*U) insert draws every state at once.
    let default_visible = vis_states.first().map(|s| s.visible_blocks.clone()).unwrap_or_default();
    for h in &ents {
        if let Some(e) = doc.get_entity_mut(*h) {
            e.common_mut().invisible = !default_visible.contains(h);
        }
    }
    let graph = doc.allocate_handle();
    let param = doc.allocate_handle();
    doc.objects.insert(
        param,
        ObjectType::BlockVisibilityParameter(BlockVisibilityParameter {
            handle: param,
            owner: graph,
            eval_parent_id: -1,
            eval_major: 33,
            eval_minor: 329,
            eval_value_code: -9999,
            eval_value: BlockEvalValue::None,
            eval_node_id: 1,
            element_name: "Visibility State".into(),
            element_major: 33,
            element_minor: 329,
            element_eed_1071: 0,
            show_properties: true,
            chain_actions: false,
            name: "Visibility1".into(),
            description: String::new(),
            def_point: grip_at,
            property_info: Default::default(),
            property_info_count: 2,
            is_initialized: true,
            unknown_bool: false,
            all_blocks: ents.clone(),
            states: vis_states,
        }),
    );
    let grip = object(
        doc,
        graph,
        "BLOCKVISIBILITYGRIP",
        "AcDbBlockVisibilityGrip",
        DynamicBlockData::VisibilityGrip(BlockGrip {
            element: element(2, "Grip"),
            flags_91: 3,
            flags_92: 4,
            location: grip_at,
            insert_cycling: false,
            insert_cycling_weight: -1,
        }),
    );
    let mut exprs = Vec::new();
    for (id, name) in [(3, "UpdatedX"), (4, "UpdatedY")] {
        exprs.push(object(
            doc,
            graph,
            "BLOCKGRIPLOCATIONCOMPONENT",
            "AcDbBlockGripExpr",
            DynamicBlockData::GripLocationComponent(BlockGripExpression {
                eval: eval(id, 40, BlockEvalValue::Real(0.0)),
                grip_type: 1,
                expression: name.into(),
            }),
        ));
    }
    let node = |id: i32, next: i32, expression: Handle, data: [i32; 4]| BlockEvaluationNode {
        id,
        edge_flags: 32,
        next_id: next,
        expression,
        node_data: data,
        active_cycles: None,
    };
    let nodes = vec![
        node(0, 1, param, [0, 0, 1, 2]),
        node(1, 2, grip, [-1, -1, 0, 0]),
        node(2, 3, exprs[0], [1, 1, -1, -1]),
        node(3, 4, exprs[1], [2, 2, -1, -1]),
    ];
    let edges = vec![
        BlockEvaluationEdge { id: 0, next_id: 0, incoming_edge: 1, source_node: 1, destination_node: 0, outgoing_edges: [-1; 5] },
        edge(1, 0, 2, 1, -1, 2),
        edge(2, 0, 3, 1, 1, -1),
    ];
    finish(doc, br, xd, graph, 4, nodes, edges)
}

/// Make `block` dynamic with one linear parameter (base -> end) whose end grip drives a stretch
/// action. `frame` = two corners of the stretch window; `moved` = (entity index, vertex indexes
/// inside the frame) as AutoCAD lists them; `values` = allowed distances (empty = free).
#[allow(clippy::too_many_arguments)]
fn add_linear_stretch(
    doc: &mut CadDocument,
    block: &str,
    name: &str,
    base: Vector3,
    end: Vector3,
    frame: [Vector2; 2],
    moved: &[(usize, Vec<i32>)],
    values: &[f64],
) -> Res<()> {
    let (br, ents, xd) = prepare(
        doc,
        block,
        &[
            ("BLOCKLINEARPARAMETER", "AcDbBlockLinearParameter", 55),
            ("BLOCKLINEARGRIP", "AcDbBlockLinearGrip", 55),
            ("BLOCKSTRETCHACTION", "AcDbBlockStretchAction", 55),
        ],
    )?;
    let dir = Vector3::new(end.x - base.x, end.y - base.y, 0.0);
    let len = (dir.x * dir.x + dir.y * dir.y).sqrt();
    if len <= f64::EPSILON || !len.is_finite() {
        return Err("linear parameter has no length".into());
    }
    let mut handles = Vec::new();
    for (i, idx) in moved {
        handles.push(BlockStretchHandle { handle: *ents.get(*i).ok_or(format!("no entity #{i}"))?, indexes: idx.clone() });
    }
    // Eval ids: parameter 1, end grip 2, grip X/Y expressions 3/4, stretch 5.
    // A grip location expression names its parameter (grip_type = 1); a parameter property
    // and an action connection name the node they read from by eval id.
    let graph = doc.allocate_handle();
    let mut props: [BlockParameterProperty; 4] = Default::default();
    props[2].connections = vec![BlockConnection { code: 2, name: "UpdatedX".into() }];
    props[3].connections = vec![BlockConnection { code: 2, name: "UpdatedY".into() }];
    let value_set = if values.is_empty() {
        BlockParameterValueSet::default()
    } else {
        // flags 8 = list of allowed values (seen on the sample's width/height lists).
        BlockParameterValueSet { description: String::new(), flags: 8, minimum: 0.0, maximum: 0.0, increment: 0.0, values: values.to_vec() }
    };
    let param = object(
        doc,
        graph,
        "BLOCKLINEARPARAMETER",
        "AcDbBlockLinearParameter",
        DynamicBlockData::LinearParameter(BlockLinearParameter {
            parameter: BlockTwoPointParameter {
                parameter: BlockParameter { element: element(1, name), show_properties: true, chain_actions: false },
                definition_base_point: base,
                definition_end_point: end,
                properties: props,
                property_states: [0, 2, 0, 0],
                ..Default::default()
            },
            distance_name: name.into(),
            distance_description: String::new(),
            distance: len,
            value_set,
        }),
    );
    let grip = object(
        doc,
        graph,
        "BLOCKLINEARGRIP",
        "AcDbBlockLinearGrip",
        DynamicBlockData::LinearGrip(BlockOrientedGrip {
            grip: BlockGrip {
                element: element(2, "End Grip"),
                flags_91: 3,
                flags_92: 4,
                location: end,
                insert_cycling: false,
                insert_cycling_weight: -1,
            },
            orientation: dir,
        }),
    );
    let mut exprs = Vec::new();
    for (id, n) in [(3, "UpdatedEndX"), (4, "UpdatedEndY")] {
        exprs.push(object(
            doc,
            graph,
            "BLOCKGRIPLOCATIONCOMPONENT",
            "AcDbBlockGripExpr",
            DynamicBlockData::GripLocationComponent(BlockGripExpression {
                eval: eval(id, -9999, BlockEvalValue::None),
                grip_type: 1,
                expression: n.into(),
            }),
        ));
    }
    let action = object(
        doc,
        graph,
        "BLOCKSTRETCHACTION",
        "AcDbBlockStretchAction",
        DynamicBlockData::StretchAction(BlockStretchAction {
            action: BlockAction {
                element: element(5, "Stretch"),
                display_location: Vector3::new(end.x, end.y - len * 0.1, 0.0),
                dependencies: handles.iter().map(|h| h.handle).collect(),
                action_ids: vec![],
            },
            connections: [BlockConnection { code: 1, name: "EndXDelta".into() }, BlockConnection { code: 1, name: "EndYDelta".into() }],
            points: frame.to_vec(),
            handles,
            codes: vec![],
            offsets: BlockActionOffsets { distance_multiplier: 1.0, angle_offset: 0.0, flags: 0 },
        }),
    );
    let node = |id: i32, next: i32, expression: Handle, data: [i32; 4]| BlockEvaluationNode {
        id,
        edge_flags: 32,
        next_id: next,
        expression,
        node_data: data,
        active_cycles: None,
    };
    let nodes = vec![
        node(0, 1, param, [2, 2, 0, 3]),
        node(1, 2, grip, [-1, -1, 2, 2]),
        node(2, 3, exprs[0], [0, 0, -1, -1]),
        node(3, 4, exprs[1], [1, 1, -1, -1]),
        node(4, 5, action, [3, 3, -1, -1]),
    ];
    let edges = vec![
        edge(0, 0, 2, 1, -1, 1),
        edge(1, 0, 3, 1, 0, 3),
        BlockEvaluationEdge { id: 2, next_id: 0, incoming_edge: 2, source_node: 1, destination_node: 0, outgoing_edges: [-1; 5] },
        edge(3, 0, 4, 2, 1, -1),
    ];
    finish(doc, br, xd, graph, 5, nodes, edges)
}

/// "SMOKE=0,1,2;HEAT=0,3" -> [(SMOKE, [0,1,2]), (HEAT, [0,3])]
fn parse_states(spec: &str) -> Res<Vec<(String, Vec<usize>)>> {
    spec.split(';')
        .filter(|s| !s.is_empty())
        .map(|s| {
            let (name, list) = s.split_once('=').ok_or(format!("state {s:?}: want NAME=i,j"))?;
            let idx = list.split(',').map(|i| i.trim().parse::<usize>().map_err(|e| format!("{s:?}: {e}"))).collect::<Res<Vec<_>>>()?;
            Ok((name.to_string(), idx))
        })
        .collect()
}

fn run() -> Res<()> {
    let a: Vec<String> = std::env::args().collect();
    let (cmd, input, block, out) = match a.as_slice() {
        [_, c, i, b, o, ..] => (c.as_str(), i, b, o),
        _ => return Err("usage: flip <in.dwg> <BLOCK> <out.dwg> | vis <in.dwg> <BLOCK> <out.dwg> NAME=i,j;NAME2=k".into()),
    };
    let bytes = std::fs::read(input).map_err(|e| e.to_string())?;
    let mut doc = DwgReader::from_stream(std::io::Cursor::new(bytes)).read().map_err(|e| e.to_string())?;
    match cmd {
        // Reflection line: vertical through the block's 30x30 test square centre.
        "flip" => add_flip(&mut doc, block, Vector3::new(15.0, -3.0, 0.0), Vector3::new(15.0, 33.0, 0.0))?,
        "vis" => {
            let spec = a.get(5).ok_or("vis needs a state list")?;
            add_visibility(&mut doc, block, &parse_states(spec)?, Vector3::new(33.0, 30.0, 0.0))?
        }
        // Panel prototype: 60x40 rectangle (entity 0), width along X, right edge (vertices 1, 2)
        // stretches; allowed widths 60/80/100/120.
        "width" => add_linear_stretch(
            &mut doc,
            block,
            "Width",
            Vector3::new(0.0, 0.0, 0.0),
            Vector3::new(60.0, 0.0, 0.0),
            [Vector2::new(70.0, 45.0), Vector2::new(50.0, -5.0)],
            &[(0, vec![1, 2])],
            &[60.0, 80.0, 100.0, 120.0],
        )?,
        _ => return Err(format!("unknown command {cmd}")),
    }
    let bytes = DwgWriter::write_to_vec(&doc).map_err(|e| e.to_string())?;
    std::fs::write(out, bytes).map_err(|e| e.to_string())?;
    println!("wrote {out}");
    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
