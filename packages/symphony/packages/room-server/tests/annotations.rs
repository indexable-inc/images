// End-to-end coverage of the Loro → SQL annotation mirror.
//
// Drives a `LoroDoc` the way a browser peer would (one root LoroMap
// per message id, JSON-encoded reviewer-note values) and asserts the
// `message_annotations` table tracks the doc through add / remove /
// noop frames. Catches regressions in the wire shape and in the
// cache invalidation behavior at the same time.

use loro::LoroDoc;
use room_server::{annotations::AnnotationMirror, db::Db};
use serde_json::json;

fn open_db() -> Db {
    let tmp = tempfile::Builder::new()
        .prefix("room-anno-test")
        .tempdir()
        .expect("tempdir");
    let path = tmp.path().join("room.db");
    std::mem::forget(tmp);
    let db = Db::open(&path).expect("open db");
    db.migrate().expect("migrate");
    db
}

fn write_annotation(doc: &LoroDoc, message_id: &str, id: &str, author: &str, text: &str) {
    let map = doc.get_map(format!("annotations:{message_id}"));
    let payload = json!({
        "author_id": author,
        "author_name": author,
        "ts_ms": 12_345_i64,
        "text": text,
    })
    .to_string();
    map.insert(id, payload).expect("insert annotation");
    doc.commit();
}

fn delete_annotation(doc: &LoroDoc, message_id: &str, id: &str) {
    let map = doc.get_map(format!("annotations:{message_id}"));
    map.delete(id).expect("delete annotation");
    doc.commit();
}

#[test]
fn mirror_tracks_loro_doc_through_add_remove() {
    let mut db = open_db();
    let doc = LoroDoc::new();
    let mut mirror = AnnotationMirror::new();

    // Empty doc → empty mirror.
    mirror.sync(&doc, &mut db).unwrap();
    assert!(db.list_annotations(100).unwrap().is_empty());

    // One note on msg-1.
    write_annotation(&doc, "msg-1", "a1", "alice", "be terser");
    mirror.sync(&doc, &mut db).unwrap();
    let rows = db.list_annotations(100).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].message_id, "msg-1");
    assert_eq!(rows[0].id, "a1");
    assert_eq!(rows[0].text, "be terser");

    // Add another note on a different message; both should mirror.
    write_annotation(&doc, "msg-2", "b1", "bob", "wrong tool");
    mirror.sync(&doc, &mut db).unwrap();
    let rows = db.list_annotations(100).unwrap();
    assert_eq!(rows.len(), 2);

    // Delete msg-1's note; mirror should drop the row.
    delete_annotation(&doc, "msg-1", "a1");
    mirror.sync(&doc, &mut db).unwrap();
    let rows = db.list_annotations(100).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].message_id, "msg-2");
    assert_eq!(rows[0].id, "b1");

    // Re-sync with no changes is a no-op (we don't have a hook to
    // assert "no SQL writes", but the row set must stay identical).
    mirror.sync(&doc, &mut db).unwrap();
    let rows_again = db.list_annotations(100).unwrap();
    assert_eq!(rows_again.len(), 1);
    assert_eq!(rows_again[0].id, "b1");
}
