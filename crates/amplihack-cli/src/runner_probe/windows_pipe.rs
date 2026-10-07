//! Single-owner Windows anonymous pipe polling; no background reader to cancel.
use super::*;
use std::{ffi::c_void, os::windows::io::AsRawHandle};

#[link(name = "kernel32")]
unsafe extern "system" {
    fn PeekNamedPipe(
        pipe: *mut c_void,
        buffer: *mut c_void,
        size: u32,
        read: *mut u32,
        available: *mut u32,
        remaining: *mut u32,
    ) -> i32;
}

pub(super) struct Pipe<T> {
    stream: T,
    pub(super) bytes: Vec<u8>,
    pub(super) eof: bool,
}
impl<T: Read + AsRawHandle> Pipe<T> {
    pub(super) fn new(stream: T) -> Result<Self> {
        Ok(Self {
            stream,
            bytes: Vec::new(),
            eof: false,
        })
    }
    pub(super) fn drain(&mut self) -> Result<()> {
        for _ in 0..8 {
            let mut available = 0;
            // This module alone owns the read handle, with no concurrent read,
            // peek or other operation on it. Peek does not wait for pipe data.
            let ok = unsafe {
                PeekNamedPipe(
                    self.stream.as_raw_handle(),
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null_mut(),
                    &mut available,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() == Some(109) {
                    // ERROR_BROKEN_PIPE: all writers closed.
                    self.eof = true;
                    break;
                }
                return Err(error).context("capability pipe peek failed");
            }
            if available == 0 {
                break;
            }
            let mut chunk = [0; 8192];
            let count = (available as usize).min(chunk.len());
            // Read only bytes already present. Sole read ownership prevents
            // another reader consuming them between peek and read.
            match self.stream.read(&mut chunk[..count]) {
                Ok(0) => {
                    self.eof = true;
                    break;
                }
                Ok(n) => {
                    let retained = n.min((LIMIT + 1).saturating_sub(self.bytes.len()));
                    self.bytes.extend_from_slice(&chunk[..retained]);
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error).context("capability pipe read failed"),
            }
        }
        Ok(())
    }
}
