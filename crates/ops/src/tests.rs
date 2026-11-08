use std::sync::Arc;

use meshling_geom::{Size, Vec2};
use meshling_model::{ArtMesh, Blend, KeyformGrid, MeshKeyform, Meta, ObjId, Param, Part, Project, Provenance};

use super::*;

fn id(n: u128) -> ObjId {
    ObjId::from_u128(n)
}

fn part(n: u128, name: &str) -> Object {
    Object::Part(Part { meta: Meta::new(name).with_id(id(n)), parent: None, visible: true })
}

fn param(n: u128) -> Object {
    Object::Param(Param { meta: Meta::new("p").with_id(id(n)), min: -1.0, max: 1.0, default: 0.0 })
}

fn mesh(n: u128, part: u128, texture: u128) -> Object {
    let tri: Arc<[Vec2]> = vec![Vec2::ZERO, Vec2::X, Vec2::Y].into();
    Object::Mesh(ArtMesh {
        meta: Meta::new("m").with_id(id(n)),
        part: id(part),
        deformer: None,
        texture: id(texture),
        uvs: tri.clone(),
        indices: vec![0, 1, 2].into(),
        draw_order: 0,
        blend: Blend::Normal,
        clip_by: Vec::new(),
        keyforms: KeyformGrid::single(MeshKeyform { positions: tri, opacity: 1.0, draw_order: 0 }),
    })
}

fn agent() -> Provenance {
    Provenance::Agent { session: "s1".into() }
}

fn base() -> Project {
    let mut e = Edit::new(&Project::new(Size::new(8, 8)), "setup", Provenance::Human);
    e.insert(part(1, "root")).unwrap();
    e.insert(part(2, "head")).unwrap();
    e.finish().unwrap().0
}

#[test]
fn transactions_replay_and_invert_exactly() {
    let p0 = base();
    let mut e = Edit::new(&p0, "edit", agent());
    e.insert(param(10)).unwrap();
    e.update(id(2), |o| o.meta_mut().name = "face".into()).unwrap();
    e.insert_at(0, part(3, "first")).unwrap();
    e.remove(id(1)).unwrap();
    e.set_canvas(Size::new(16, 9)).unwrap();
    let (p1, tx) = e.finish().unwrap();

    assert_eq!(tx.apply(&p0).unwrap(), p1);
    assert_eq!(tx.inverse().apply(&p1).unwrap(), p0);
    assert_eq!(p1.parts.iter().map(|p| p.meta.name.as_str()).collect::<Vec<_>>(), ["first", "face"]);
    assert_eq!(p1.canvas, Size::new(16, 9));
    assert_eq!(tx.touched(), vec![id(10), id(2), id(3), id(1)]);
}

#[test]
fn edits_stamp_provenance_and_skip_no_ops() {
    let mut e = Edit::new(&base(), "rename", agent());
    e.update(id(2), |o| o.meta_mut().name = "head".into()).unwrap();
    e.set_canvas(Size::new(8, 8)).unwrap();
    let (_, tx) = e.clone().finish().unwrap();
    assert!(tx.is_empty(), "unchanged object and canvas record nothing");

    e.update(id(2), |o| o.meta_mut().tags.push("face".into())).unwrap();
    let (p, _) = e.finish().unwrap();
    assert_eq!(p.part(id(2)).unwrap().meta.provenance, agent());
    assert_eq!(p.part(id(1)).unwrap().meta.provenance, Provenance::Human);
}

#[test]
fn edits_refuse_new_structural_errors_but_tolerate_old_ones() {
    // A mesh whose texture is missing: an existing error that must not block unrelated edits.
    let mut broken = base();
    let Object::Mesh(m) = mesh(20, 1, 99) else { unreachable!() };
    broken.meshes.push(m);
    assert_eq!(broken.check().len(), 1);

    let mut e = Edit::new(&broken, "rename", agent());
    e.update(id(2), |o| o.meta_mut().name = "x".into()).unwrap();
    assert!(e.finish().is_ok());

    let mut e = Edit::new(&broken, "delete root", agent());
    e.remove(id(1)).unwrap();
    match e.finish() {
        Err(OpError::Structure(errors)) => assert_eq!(errors.len(), 1, "{errors:?}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn ops_check_identity_and_staleness() {
    let p = base();
    let mut e = Edit::new(&p, "x", agent());
    assert_eq!(e.insert(part(1, "again")), Err(OpError::DuplicateId(id(1))));
    assert_eq!(e.update(id(77), |_| {}), Err(OpError::NotFound(id(77))));
    assert!(matches!(e.update(id(1), |o| o.meta_mut().id = id(5)), Err(OpError::IdentityChanged(_))));
    assert!(matches!(e.update(id(1), |o| *o = param(1)), Err(OpError::IdentityChanged(_))));
    assert!(matches!(e.insert_at(9, part(4, "far")), Err(OpError::OutOfRange { .. })));

    let mut e = Edit::new(&p, "rename", agent());
    e.update(id(2), |o| o.meta_mut().name = "face".into()).unwrap();
    let (_, tx) = e.finish().unwrap();
    let mut moved_on = p.clone();
    moved_on.parts[1].meta.tags.push("someone else".into());
    assert_eq!(tx.apply(&moved_on), Err(OpError::Stale(id(2))));
}

#[test]
fn reorder_moves_and_undoes() {
    let mut e = Edit::new(&base(), "add", agent());
    e.insert(part(3, "c")).unwrap();
    let (p, _) = e.finish().unwrap();
    let mut e = Edit::new(&p, "move", agent());
    e.reorder(id(3), 0).unwrap();
    assert!(matches!(e.reorder(id(3), 3), Err(OpError::OutOfRange { .. })));
    let (moved, tx) = e.finish().unwrap();
    let names = |p: &Project| p.parts.iter().map(|q| q.meta.name.clone()).collect::<Vec<_>>();
    assert_eq!(names(&moved), ["c", "root", "head"]);
    assert_eq!(names(&tx.inverse().apply(&moved).unwrap()), ["root", "head", "c"]);
}

#[test]
fn history_undoes_redoes_and_forgets() {
    let mut h = History::new(2);
    let mut p = base();
    for (n, name) in [(3, "a"), (4, "b"), (5, "c")] {
        let mut e = Edit::new(&p, format!("add {name}"), agent());
        e.insert(part(n, name)).unwrap();
        let (next, tx) = e.finish().unwrap();
        h.record(tx);
        p = next;
    }
    assert_eq!(h.undo_labels().collect::<Vec<_>>(), ["add c", "add b"], "limit keeps the newest two");

    let (p1, tx) = h.undo(&p).unwrap().unwrap();
    assert_eq!(tx.label, "add c");
    let (p2, _) = h.undo(&p1).unwrap().unwrap();
    assert_eq!(p2.parts.len(), 3);
    assert!(h.undo(&p2).unwrap().is_none());

    let (p3, _) = h.redo(&p2).unwrap().unwrap();
    assert_eq!(p3, p1);
    assert_eq!(h.redo_labels().collect::<Vec<_>>(), ["add c"]);

    // A failed undo (project changed underneath) leaves both stacks as they were.
    assert!(h.undo(&p2).is_err());
    assert_eq!(h.undo_labels().collect::<Vec<_>>(), ["add b"]);
    assert!(h.can_redo());

    // New work drops the redo stack.
    let mut e = Edit::new(&p3, "add d", agent());
    e.insert(part(6, "d")).unwrap();
    h.record(e.finish().unwrap().1);
    assert!(!h.can_redo());

    h.record(Transaction { label: "nothing".into(), ops: Vec::new() });
    assert_eq!(h.undo_labels().next(), Some("add d"), "empty transactions are not recorded");
}

#[test]
fn objects_and_transactions_round_trip_through_json() {
    let mut e = Edit::new(&base(), "all kinds", agent());
    e.insert(param(10)).unwrap();
    e.insert(mesh(20, 1, 30)).unwrap();
    e.insert(Object::Texture(meshling_model::TextureAsset {
        meta: Meta::new("t").with_id(id(30)),
        size: Size::new(1, 1),
        content_hash: "00".into(),
    }))
    .unwrap();
    let (_, tx) = e.finish().unwrap();
    let json = serde_json::to_string(&tx).unwrap();
    assert_eq!(serde_json::from_str::<Transaction>(&json).unwrap(), tx);
}
