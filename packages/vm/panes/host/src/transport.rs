use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

pub enum Target {
    Unix(PathBuf),
    Tcp(String),
}

pub struct Stream {
    pub read: Box<dyn Read + Send>,
    pub write: Box<dyn Write + Send>,
    pub shutdown: ShutdownHandle,
}

pub enum ShutdownHandle {
    Unix(UnixStream),
    Tcp(TcpStream),
}

pub fn connect(target: &Target) -> std::io::Result<Stream> {
    match target {
        Target::Unix(path) => {
            let stream = UnixStream::connect(path)?;
            Ok(Stream {
                read: Box::new(stream.try_clone()?),
                shutdown: ShutdownHandle::Unix(stream.try_clone()?),
                write: Box::new(stream),
            })
        }
        Target::Tcp(addr) => {
            let stream = TcpStream::connect(addr.as_str())?;
            stream.set_nodelay(true)?;
            Ok(Stream {
                read: Box::new(stream.try_clone()?),
                shutdown: ShutdownHandle::Tcp(stream.try_clone()?),
                write: Box::new(stream),
            })
        }
    }
}

impl ShutdownHandle {
    pub fn shutdown(self) {
        match self {
            Self::Unix(stream) => {
                let _ = stream.shutdown(Shutdown::Both);
            }
            Self::Tcp(stream) => {
                let _ = stream.shutdown(Shutdown::Both);
            }
        }
    }
}
