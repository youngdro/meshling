use super::*;

fn id(n: u128) -> ObjId {
    ObjId::from_u128(n)
}

fn meta(n: u128, name: &str) -> Meta {
    Meta { id: id(n), name: name.into(), tags: vec![], provenance: Provenance::Human }
}

/// A face part with one warp deformer and one quad mesh bound to an `EyeLOpen` parameter.
fn sample() -> Project {
    let mut p = Project::new(Size::new(1024, 1024));
    p.textures.push(TextureAsset { meta: meta(1, "eye_L"), size: Size::new(64, 32), content_hash: "00".repeat(32) });
    p.parts.push(Part { meta: meta(2, "face"), parent: None, visible: true });
    p.params.push(Param { meta: meta(3, "EyeLOpen"), min: 0.0, max: 1.0, default: 1.0 });
    let lattice = |dy: f32| -> Arc<[Vec2]> {
        (0..9).map(|i| Vec2::new((i % 3) as f32 * 50.0, (i / 3) as f32 * 25.0 + dy)).collect()
    };
    p.deformers.push(Deformer {
        meta: meta(4, "eye_L_warp"),
        parent: None,
        kind: DeformerKind::Warp {
            rows: 2,
            cols: 2,
            keyforms: KeyformGrid {
                bindings: vec![ParamBinding { param: id(3), keys: vec![0.0, 1.0] }],
                forms: vec![
                    WarpKeyform { points: lattice(10.0), opacity: 1.0 },
                    WarpKeyform { points: lattice(0.0), opacity: 1.0 },
                ],
            },
        },
    });
    let quad: Arc<[Vec2]> = vec![Vec2::ZERO, Vec2::X, Vec2::ONE, Vec2::Y].into();
    let mut m = meta(5, "eye_L_white");
    m.tags = vec!["eye_L".into()];
    m.provenance = Provenance::Agent { session: "test".into() };
    p.meshes.push(ArtMesh {
        meta: m,
        part: id(2),
        deformer: Some(id(4)),
        texture: id(1),
        uvs: quad.clone(),
        indices: vec![0, 1, 2, 0, 2, 3].into(),
        draw_order: 500,
        blend: Blend::Normal,
        clip_by: vec![],
        keyforms: KeyformGrid::single(MeshKeyform { positions: quad, opacity: 1.0, draw_order: 0 }),
    });
    p
}

#[test]
fn sample_is_well_formed() {
    assert_eq!(sample().check(), vec![]);
}

#[test]
fn json_round_trip_is_byte_identical() {
    let p = sample();
    let text = p.to_json();
    let back = Project::from_json(&text).unwrap();
    assert_eq!(back, p);
    assert_eq!(back.to_json(), text);
}

#[test]
fn json_uses_camel_case_and_string_ids() {
    let text = sample().to_json();
    assert!(text.contains("\"formatVersion\": 1"));
    assert!(!text.contains("\"clipBy\""), "empty clipBy is omitted");
    assert!(text.contains("\"by\": \"agent\""));
    assert!(text.contains(&format!("\"{}\"", id(5))));
}

#[test]
fn rejects_newer_and_unversioned_files() {
    let newer = sample().to_json().replace("\"formatVersion\": 1", "\"formatVersion\": 99");
    assert!(matches!(Project::from_json(&newer), Err(LoadError::TooNew { found: 99, .. })));
    assert!(matches!(Project::from_json("{\"canvas\":{\"width\":1,\"height\":1}}"), Err(LoadError::MissingVersion)));
}

#[test]
fn keyform_grid_indexing_is_first_binding_fastest() {
    let g = KeyformGrid {
        bindings: vec![
            ParamBinding { param: id(1), keys: vec![0.0, 0.5, 1.0] },
            ParamBinding { param: id(2), keys: vec![-1.0, 1.0] },
        ],
        forms: (0..6).collect::<Vec<u32>>(),
    };
    assert_eq!(g.expected_len(), 6);
    assert_eq!(g.get(&[2, 0]), Some(&2));
    assert_eq!(g.get(&[0, 1]), Some(&3));
    assert_eq!(g.get(&[3, 0]), None);
    assert_eq!(g.get(&[0]), None);
    assert_eq!(KeyformGrid::single(7).expected_len(), 1);
}

#[test]
fn check_reports_structural_defects() {
    let mut p = sample();
    p.meshes[0].texture = id(99);
    p.meshes[0].indices = vec![0, 1, 7].into();
    p.params[0].default = 2.0;
    if let DeformerKind::Warp { keyforms, .. } = &mut p.deformers[0].kind {
        keyforms.forms.pop();
        keyforms.bindings[0].keys = vec![1.0, 0.0];
    }
    let errors = p.check();
    assert!(errors.contains(&StructureError::Dangling { owner: id(5), what: "texture", target: id(99) }));
    assert!(errors.contains(&StructureError::BadIndices { owner: id(5), vertices: 4 }));
    assert!(errors.contains(&StructureError::ParamRange(id(3))));
    assert!(errors.contains(&StructureError::GridSize { owner: id(4), expected: 2, found: 1 }));
    assert!(errors.contains(&StructureError::BadKeys { owner: id(4), param: id(3) }));
}

#[test]
fn check_finds_duplicate_ids_and_cycles() {
    let mut p = sample();
    p.parts.push(Part { meta: meta(6, "a"), parent: Some(id(7)), visible: true });
    p.parts.push(Part { meta: meta(7, "b"), parent: Some(id(6)), visible: true });
    p.params.push(Param { meta: meta(3, "dup"), min: 0.0, max: 1.0, default: 0.0 });
    let errors = p.check();
    assert!(errors.contains(&StructureError::DuplicateId(id(3))));
    assert!(errors.contains(&StructureError::PartCycle(id(6))));
    assert!(errors.contains(&StructureError::PartCycle(id(7))));
}

#[test]
fn ids_parse_from_their_display() {
    let a = ObjId::new();
    assert_eq!(a.to_string().parse::<ObjId>(), Ok(a));
    assert_eq!("not-an-id".parse::<ObjId>(), Err(ParseIdError));
}
