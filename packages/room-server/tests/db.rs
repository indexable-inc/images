use room_server::db::{BackendUpsert, Db, Message, ThreadUpsert};

fn open_db() -> Db {
    let tmp = tempfile::Builder::new()
        .prefix("room-db-test")
        .tempdir()
        .expect("tempdir");
    let path = tmp.path().join("room.db");
    std::mem::forget(tmp);
    let db = Db::open(&path).expect("open db");
    db.migrate().expect("migrate");
    db
}

fn user_message(id: &str, thread_id: &str, text: &str, images: Vec<String>) -> Message {
    Message {
        id: id.to_owned(),
        thread_id: thread_id.to_owned(),
        ts_ms: 2_000,
        role: "user".to_owned(),
        kind: "user_prompt".to_owned(),
        text: Some(text.to_owned()),
        tool_name: None,
        tool_use_id: None,
        tool_input: None,
        result: None,
        patch: None,
        images,
    }
}

#[test]
fn empty_user_text_does_not_clear_existing_preview() {
    let db = open_db();
    db.upsert_thread(&ThreadUpsert {
        id: "thread-1".to_owned(),
        user: "alice".to_owned(),
        host: "alice-dev".to_owned(),
        repo: None,
        branch: None,
        cwd: None,
        workspace_root: None,
        base_sha: None,
        model: None,
        reasoning_effort: None,
        approval_policy: None,
        permission_profile: None,
        title_if_empty: Some("(image attachment)".to_owned()),
        status: Some("active".to_owned()),
        now_ms: 1_000,
        preview: Some("(image attachment)".to_owned()),
    })
    .unwrap();

    db.insert_message(&user_message(
        "msg-1",
        "thread-1",
        "",
        vec!["data:image/png;base64,AA==".to_owned()],
    ))
    .unwrap();
    let thread = db.get_thread("thread-1").unwrap().unwrap();
    assert_eq!(thread.preview, "(image attachment)");

    db.insert_message(&user_message("msg-2", "thread-1", "follow-up", Vec::new()))
        .unwrap();
    let thread = db.get_thread("thread-1").unwrap().unwrap();
    assert_eq!(thread.preview, "follow-up");
}

#[test]
fn insert_message_needs_a_thread_but_ensure_thread_provides_one() {
    let db = open_db();

    // An engine-opened thread (the `/api/agent/turns` path) is never
    // seeded by the chat path, so a streamed item would fail the
    // messages -> threads foreign key on a bare insert.
    let assistant = Message {
        kind: "assistant_text".to_owned(),
        role: "assistant".to_owned(),
        ..user_message("item-1", "engine-thread", "hello from codex", Vec::new())
    };
    assert!(
        db.insert_message(&assistant).is_err(),
        "message for a missing thread should violate the FK"
    );

    // ensure_thread seeds the minimal row the FK needs; the same insert
    // then lands and is listable.
    db.ensure_thread("engine-thread", 1_500).unwrap();
    db.insert_message(&assistant).unwrap();
    let messages = db.list_messages("engine-thread", 50).unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].text.as_deref(), Some("hello from codex"));

    // A later richer upsert keeps the row and its messages (ensure_thread
    // is INSERT OR IGNORE, not a clobber).
    db.ensure_thread("engine-thread", 9_999).unwrap();
    assert_eq!(db.list_messages("engine-thread", 50).unwrap().len(), 1);
}

#[test]
fn backend_registry_lists_only_active_backends() {
    let db = open_db();
    db.upsert_backend(&BackendUpsert {
        id: "symphony:run:node".to_owned(),
        name: "ENG-1 / implement".to_owned(),
        url: "http://10.0.0.2:8080".to_owned(),
        source: "symphony".to_owned(),
        run_id: Some("run".to_owned()),
        node_id: Some("node".to_owned()),
        vm_name: Some("sym-run-node".to_owned()),
        runtime: Some("ixvm".to_owned()),
        status: "active".to_owned(),
        now_ms: 1_000,
    })
    .unwrap();

    let backends = db.list_backends().unwrap();
    assert_eq!(backends.len(), 1);
    assert_eq!(backends[0].id, "symphony:run:node");
    assert_eq!(backends[0].url, "http://10.0.0.2:8080");
    assert_eq!(backends[0].runtime.as_deref(), Some("ixvm"));

    db.delete_backend("symphony:run:node", 2_000).unwrap();
    assert!(db.list_backends().unwrap().is_empty());
}

#[test]
fn host_backend_persists_runtime_without_vm() {
    let db = open_db();
    db.upsert_backend(&BackendUpsert {
        id: "symphony:run:host".to_owned(),
        name: "ENG-2 / implement".to_owned(),
        url: "http://127.0.0.1:18080".to_owned(),
        source: "symphony".to_owned(),
        run_id: Some("run".to_owned()),
        node_id: Some("host".to_owned()),
        vm_name: None,
        runtime: Some("host".to_owned()),
        status: "active".to_owned(),
        now_ms: 1_000,
    })
    .unwrap();

    let backend = db.get_backend("symphony:run:host").unwrap().unwrap();
    assert_eq!(backend.runtime.as_deref(), Some("host"));
    assert_eq!(backend.vm_name, None);
}
