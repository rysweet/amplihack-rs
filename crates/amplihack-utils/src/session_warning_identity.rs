//! Kernel lifecycle identity; PID reuse must not inherit another process cursor.
use std::io;

#[cfg(target_os = "macos")]
pub fn birth(pid: u32) -> io::Result<String> {
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::uninit();
    let size = std::mem::size_of::<libc::proc_bsdinfo>();
    // SAFETY: the kernel writes at most size bytes into the correctly sized
    // buffer. It is read only when the entire structure was returned.
    let read = unsafe {
        libc::proc_pidinfo(
            pid as i32,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size as i32,
        )
    };
    if read != size as i32 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the successful kernel call initialized the entire structure.
    let info = unsafe { info.assume_init() };
    // SAFETY: geteuid has no preconditions.
    if info.pbi_uid != unsafe { libc::geteuid() } || info.pbi_pid != pid {
        return Err(io::Error::other(
            "session process identity is not owned by this user",
        ));
    }
    Ok(format!(
        "{}-{}",
        info.pbi_start_tvsec, info.pbi_start_tvusec
    ))
}
#[cfg(target_os = "linux")]
pub fn birth(pid: u32) -> io::Result<String> {
    use std::os::unix::fs::MetadataExt;
    let path = format!("/proc/{pid}/stat");
    let meta = std::fs::metadata(&path)?;
    // SAFETY: geteuid has no preconditions.
    if meta.uid() != unsafe { libc::geteuid() } {
        return Err(io::Error::other(
            "session process is not owned by this user",
        ));
    }
    let raw = std::fs::read_to_string(path)?;
    let (_, tail) = raw
        .rsplit_once(')')
        .ok_or_else(|| io::Error::other("invalid process stat"))?;
    tail.split_whitespace()
        .nth(19)
        .filter(|s| s.bytes().all(|b| b.is_ascii_digit()))
        .map(str::to_owned)
        .ok_or_else(|| io::Error::other("process start identity missing"))
}
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn birth(_pid: u32) -> io::Result<String> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "session warning identity requires Linux or macOS",
    ))
}
