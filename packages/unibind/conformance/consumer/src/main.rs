//! Conformance consumer: proves the generated `unibind-conformance-client`
//! drives the engine cdylib correctly, without any cargo dependency edge on
//! the engine crate.
//!
//! Reads the engine library path from `UNIBIND_CONFORMANCE_ENGINE`, loads it
//! through the handshake, and asserts one behavior group per line: record
//! round trips, error variant mapping, an awaited async call (which
//! exercises the cross-ABI waker), and the Drop-cancellation witness.

use std::collections::HashMap;
use std::future::Future as _;
use std::path::PathBuf;
use std::pin::pin;
use std::process::ExitCode;
use std::task::{Context, Poll, Waker};

use unibind_conformance_client::{ConformanceError, Engine, Inner, Sample};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("conformance failed: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let path = std::env::var_os("UNIBIND_CONFORMANCE_ENGINE")
        .map(PathBuf::from)
        .ok_or("UNIBIND_CONFORMANCE_ENGINE is not set")?;
    let engine = Engine::load(&path).map_err(|error| format!("load: {error}"))?;
    println!("ok load + IR-hash handshake");

    records(&engine)?;
    println!("ok record round trip (map, option, vec, nested, awkward layout)");

    errors(&engine)?;
    println!("ok error variants map with Display text");

    async_call(&engine)?;
    println!("ok async delayed_double awaited across the ABI (waker path)");

    stream(&engine)?;
    println!("ok count_to stream collected across the ABI (waker path per item)");

    cancellation(&engine)?;
    println!("ok dropping the client future cancels the engine-side future");

    Ok(())
}

fn records(engine: &Engine) -> Result<(), String> {
    let sample = Sample {
        flag: true,
        id: 42,
        name: "answer".to_owned(),
        note: Some("of everything".to_owned()),
        values: vec![-1, 0, 7],
        weights: HashMap::from([("a".to_owned(), 1), ("b".to_owned(), -2)]),
        inner: Inner {
            label: "nested".to_owned(),
            ratio: 0.5,
        },
    };
    let echoed = engine.echo_record(sample.clone());
    if echoed != sample {
        return Err(format!("echo_record changed the record: {echoed:?}"));
    }
    let none_note = Sample {
        note: None,
        ..sample
    };
    let echoed = engine.echo_record(none_note.clone());
    if echoed != none_note {
        return Err(format!("echo_record changed the None note: {echoed:?}"));
    }
    let total = engine.sum(vec![1, 2, 3, 4]);
    if total != 10 {
        return Err(format!("sum([1,2,3,4]) = {total}, expected 10"));
    }
    Ok(())
}

fn errors(engine: &Engine) -> Result<(), String> {
    match engine.fail(0) {
        Err(ConformanceError::StoreGone { message }) if message == "store gone: kind 0" => {}
        other => return Err(format!("fail(0): expected StoreGone, got {other:?}")),
    }
    match engine.fail(1) {
        Err(ConformanceError::Invalid { message }) if message == "invalid input: kind 1" => {}
        other => return Err(format!("fail(1): expected Invalid, got {other:?}")),
    }
    match engine.fail(7) {
        Ok(7) => Ok(()),
        other => Err(format!("fail(7): expected Ok(7), got {other:?}")),
    }
}

fn async_call(engine: &Engine) -> Result<(), String> {
    let doubled = futures::executor::block_on(engine.delayed_double(21));
    if doubled == 42 {
        Ok(())
    } else {
        Err(format!("delayed_double(21) = {doubled}, expected 42"))
    }
}

fn stream(engine: &Engine) -> Result<(), String> {
    let collected: Vec<u64> =
        futures::executor::block_on(futures::StreamExt::collect(engine.count_to(5)));
    if collected == [0, 1, 2, 3, 4] {
        Ok(())
    } else {
        Err(format!("count_to(5) yielded {collected:?}, expected [0, 1, 2, 3, 4]"))
    }
}

fn cancellation(engine: &Engine) -> Result<(), String> {
    engine.reset_cancel_witness();
    let future = engine.hang_until_dropped();
    {
        let mut pinned = pin!(future);
        let mut context = Context::from_waker(Waker::noop());
        if let Poll::Ready(value) = pinned.as_mut().poll(&mut context) {
            return Err(format!("hang_until_dropped resolved early with {value}"));
        }
        if engine.cancel_witnessed() {
            return Err("witness fired before the future was dropped".to_owned());
        }
        // `pinned` (and with it the engine-side future) drops here.
    }
    if engine.cancel_witnessed() {
        Ok(())
    } else {
        Err("dropping the future did not cancel the engine-side future".to_owned())
    }
}
