//! `UniStream` drains in order and pulls items lazily, one per request.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use futures::executor::block_on;
use futures::StreamExt as _;
use unibind_runtime::UniStream;

#[test]
fn next_drains_a_stream_in_order() {
    let mut stream = UniStream::new(futures::stream::iter([1, 2, 3]));
    assert_eq!(block_on(stream.next()), Some(1));
    assert_eq!(block_on(stream.next()), Some(2));
    assert_eq!(block_on(stream.next()), Some(3));
    assert_eq!(block_on(stream.next()), None);
}

#[test]
fn items_are_produced_one_pull_at_a_time() {
    let produced = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&produced);
    let mut stream = UniStream::new(futures::stream::iter(0..).map(move |item: usize| {
        counter.fetch_add(1, Ordering::SeqCst);
        item
    }));
    assert_eq!(produced.load(Ordering::SeqCst), 0, "nothing runs before the first pull");
    assert_eq!(block_on(stream.next()), Some(0));
    assert_eq!(produced.load(Ordering::SeqCst), 1);
    assert_eq!(block_on(stream.next()), Some(1));
    assert_eq!(produced.load(Ordering::SeqCst), 2, "an unbounded source only advances per next()");
}

#[test]
fn unistream_is_itself_a_stream() {
    let stream = UniStream::new(futures::stream::iter(["a", "b"]));
    let items: Vec<&str> = block_on(stream.collect());
    assert_eq!(items, ["a", "b"]);
}
