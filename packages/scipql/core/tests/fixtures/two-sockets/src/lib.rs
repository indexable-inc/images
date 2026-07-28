pub mod net;
pub mod mock;

pub fn open() -> net::Socket {
    net::Socket { fd: 3 }
}
