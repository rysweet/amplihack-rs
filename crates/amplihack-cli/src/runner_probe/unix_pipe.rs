//! Single-owner nonblocking Unix capability pipes.
use super::*;
pub(super) struct Pipe<T> {
    stream: T,
    pub(super) bytes: Vec<u8>,
    pub(super) eof: bool,
}
impl<T: Read + std::os::fd::AsRawFd> Pipe<T> {
    pub(super) fn new(stream: T) -> Result<Self> {
        let fd = stream.as_raw_fd();
        #[cfg(test)]
        let fd = if testing::fault(testing::Fault::Setup) {
            -1
        } else {
            fd
        };
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        ensure!(
            flags >= 0 && unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } >= 0,
            "capability pipe nonblocking setup failed"
        );
        Ok(Self {
            stream,
            bytes: Vec::new(),
            eof: false,
        })
    }
    pub(super) fn drain(&mut self) -> Result<()> {
        // A fixed quantum keeps continuous writers from starving the deadline or other pipe.
        for _ in 0..8 {
            let mut chunk = [0; 8192];
            #[cfg(test)]
            testing::read()?;
            match self.stream.read(&mut chunk) {
                Ok(0) => {
                    self.eof = true;
                    break;
                }
                Ok(n) => {
                    let retained = n.min((LIMIT + 1).saturating_sub(self.bytes.len()));
                    self.bytes.extend_from_slice(&chunk[..retained]);
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => anyhow::bail!("capability pipe read failed"),
            }
        }
        Ok(())
    }
}
