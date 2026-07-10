//! Reopenable polling for overlays backed by an external data source.

use std::fmt::Display;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

/// Define a domain-typed wrapper around [`spawn_data_watcher`]. The macro owns
/// only the repeated public adapter signature; the watcher behavior remains in
/// the ordinary composable function below.
#[macro_export]
macro_rules! define_data_watcher {
    ($value:ty, $poll:expr, $label:literal, $open:path, $version:path, $read:path) => {
        pub fn spawn_watcher<F>(path: std::path::PathBuf, sink: F)
        where
            F: FnMut($value) -> bool + Send + 'static,
        {
            $crate::watch::spawn_data_watcher(
                path, $poll, $label, $open, $version, $read, sink,
            );
        }
    };
}

/// Poll a versioned data source on a background thread and emit each new value.
///
/// A failed poll drops the connection so the next tick reopens it. A failed
/// reopen is retried quietly, while the initial open and connected operations
/// are reported with `label` so each overlay retains a useful error prefix.
pub fn spawn_data_watcher<C, T, E, Open, Version, Read, Sink>(
    path: PathBuf,
    poll: Duration,
    label: &'static str,
    open: Open,
    version: Version,
    read: Read,
    mut sink: Sink,
) where
    C: Send + 'static,
    T: Send + 'static,
    E: Display + Send + 'static,
    Open: Fn(&Path) -> Result<C, E> + Send + 'static,
    Version: Fn(&C) -> Result<i64, E> + Send + 'static,
    Read: Fn(&C) -> Result<T, E> + Send + 'static,
    Sink: FnMut(T) -> bool + Send + 'static,
{
    thread::spawn(move || {
        let mut connection = match open(&path) {
            Ok(connection) => Some(connection),
            Err(error) => {
                eprintln!("{label}: failed to open {}: {error}", path.display());
                None
            }
        };
        let mut last_version = None;

        loop {
            match connection.as_ref() {
                Some(current) => match version(current) {
                    Ok(current_version) if Some(current_version) != last_version => {
                        last_version = Some(current_version);
                        match read(current) {
                            Ok(value) => {
                                if !sink(value) {
                                    return;
                                }
                            }
                            Err(error) => eprintln!("{label}: read failed: {error}"),
                        }
                    }
                    Ok(_) => {}
                    Err(error) => {
                        eprintln!("{label}: poll failed, reopening: {error}");
                        connection = None;
                        last_version = None;
                    }
                },
                None => connection = open(&path).ok(),
            }
            thread::sleep(poll);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emits_the_first_version_and_stops_when_the_sink_closes() {
        let (sender, receiver) = std::sync::mpsc::channel();
        spawn_data_watcher(
            PathBuf::from("unused"),
            Duration::ZERO,
            "test-watcher",
            |_| Ok::<_, std::io::Error>(()),
            |_| Ok(1),
            |_| Ok(42),
            move |value| {
                sender.send(value).expect("receiver remains connected");
                false
            },
        );

        assert_eq!(
            receiver.recv_timeout(Duration::from_secs(1)),
            Ok(42),
            "the watcher must emit immediately instead of waiting for a change"
        );
    }
}
