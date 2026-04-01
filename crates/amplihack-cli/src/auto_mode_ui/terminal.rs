//! Terminal raw-mode handling and key input for auto-mode UI.

use std::io::{self, Write};
#[cfg(unix)]
use std::os::fd::AsRawFd;
use std::time::Duration;

use anyhow::{Result, bail};

#[cfg(unix)]
pub(super) struct AutoModeTerminalGuard {
    fd: i32,
    original: Option<libc::termios>,
}

#[cfg(unix)]
impl AutoModeTerminalGuard {
    pub(super) fn activate() -> Result<Self> {
        let fd = io::stdin().as_raw_fd();
        let mut original = std::mem::MaybeUninit::<libc::termios>::uninit();
        if unsafe { libc::tcgetattr(fd, original.as_mut_ptr()) } != 0 {
            bail!("failed to read terminal attributes");
        }
        let original = unsafe { original.assume_init() };
        let mut raw = original;
        raw.c_lflag &= !(libc::ICANON | libc::ECHO);
        raw.c_cc[libc::VMIN] = 0;
        raw.c_cc[libc::VTIME] = 0;
        if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw) } != 0 {
            bail!("failed to enable auto-mode raw mode");
        }

        print!("\x1b[?1049h\x1b[?25l");
        io::stdout().flush()?;

        Ok(Self {
            fd,
            original: Some(original),
        })
    }
}

#[cfg(unix)]
impl Drop for AutoModeTerminalGuard {
    fn drop(&mut self) {
        if let Some(original) = self.original.take() {
            let _ = unsafe { libc::tcsetattr(self.fd, libc::TCSANOW, &original) };
        }
        let _ = io::stdout().write_all(b"\x1b[?25h\x1b[?1049l");
        let _ = io::stdout().flush();
    }
}

#[cfg(not(unix))]
pub(super) struct AutoModeTerminalGuard;

#[cfg(not(unix))]
impl AutoModeTerminalGuard {
    pub(super) fn activate() -> Result<Self> {
        Ok(Self)
    }
}

#[cfg(not(unix))]
impl Drop for AutoModeTerminalGuard {
    fn drop(&mut self) {}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AutoModeKey {
    Char(char),
    Left,
    Right,
    Up,
    Down,
}

pub(super) fn decode_auto_mode_key_bytes(bytes: &[u8]) -> Option<AutoModeKey> {
    match bytes {
        [byte] => Some(AutoModeKey::Char(*byte as char)),
        [0x1b, b'[', b'A'] => Some(AutoModeKey::Up),
        [0x1b, b'[', b'B'] => Some(AutoModeKey::Down),
        [0x1b, b'[', b'C'] => Some(AutoModeKey::Right),
        [0x1b, b'[', b'D'] => Some(AutoModeKey::Left),
        _ => None,
    }
}

#[cfg(unix)]
pub(super) fn read_auto_mode_key(timeout: Duration) -> Option<AutoModeKey> {
    let fd = io::stdin().as_raw_fd();
    let timeout_ms = timeout.as_millis().min(i32::MAX as u128) as i32;
    let mut poll_fd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    let ready = unsafe { libc::poll(&mut poll_fd, 1, timeout_ms) };
    if ready <= 0 || (poll_fd.revents & libc::POLLIN) == 0 {
        return None;
    }

    let mut first = [0u8; 1];
    if unsafe { libc::read(fd, first.as_mut_ptr().cast(), 1) } != 1 {
        return None;
    }
    if first[0] != 0x1b {
        return decode_auto_mode_key_bytes(&first);
    }

    let mut bytes = vec![first[0]];
    for _ in 0..2 {
        let mut extra_poll = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let ready = unsafe { libc::poll(&mut extra_poll, 1, 5) };
        if ready <= 0 || (extra_poll.revents & libc::POLLIN) == 0 {
            break;
        }
        let mut next = [0u8; 1];
        if unsafe { libc::read(fd, next.as_mut_ptr().cast(), 1) } != 1 {
            break;
        }
        bytes.push(next[0]);
    }

    decode_auto_mode_key_bytes(&bytes)
}

#[cfg(not(unix))]
pub(super) fn read_auto_mode_key(timeout: Duration) -> Option<AutoModeKey> {
    std::thread::sleep(timeout);
    None
}
