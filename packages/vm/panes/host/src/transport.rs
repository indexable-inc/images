use std::io::{Read, Write};
use std::net::TcpStream;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

pub enum Target {
    Unix(PathBuf),
    Tcp(String),
}

pub struct Stream {
    pub read: Box<dyn Read + Send>,
    pub write: Box<dyn Write + Send>,
}

pub fn connect(target: &Target) -> std::io::Result<Stream> {
    match target {
        Target::Unix(path) => split(UnixStream::connect(path)?, UnixStream::try_clone),
        Target::Tcp(addr) => {
            let stream = TcpStream::connect(addr.as_str())?;
            stream.set_nodelay(true)?;
            split(stream, TcpStream::try_clone)
        }
    }
}

fn split<S>(stream: S, clone: impl FnOnce(&S) -> std::io::Result<S>) -> std::io::Result<Stream>
where
    S: Read + Write + Send + 'static,
{
    let read = clone(&stream)?;
    Ok(Stream {
        read: Box::new(read),
        write: Box::new(stream),
    })
}
