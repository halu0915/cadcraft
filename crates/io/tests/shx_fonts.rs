//! Synthetic fonts generated in memory; no third-party font assets.
use cadcraft_doc::{Common, Drawing, EntityKind, Space, Text, TextStyle};
use cadcraft_geom::Vec3;

#[path = "support/pdf_read.rs"]
mod pdf_read;

fn regular() -> Vec<u8> {
    let records = [(0u16, vec![0, 10, 2, 0, 0]), (65, vec![0, 8, 0, 10, 8, 5, 246, 2, 8, 2, 0, 0])];
    let mut bytes = b"AutoCAD-86 shapes 1.0\r\n\x1a".to_vec();
    for n in [0u16, 65, 2] {
        bytes.extend(n.to_le_bytes());
    }
    for (code, data) in &records {
        bytes.extend(code.to_le_bytes());
        bytes.extend((data.len() as u16).to_le_bytes());
    }
    for (_, data) in records {
        bytes.extend(data);
    }
    bytes
}
fn big() -> Vec<u8> {
    let records = [(0u16, vec![0, 20, 0, 0, 0]), (0xa4a4, vec![0, 8, 0, 20, 8, 20, 236, 2, 8, 5, 0, 0])];
    let mut bytes = b"AutoCAD-86 bigfont 1.0\r\n\x1a".to_vec();
    for n in [8u16, 2, 1, 0xa1, 0xfe] {
        bytes.extend(n.to_le_bytes());
    }
    let mut offset = bytes.len() + 16;
    for (code, data) in &records {
        bytes.extend(code.to_le_bytes());
        bytes.extend((data.len() as u16).to_le_bytes());
        bytes.extend((offset as u32).to_le_bytes());
        offset += data.len();
    }
    for (_, data) in records {
        bytes.extend(data);
    }
    bytes
}
#[test]
fn shx_style_drives_text_mtext_and_png_pdf_exports() {
    let dir = std::env::temp_dir().join(format!("cadcraft-shx-export-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let small = dir.join("synthetic.shx");
    let large = dir.join("synthetic-big.shx");
    std::fs::write(&small, regular()).unwrap();
    std::fs::write(&large, big()).unwrap();
    let mut d = Drawing::new_imperial();
    d.text_styles.push(TextStyle {
        name: "Synthetic".into(),
        font: small.to_string_lossy().into(),
        big_font: large.to_string_lossy().into(),
        width_factor: 0.8,
        oblique: 0.2,
        ..Default::default()
    });
    let t = Text {
        insert: Vec3::ZERO,
        align_pt: None,
        height: 10.,
        value: "A中".into(),
        rotation: 0.,
        width_factor: 0.8,
        oblique: 0.2,
        style: "Synthetic".into(),
        halign: Default::default(),
        valign: Default::default(),
    };
    let (shape, _) = cadcraft_render::place_text_entity(&d, &t, &t.value);
    assert!((shape.width - 15.6).abs() < 1e-9);
    assert_eq!(shape.strokes.len(), 4);
    assert!(shape.glyphs.is_empty());
    d.add(&Space::Model, Common::default(), EntityKind::Text(t)).unwrap();
    let m = cadcraft_doc::MText {
        insert: Vec3::new(0., 20., 0.),
        height: 10.,
        width: 0.,
        attach: 1,
        rotation: 0.,
        style: "Synthetic".into(),
        contents: "A中".into(),
        line_spacing: 1.,
        line_spacing_exact: false,
        columns: None,
    };
    let layout = cadcraft_render::layout_mtext_entity(&d, &m);
    assert!((layout.bounds.width() - 15.6).abs() < 1e-9);
    let inline = cadcraft_doc::MText { contents: format!("\\F{};A中", small.display()), ..m.clone() };
    assert!((cadcraft_render::layout_mtext_entity(&d, &inline).bounds.width() - 15.6).abs() < 1e-9);
    d.add(&Space::Model, Common::default(), EntityKind::MText(m)).unwrap();
    let png = cadcraft_io::png(&d, &Space::Model, 320, 240).unwrap();
    let pix = tiny_skia::Pixmap::decode_png(&png).unwrap();
    assert!(pix.pixels().windows(2).any(|p| p[0] != p[1]), "PNG must contain visible ink");
    let pdf = cadcraft_io::pdf(&d, &Space::Model, &cadcraft_io::PdfOptions::default()).unwrap();
    assert!(pdf.starts_with(b"%PDF-"));
    let doc = pdf_read::read(&pdf);
    assert!(pdf_read::drawn(doc.content()).iter().filter(|op| op.op == "l").count() >= 8, "PDF must contain both SHX text runs as vector strokes");
    std::fs::remove_dir_all(dir).unwrap();
}
