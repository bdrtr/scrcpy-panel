use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::time::Duration;

/// Read exactly `n` bytes from a stream, retrying on partial reads
pub fn recv_all(stream: &mut TcpStream, buf: &mut [u8]) -> io::Result<()> {
    let mut total = 0;
    while total < buf.len() {
        let n = stream.read(&mut buf[total..])?;
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "connection closed"));
        }
        total += n;
    }
    Ok(())
}

/// Send all bytes to a stream
pub fn send_all(stream: &mut TcpStream, buf: &[u8]) -> io::Result<()> {
    stream.write_all(buf)
}

/// Connect to localhost with retry logic
pub fn connect_with_retry(port: u16, attempts: u32, delay: Duration) -> io::Result<TcpStream> {
    let addr = format!("127.0.0.1:{}", port);
    for i in 0..attempts {
        log::debug!("Connection attempt {}/{} to {}", i + 1, attempts, addr);
        match TcpStream::connect(&addr) {
            Ok(stream) => {
                // Read dummy byte (for forward connections)
                let mut byte = [0u8; 1];
                match stream.peek(&mut byte) {
                    Ok(1) => {
                        let mut s = stream;
                        s.read_exact(&mut byte)?;
                        return Ok(s);
                    }
                    _ => {
                        // Server not ready yet
                    }
                }
            }
            Err(_) => {}
        }
        if i + 1 < attempts {
            std::thread::sleep(delay);
        }
    }
    Err(io::Error::new(io::ErrorKind::ConnectionRefused, "all connection attempts failed"))
}
