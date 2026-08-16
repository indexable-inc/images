//! Publish one `cat` terminal to the ix dashboard discovery directory and
//! stay alive: the smallest live producer, used to exercise the full
//! browser-to-PTY loop by hand.
//!
//! Run the aggregator (`cargo run -p dashboard`), then:
//!
//! ```sh
//! cargo run -p tui --example publish_cat --features dashboard,publish
//! ```
//!
//! Typing into the pane from a browser (the `send` input) reaches `cat`
//! through the socket's return channel and echoes on the published screen.

// Auto-discovered, deliberately absent from the manifest: the repo's
// nix cargo-unit planner builds from a source set without `examples/`, so a
// `[[example]]` entry (the usual `required-features` spelling) fails its
// manifest parse. The cfg gate below is the same guard by other means: the
// example is a no-op unless built with the features it needs.
#[cfg(all(feature = "dashboard", feature = "publish"))]
fn main() {
    use std::io::Write as _;
    use std::sync::Arc;
    use std::time::Duration;

    let manager = Arc::new(tui::TuiManager::new());
    let term = manager
        .spawn("cat".into(), vec![], tui::SpawnConfig::default())
        .expect("spawn cat");
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    let publisher = runtime
        .block_on(tui::publish(
            &manager,
            tui::socket_path(),
            Duration::from_millis(50),
        ))
        .expect("publish");
    // Machine-readable so a driving script can address the pane.
    println!("PRODUCER={}", publisher.producer_id());
    println!("PANE={}", term.id);
    std::io::stdout().flush().expect("flush");
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}

#[cfg(not(all(feature = "dashboard", feature = "publish")))]
fn main() {
    eprintln!("publish_cat needs --features dashboard,publish");
}
