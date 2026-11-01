use super::*;
fn word(out: &mut Vec<u8>, n: u16) {
    out.extend(n.to_le_bytes());
}
fn fixture(kind: Kind, entries: &[(u16, &[u8])]) -> Vec<u8> {
    let label = match kind {
        Kind::Shapes => "shapes",
        Kind::Unicode => "unifont",
        Kind::BigFont => "bigfont",
    };
    let mut out = format!("AutoCAD-86 {label} 1.0\r\n\x1a").into_bytes();
    let info: &[u8] = &[10, 2, 0, 0, 0, 0];
    let records: Vec<_> = std::iter::once((0, info))
        .chain(entries.iter().copied())
        .map(|(code, data)| {
            let mut v = vec![0];
            v.extend(data);
            (code, v)
        })
        .collect();
    match kind {
        Kind::Shapes => {
            word(&mut out, 0);
            word(&mut out, 65535);
            word(&mut out, records.len() as u16);
            for (code, data) in &records {
                word(&mut out, *code);
                word(&mut out, data.len() as u16);
            }
            for (_, data) in records {
                out.extend(data);
            }
        }
        Kind::BigFont => {
            word(&mut out, 8);
            word(&mut out, records.len() as u16);
            word(&mut out, 1);
            word(&mut out, 0x81);
            word(&mut out, 0xfe);
            let mut offset = out.len() + 8 * records.len();
            for (code, data) in &records {
                word(&mut out, *code);
                word(&mut out, data.len() as u16);
                out.extend((offset as u32).to_le_bytes());
                offset += data.len();
            }
            for (_, data) in records {
                out.extend(data);
            }
        }
        Kind::Unicode => {
            out.extend([0; 6]);
            out.push(0);
            out.extend(info);
            for (code, data) in records.into_iter().skip(1) {
                word(&mut out, code);
                word(&mut out, data.len() as u16);
                out.extend(data);
            }
        }
    }
    out
}
fn font(code: &[u8]) -> Font {
    Font::parse(&fixture(Kind::Shapes, &[(65, code)])).unwrap()
}
#[test]
fn all_containers_and_advances() {
    for kind in [Kind::Shapes, Kind::Unicode, Kind::BigFont] {
        let f = Font::parse(&fixture(kind, &[(65, &[8, 7, 0, 0])])).unwrap();
        assert_eq!(f.above, 10.);
        assert_eq!(f.below, 2.);
        assert_eq!(f.glyph(65).unwrap().width, 7.);
    }
}
#[test]
fn pen_stack_scale_and_vertical_skip() {
    let g = font(&[5, 4, 2, 8, 3, 0, 3, 2, 6, 2, 14, 9, 99, 99, 0, 0, 8, 7, 0, 0]).glyph(65).unwrap();
    assert_eq!(g.width, 7.);
    assert_eq!(g.strokes, vec![vec![Vec2::ZERO, Vec2::new(6., 0.)]]);
}
#[test]
fn vectors_use_half_components_not_unit_circle() {
    let g = font(&[0x21, 0]).glyph(65).unwrap();
    assert_eq!(g.width, 2.);
    assert_eq!(g.strokes[0][1], Vec2::new(2., 1.));
}
#[test]
fn arcs_have_expected_endpoints() {
    let g = font(&[10, 10, 2, 0]).glyph(65).unwrap();
    assert!((g.width + 10.).abs() < 1e-9);
    assert!((g.strokes.last().unwrap()[1].y - 10.).abs() < 1e-9);
    let g = font(&[12, 10, 0, 127, 0]).glyph(65).unwrap();
    assert_eq!(g.width, 10.);
    assert!(g.strokes.iter().flatten().any(|p| p.y < -4.9));
}
#[test]
fn fractional_arc_uses_fractional_octants() {
    let g = font(&[11, 56, 28, 0, 3, 0x12, 0]).glyph(65).unwrap();
    let start = (1. + 56. / 256.) * std::f64::consts::FRAC_PI_4;
    let end = (2. + 28. / 256.) * std::f64::consts::FRAC_PI_4;
    assert!((g.width - 3. * (end.cos() - start.cos())).abs() < 1e-9);
}
#[test]
fn unicode_subshape_has_two_byte_id() {
    let f = Font::parse(&fixture(Kind::Unicode, &[(65, &[7, 0x34, 0x12, 0]), (0x1234, &[8, 9, 0, 0])])).unwrap();
    assert_eq!(f.glyph(65).unwrap().width, 9.);
}
#[test]
fn malformed_programs_are_bounded() {
    for p in [&[6, 0][..], &[3, 0, 0], &[7, 65, 0], &[5, 5, 5, 5, 5, 0], &[9, 1, 2], &[14, 14, 0]] {
        assert!(font(p).glyph(65).is_err());
    }
    for n in 0..80 {
        let bytes = vec![255; n];
        assert!(Font::parse(&bytes).is_err());
    }
    let valid = fixture(Kind::BigFont, &[(65, &[0])]);
    for n in 0..valid.len() {
        assert!(Font::parse(&valid[..n]).is_err());
    }
}
#[test]
fn big5_and_gb_are_explicit_and_escape_ranges_are_honored() {
    let regular = Arc::new(font(&[2, 8, 7, 0, 0]));
    for (encoding, code) in [(Encoding::Big5, 0xa4a4), (Encoding::Gb, 0xd6d0)] {
        let big = Arc::new(Font::parse(&fixture(Kind::BigFont, &[(code, &[8, 12, 0, 0])])).unwrap());
        let pair = Pair { regular: regular.clone(), big: Some(big), encoding };
        let shaped = pair.shape("A中", 20., 0.8, 0.);
        assert!((shaped.width - 30.4).abs() < 1e-9);
        assert_eq!(crate::text_width(&crate::TextFont::Shx(pair), "A中", 20., 0.8), shaped.width);
    }
}
#[test]
fn oblique_changes_ink_without_changing_advance() {
    let pair = Pair { regular: Arc::new(font(&[8, 0, 10, 2, 8, 7, 246, 0])), big: None, encoding: Encoding::Big5 };
    let a = pair.shape("A", 10., 2., 0.);
    let b = pair.shape("A", 10., 2., 0.5_f64.atan());
    assert_eq!(a.width, b.width);
    assert!((b.strokes[0][1].x - 5.).abs() < 1e-9);
}

#[test]
fn extended_bigfont_primitive_restores_baseline_and_scales_axes() {
    let mut f = Font::parse(&fixture(Kind::BigFont, &[(65, &[7, 0, 0x34, 0x12, 2, 3, 5, 20, 2, 8, 12, 0, 0]), (0x1234, &[8, 10, 10, 0])])).unwrap();
    f.shapes.insert(0, vec![10, 0, 2, 10, 0]);
    let g = f.glyph(65).unwrap();
    assert_eq!(g.width, 12.);
    assert_eq!(g.strokes, vec![vec![Vec2::new(2., 3.), Vec2::new(7., 23.)]]);
}
#[test]
fn mtext_uses_shx_spaces_and_width_factor() {
    let regular = Arc::new(Font::parse(&fixture(Kind::Shapes, &[(65, &[8, 7, 0, 0]), (32, &[2, 8, 3, 0, 0])])).unwrap());
    let font = crate::TextFont::Shx(Pair { regular, big: None, encoding: Encoding::Big5 });
    assert_eq!(crate::space_width(&font, 10., 2.), 6.);
    let params = crate::MTextParams { font, width_factor: 2., height: 10., width: 0., ..crate::MTextParams::new(Vec2::ZERO, 10.) };
    let layout = crate::layout_mtext_with("A A", &params);
    assert!(!layout.pieces.is_empty());
    assert_eq!(crate::text_width(&params.font, "A A", 10., 2.), 34.);
}

#[test]
fn resolver_matches_case_and_reloads_after_path_change() {
    let dir = std::env::temp_dir().join(format!("cadcraft-shx-resolver-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("SYNTHETIC.SHX"), fixture(Kind::Shapes, &[(65, &[8, 7, 0, 0])])).unwrap();
    let before = font_path();
    set_font_path(&dir.to_string_lossy());
    assert_eq!(find("synthetic.shx").unwrap().glyph(65).unwrap().width, 7.);
    std::fs::write(dir.join("SYNTHETIC.SHX"), fixture(Kind::Shapes, &[(65, &[8, 9, 0, 0])])).unwrap();
    set_font_path(&dir.to_string_lossy());
    assert_eq!(find("synthetic.shx").unwrap().glyph(65).unwrap().width, 9.);
    set_font_path(&before);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn hostile_bytecode_does_not_panic() {
    let mut seed = 123u32;
    for _ in 0..512 {
        let mut program = Vec::new();
        for _ in 0..64 {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            program.push(seed as u8);
        }
        let _ = font(&program).glyph(65);
    }
}

#[test]
fn ascii_bigfont_escape_pair_consumes_two_characters() {
    let mut big = Font::parse(&fixture(Kind::BigFont, &[(0x7c41, &[8, 12, 0, 0])])).unwrap();
    big.ranges = vec![(124, 124)];
    let pair = Pair { regular: Arc::new(font(&[8, 7, 0, 0])), big: Some(Arc::new(big)), encoding: Encoding::Big5 };
    assert_eq!(pair.shape("|AA", 10., 1., 0.).width, 19.);
}

#[test]
fn compiled_clockwise_arc_uses_direction_bit() {
    let g = font(&[10, 10, 0x82, 0]).glyph(65).unwrap();
    assert!((g.width + 10.).abs() < 1e-9);
    assert!((g.strokes.last().unwrap()[1].y + 10.).abs() < 1e-9);
    let g = font(&[11, 56, 28, 0, 3, 0x92, 0]).glyph(65).unwrap();
    let start = (1. - 56. / 256.) * std::f64::consts::FRAC_PI_4;
    let end = -(28. / 256.) * std::f64::consts::FRAC_PI_4;
    let p = g.strokes.last().unwrap()[1];
    assert!((p.x - 3. * (end.cos() - start.cos())).abs() < 1e-9);
    assert!((p.y - 3. * (end.sin() - start.sin())).abs() < 1e-9);
}
#[test]
fn legacy_special_symbols_use_documented_records() {
    let regular = Arc::new(
        Font::parse(&fixture(
            Kind::Shapes,
            &[
                (127, &[8, 99, 0, 0]),
                (128, &[8, 99, 0, 0]),
                (129, &[8, 99, 0, 0]),
                (256, &[8, 3, 0, 0]),
                (257, &[8, 5, 0, 0]),
                (258, &[8, 7, 0, 0]),
                (176, &[8, 99, 0, 0]),
                (177, &[8, 99, 0, 0]),
                (216, &[8, 99, 0, 0]),
            ],
        ))
        .unwrap(),
    );
    let pair = Pair { regular, big: None, encoding: Encoding::Big5 };
    assert_eq!(pair.shape("%%d%%p%%c", 10., 1., 0.).width, 15.);
    let regular = Arc::new(
        Font::parse(&fixture(
            Kind::Unicode,
            &[
                (176, &[8, 3, 0, 0]),
                (177, &[8, 5, 0, 0]),
                (0x2205, &[8, 7, 0, 0]),
                (127, &[8, 99, 0, 0]),
                (128, &[8, 99, 0, 0]),
                (129, &[8, 99, 0, 0]),
                (256, &[8, 99, 0, 0]),
            ],
        ))
        .unwrap(),
    );
    let pair = Pair { regular, big: None, encoding: Encoding::Big5 };
    assert_eq!(pair.shape("%%d%%p%%c", 10., 1., 0.).width, 15.);
}
#[test]
fn legacy_special_symbols_use_127_129() {
    let regular = Arc::new(
        Font::parse(&fixture(
            Kind::Shapes,
            &[(127, &[8, 3, 0, 0]), (128, &[8, 5, 0, 0]), (129, &[8, 7, 0, 0]), (176, &[8, 99, 0, 0]), (177, &[8, 99, 0, 0]), (216, &[8, 99, 0, 0])],
        ))
        .unwrap(),
    );
    let pair = Pair { regular, big: None, encoding: Encoding::Big5 };
    for (text, width) in [("%%d", 3.), ("%%p", 5.), ("%%c", 7.)] {
        let actual = pair.shape(text, 10., 1., 0.);
        assert_eq!(actual.width, width, "{text}");
        assert_eq!(actual.strokes.len(), 1, "{text}");
    }
}
#[test]
fn rtl_fallback_keeps_whole_line_order_and_context() {
    let pair = Pair { regular: Arc::new(font(&[8, 7, 0, 0])), big: None, encoding: Encoding::Big5 };
    for text in ["אבג A", "سلام A", "A\u{200f}B"] {
        let actual = pair.shape(text, 10., 0.8, 0.2);
        let expected = crate::shape_stroke(text, 10., 0.8, 0.2);
        assert_eq!(actual.width, expected.width);
        assert_eq!(actual.strokes, expected.strokes);
    }
}

#[test]
fn unavailable_escape_glyph_preserves_both_characters() {
    let regular = Arc::new(Font::parse(&fixture(Kind::Shapes, &[(124, &[8, 3, 0, 0]), (90, &[8, 7, 0, 0])])).unwrap());
    for entries in [vec![], vec![(0x7c5a, &[6, 0][..])]] {
        let mut big = Font::parse(&fixture(Kind::BigFont, &entries)).unwrap();
        big.ranges = vec![(124, 124)];
        let pair = Pair { regular: regular.clone(), big: Some(Arc::new(big)), encoding: Encoding::Big5 };
        assert_eq!(pair.shape("|Z", 10., 1., 0.).width, 10.);
    }
}
#[test]
fn metadata_cannot_be_executed_or_redefined() {
    for kind in [Kind::Shapes, Kind::Unicode, Kind::BigFont] {
        let f = Font::parse(&fixture(kind, &[(65, &[7, 0, 0, 0, 0, 0, 1, 1, 0])])).unwrap();
        assert!(!f.contains(0));
        assert!(f.glyph(0).is_err());
        assert!(f.glyph(65).is_err());
        assert!(Font::parse(&fixture(kind, &[(0, &[20, 0, 0, 0, 0, 0])])).is_err());
    }
}
#[test]
fn legacy_shape_numbers_do_not_alias_unicode() {
    let regular = Arc::new(Font::parse(&fixture(Kind::Shapes, &[(0x4e2d, &[8, 99, 0, 0])])).unwrap());
    let pair = Pair { regular, big: None, encoding: Encoding::Big5 };
    assert!(pair.glyph('中').is_none());
    let regular = Arc::new(Font::parse(&fixture(Kind::BigFont, &[(0x96fb, &[8, 99, 0, 0]), (0xeb8a, &[8, 7, 0, 0])])).unwrap());
    let pair = Pair { regular, big: None, encoding: Encoding::Gb };
    // GBK encodes U+96FB as EB8A, not its Unicode scalar 96FB.
    assert_eq!(pair.shape("電", 10., 1., 0.).width, 7.);
}
#[test]
fn invalid_arc_radii_and_reverse_fractional_spans_are_rejected() {
    for p in [&[10, 0, 0, 0][..], &[11, 200, 50, 0, 10, 1, 0], &[11, 200, 50, 0, 10, 0x81, 0]] {
        assert!(font(p).glyph(65).is_err());
    }
}
#[test]
fn invalid_width_uses_positive_default_for_shx_and_fallback() {
    let pair = Pair { regular: Arc::new(font(&[8, 7, 0, 0])), big: None, encoding: Encoding::Big5 };
    for text in ["A", "AB", "אב A"] {
        let expected = pair.shape(text, 10., 1., 0.);
        for wf in [-1., 0., f64::NAN, f64::INFINITY] {
            let actual = pair.shape(text, 10., wf, 0.);
            assert_eq!(actual.width, expected.width);
            assert_eq!(actual.strokes, expected.strokes);
        }
    }
}

#[test]
fn contiguous_fallback_retains_spacing_controls_and_decorations() {
    let pair = Pair { regular: Arc::new(font(&[8, 7, 0, 0])), big: None, encoding: Encoding::Big5 };
    for text in ["BB", "BB%%uB", "%%uBB%%u", "%%%%%%d", "%%oB%%uB"] {
        let actual = pair.shape(text, 6., 1., 0.);
        let expected = crate::shape_stroke(text, 6., 1., 0.);
        assert_eq!(actual.width, expected.width, "{text}");
        assert_eq!(actual.strokes, expected.strokes, "{text}");
    }
    assert!((pair.shape("ABBA", 6., 1., 0.).width - (4.2 + 9.6 + 4.2)).abs() < 1e-9);
}

#[test]
fn fallback_breaks_shx_decoration_spans() {
    let pair = Pair { regular: Arc::new(font(&[8, 7, 0, 0])), big: None, encoding: Encoding::Big5 };
    for (control, y) in [("%%u", -2.), ("%%o", 12.)] {
        let text = format!("{control}A{control}B{control}A");
        let actual = pair.shape(&text, 10., 1., 0.);
        let decoration: Vec<_> = actual.strokes.iter().filter(|st| st.iter().all(|p| p.y == y)).cloned().collect();
        assert_eq!(
            decoration,
            vec![vec![Vec2::new(0., y), Vec2::new(7., y)], vec![Vec2::new(actual.width - 7., y), Vec2::new(actual.width, y)],],
            "{control}"
        );
    }
}
