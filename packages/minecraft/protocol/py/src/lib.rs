//! Python bindings for `mc-protocol`, declared through `unibind`.
//!
//! The boundary is three synchronous calls: [`_mc_protocol::parse_address`]
//! splits `host[:port]`, [`_mc_protocol::status`] runs the full Server List
//! Ping exchange, and [`_mc_protocol::strip_format_codes`] normalizes MOTD
//! text for comparison. All protocol logic lives in the core crate; the
//! exported module only converts at the boundary, and `unibind` renders the
//! `pyo3` glue (function wrappers, record classes, the exception hierarchy,
//! and the module registration) from these declarations.

// clone:ignore-file -- deliberately parallel to ../../jvm/src/lib.rs: each
// backend's boundary must be declared inside its own exported module in its
// own crate (see the `backends(py)` note below), so the two declarations are
// kept in lockstep by hand until unibind exports one module to several
// backends per crate.

// `backends(py)`: a whole-workspace build unifies unibind's backend
// features across consumers, so pin this crate's glue to the backend whose
// runtime deps it declares.
#[unibind::export(backends(py))]
mod _mc_protocol {
    use std::time::Duration;

    /// A parsed `host[:port]` pair for a Java Edition server.
    #[unibind::record]
    #[derive(Clone)]
    pub struct ServerAddress {
        pub host: String,
        pub port: u16,
    }

    /// A parsed status response plus the measured ping round-trip.
    #[unibind::record]
    #[derive(Clone)]
    pub struct SlpStatus {
        /// Display name of the server version, e.g. `"26.1.2"`.
        pub version_name: String,
        /// Numeric protocol version, e.g. 775 for Minecraft 26.1.2.
        pub protocol_version: i32,
        pub players_online: i64,
        pub players_max: i64,
        /// MOTD flattened to text, legacy format codes kept verbatim (strip
        /// with `strip_format_codes`).
        pub motd: String,
        /// The full status JSON, for consumers needing fields beyond the
        /// basics.
        pub raw_json: String,
        /// Round-trip time of the ping/pong packet pair, in seconds.
        pub latency_seconds: f64,
    }

    /// Everything the boundary raises. Python sees `SlpError` (an `OSError`,
    /// the family socket-level failures raise anyway) with one subclass per
    /// failure stage.
    #[unibind::error(py(base = "OSError"))]
    #[derive(Debug)]
    pub enum SlpError {
        /// The address string or timeout value could not be used.
        #[unibind(py(name = "InvalidInputError"))]
        Input { message: String },
        /// Resolving, connecting, or socket I/O failed.
        #[unibind(py(name = "NetworkError"))]
        Network { message: String },
        /// The server answered with something that is not a valid status
        /// exchange.
        #[unibind(py(name = "ProtocolError"))]
        Protocol { message: String },
    }

    impl std::fmt::Display for SlpError {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            let (Self::Input { message }
            | Self::Network { message }
            | Self::Protocol { message }) = self;
            formatter.write_str(message)
        }
    }

    impl std::error::Error for SlpError {}

    /// Sort a core I/O error from the ping exchange into the boundary's
    /// exception classes: malformed peer data is `Protocol`, everything else
    /// (resolution, connect, timeout, reset) is `Network`.
    fn classify(error: &std::io::Error) -> SlpError {
        let message = error.to_string();
        if error.kind() == std::io::ErrorKind::InvalidData {
            SlpError::Protocol { message }
        } else {
            SlpError::Network { message }
        }
    }

    fn parse(address: &str) -> Result<mc_protocol::ServerAddress, SlpError> {
        mc_protocol::ServerAddress::parse(address).map_err(|error| SlpError::Input {
            message: error.to_string(),
        })
    }

    /// Parse `host[:port]`, defaulting to the standard Minecraft port 25565.
    /// Bare IPv6 addresses need brackets to carry a port (`[::1]:25565`).
    pub fn parse_address(address: &str) -> Result<ServerAddress, SlpError> {
        let parsed = parse(address)?;
        Ok(ServerAddress {
            host: parsed.host,
            port: parsed.port,
        })
    }

    /// Perform a full Server List Ping against `address` (`host[:port]`):
    /// connect, handshake, status request, status response, ping/pong.
    ///
    /// `timeout_seconds` bounds the TCP connect and each read/write
    /// individually.
    pub fn status(
        address: &str,
        #[unibind(default = 5.0)] timeout_seconds: f64,
    ) -> Result<SlpStatus, SlpError> {
        let parsed = parse(address)?;
        let timeout = Duration::try_from_secs_f64(timeout_seconds).map_err(|error| {
            SlpError::Input {
                message: format!("invalid timeout {timeout_seconds}: {error}"),
            }
        })?;
        let status = mc_protocol::query(&parsed, timeout).map_err(|error| classify(&error))?;
        Ok(SlpStatus {
            version_name: status.version_name,
            protocol_version: status.protocol_version,
            players_online: status.players_online,
            players_max: status.players_max,
            motd: status.motd,
            raw_json: status.raw_json,
            latency_seconds: status.latency.as_secs_f64(),
        })
    }

    /// Return `text` with every legacy format code (`§` or `&` followed by a
    /// color/style character) removed.
    pub fn strip_format_codes(text: &str) -> String {
        mc_protocol::strip_format_codes(text)
    }
}
