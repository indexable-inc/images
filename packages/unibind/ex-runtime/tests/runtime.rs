//! Pure-Rust behavior of the runtime primitive; the BEAM-bound plumbing
//! (`spawn_reply`, `spawn_stream`) needs a live node and is exercised by
//! the phase 5 conformance project (packages/unibind/conformance-ex).

#[test]
fn runtime_is_one_shared_instance() {
    assert!(std::ptr::eq(
        unibind_ex_runtime::runtime(),
        unibind_ex_runtime::runtime()
    ));
}

#[test]
fn runtime_drives_a_unistream() {
    let mut stream = unibind_runtime::UniStream::new(futures::stream::iter([1_u64, 2, 3]));
    let items = unibind_ex_runtime::runtime().block_on(async move {
        let mut items = Vec::new();
        while let Some(item) = stream.next().await {
            items.push(item);
        }
        items
    });
    assert_eq!(items, vec![1, 2, 3]);
}
