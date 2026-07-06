//! Pure-Rust behavior of the runtime and stream primitives; the BEAM-bound
//! plumbing (`spawn_reply`, `spawn_stream`) needs a live node and is
//! exercised by the phase 5 stage 2 conformance project.

#[test]
fn runtime_is_one_shared_instance() {
    assert!(std::ptr::eq(
        unibind_ex_runtime::runtime(),
        unibind_ex_runtime::runtime()
    ));
}

#[test]
fn from_iter_yields_items_in_order() {
    let mut stream = unibind_ex_runtime::Stream::from_iterator(vec![1_u64, 2, 3]);
    let items = unibind_ex_runtime::runtime().block_on(async move {
        let mut items = Vec::new();
        while let Some(item) = stream.next().await {
            items.push(item);
        }
        items
    });
    assert_eq!(items, vec![1, 2, 3]);
}

#[test]
fn new_wraps_an_existing_stream() {
    let inner = unibind_ex_runtime::Stream::from_iterator(["a".to_owned()]);
    let mut stream = unibind_ex_runtime::Stream::new(inner.0);
    let first = unibind_ex_runtime::runtime().block_on(async move { stream.next().await });
    assert_eq!(first.as_deref(), Some("a"));
}
