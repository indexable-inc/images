//! Packet ids and serverbound builders for the states the bot traverses.
//!
//! Ids are per-state ordinals; these are transcribed from the packet
//! registry of the pinned Minestom (net.minestom.server.network.packet
//! .`PacketRegistry`, Minecraft 26.2 = protocol 776, the same pin as
//! packages/minecraft/minestom/servers/spleef). A protocol bump reshuffles
//! them, so they live in one table here rather than scattered as literals.

use md5::{Digest, Md5};
use mc_protocol::varint::write_varint;
use mc_protocol::wire::write_string;

/// Clientbound (server → bot) packet ids.
pub mod clientbound {
    pub mod login {
        pub const DISCONNECT: i32 = 0x00;
        pub const ENCRYPTION_REQUEST: i32 = 0x01;
        pub const LOGIN_SUCCESS: i32 = 0x02;
        pub const SET_COMPRESSION: i32 = 0x03;
        pub const PLUGIN_REQUEST: i32 = 0x04;
    }

    pub mod config {
        pub const DISCONNECT: i32 = 0x02;
        pub const FINISH_CONFIGURATION: i32 = 0x03;
        pub const KEEP_ALIVE: i32 = 0x04;
        pub const PING: i32 = 0x05;
        pub const SELECT_KNOWN_PACKS: i32 = 0x0E;
    }

    pub mod play {
        pub const DISCONNECT: i32 = 0x20;
        pub const KEEP_ALIVE: i32 = 0x2C;
        pub const PING: i32 = 0x3D;
    }
}

/// Serverbound (bot → server) packet ids.
pub mod serverbound {
    pub mod login {
        pub const LOGIN_START: i32 = 0x00;
        pub const PLUGIN_RESPONSE: i32 = 0x02;
        pub const LOGIN_ACKNOWLEDGED: i32 = 0x03;
    }

    pub mod config {
        pub const FINISH_CONFIGURATION: i32 = 0x03;
        pub const KEEP_ALIVE: i32 = 0x04;
        pub const PONG: i32 = 0x05;
        pub const SELECT_KNOWN_PACKS: i32 = 0x07;
    }

    pub mod play {
        pub const KEEP_ALIVE: i32 = 0x1C;
        pub const PONG: i32 = 0x2D;
    }
}

/// A packet in its logical form: `VarInt` id + body.
pub fn packet(id: i32, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len() + 1);
    write_varint(&mut out, id);
    out.extend_from_slice(body);
    out
}

/// The handshake that moves the connection into the login state.
pub fn handshake(protocol_version: i32, host: &str, port: u16) -> Vec<u8> {
    let mut body = Vec::new();
    write_varint(&mut body, protocol_version);
    write_string(&mut body, host);
    body.extend_from_slice(&port.to_be_bytes());
    write_varint(&mut body, 2); // next state: login
    packet(0x00, &body)
}

/// Login start: username + the profile id the server should book us under.
pub fn login_start(username: &str, profile_id: [u8; 16]) -> Vec<u8> {
    let mut body = Vec::new();
    write_string(&mut body, username);
    body.extend_from_slice(&profile_id);
    packet(serverbound::login::LOGIN_START, &body)
}

/// Declines a login plugin request (`message_id` echoed, successful = false)
/// — the vanilla answer to a channel the client does not speak.
pub fn login_plugin_declined(message_id: i32) -> Vec<u8> {
    let mut body = Vec::new();
    write_varint(&mut body, message_id);
    body.push(0); // successful: false
    packet(serverbound::login::PLUGIN_RESPONSE, &body)
}

pub fn login_acknowledged() -> Vec<u8> {
    packet(serverbound::login::LOGIN_ACKNOWLEDGED, &[])
}

/// Answers `SelectKnownPacks` with an empty list. Honest — the bot bundles
/// no data packs — and it makes the server inline the full registry data,
/// so the recorded replay is self-contained.
pub fn select_known_packs_none() -> Vec<u8> {
    let mut body = Vec::new();
    write_varint(&mut body, 0); // zero known packs
    packet(serverbound::config::SELECT_KNOWN_PACKS, &body)
}

pub fn finish_configuration_ack() -> Vec<u8> {
    packet(serverbound::config::FINISH_CONFIGURATION, &[])
}

/// Offline-mode profile id, matching vanilla's
/// `UUID.nameUUIDFromBytes(("OfflinePlayer:" + name).getBytes(UTF_8))`:
/// MD5 of the prefixed name with the RFC 4122 version-3/variant bits set.
/// Offline servers take the client's word for its identity (Minestom copies
/// this uuid into the `GameProfile` verbatim), so matching vanilla keeps the
/// recorded session indistinguishable from a real client's.
pub fn offline_profile_id(username: &str) -> [u8; 16] {
    let digest = Md5::digest(format!("OfflinePlayer:{username}").as_bytes());
    let mut bytes: [u8; 16] = digest.into();
    bytes[6] = (bytes[6] & 0x0F) | 0x30; // version 3 (name-based, MD5)
    bytes[8] = (bytes[8] & 0x3F) | 0x80; // IETF variant
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handshake_wire_format() {
        let bytes = handshake(776, "localhost", 25565);
        let mut expected = vec![
            0x00, // packet id
            0x88, 0x06, // protocol 776
            9,    // host length
        ];
        expected.extend_from_slice(b"localhost");
        expected.extend_from_slice(&[0x63, 0xDD]); // port 25565
        expected.push(2); // next state: login
        assert_eq!(bytes, expected);
    }

    #[test]
    fn login_start_wire_format() {
        let id = [0xAB; 16];
        let bytes = login_start("bot", id);
        let mut expected = vec![0x00, 3];
        expected.extend_from_slice(b"bot");
        expected.extend_from_slice(&id);
        assert_eq!(bytes, expected);
    }

    #[test]
    fn known_offline_uuid() {
        // java.util.UUID.nameUUIDFromBytes("OfflinePlayer:Notch".getBytes())
        // == b50ad385-829d-3141-a216-7e7d7539ba7f.
        let bytes = offline_profile_id("Notch");
        assert_eq!(
            bytes,
            [
                0xB5, 0x0A, 0xD3, 0x85, 0x82, 0x9D, 0x31, 0x41, 0xA2, 0x16, 0x7E, 0x7D, 0x75,
                0x39, 0xBA, 0x7F
            ]
        );
    }

    #[test]
    fn empty_bodies_are_bare_ids() {
        assert_eq!(login_acknowledged(), [0x03]);
        assert_eq!(finish_configuration_ack(), [0x03]);
        assert_eq!(select_known_packs_none(), [0x07, 0x00]);
    }
}
