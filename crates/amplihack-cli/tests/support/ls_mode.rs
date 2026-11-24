//! Issue #2931 (HOSTTOOL): the kind, permission bits and owner of a file, read
//! from `ls -ldn <path>` run by a stub.
//!
//! A stub shell script that must look at a file which the test removes after
//! the run cannot call `stat -c` (GNU) or `stat -f` (BSD): the two disagree.
//! `ls -ldn` has one format on both, fixed by POSIX, and Rust does the parsing
//! here. The stub records the raw line; the test calls [`parse_ls_line`].
//!
//! Contract of [`parse_ls_line`], pinned by the tests below:
//!
//! - the first character is the kind: `-` regular, `d` directory, `l` symlink,
//!   `p` fifo, `c`, `b` and `s` (devices and sockets) [`LsKind::Other`], and
//!   any other character is a malformed line;
//! - the next nine characters are the owner, group and other triplets, with
//!   `s`/`S` in the owner and group execute positions meaning setuid and
//!   setgid, and `t`/`T` in the last position meaning sticky (a capital letter
//!   is the bit set without execute). The result's mode keeps all twelve bits,
//!   so a permission assertion cannot lose a setuid, setgid or sticky bit;
//! - one character after the nine is read past when it is `@` (an extended
//!   attribute, which macOS shows) or `.` (an SELinux context, which GNU `ls`
//!   shows). Where `ls` prints `+` for an ACL the line is refused; where it
//!   prints nothing for one, an ACL on a 0600 file passes, as it did with
//!   `stat`;
//! - the third whitespace-separated field is the numeric owner (`-n`);
//! - a line that does not have this shape gives `None`, which an assertion
//!   built on it must treat as a failure and never as "no extra bits".
//!
//! This file uses only `std`.

/// The file type character of an `ls -l` line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LsKind {
    Regular,
    Directory,
    Symlink,
    Fifo,
    Other,
}

/// What [`parse_ls_line`] reads from one `ls -ldn` line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LsEntry {
    pub kind: LsKind,
    /// Twelve bits: setuid, setgid and sticky above the nine permission bits.
    pub mode: u32,
    pub uid: u32,
}

/// The kind, mode and numeric owner of the file an `ls -ldn` line describes,
/// or `None` when the line does not have that shape (module documentation).
pub fn parse_ls_line(line: &str) -> Option<LsEntry> {
    let mut fields = line.split_whitespace();
    let mode_field = fields.next()?;
    let _links = fields.next()?;
    let uid = fields.next()?.parse::<u32>().ok()?;
    let chars: Vec<char> = mode_field.chars().collect();
    // The kind, nine permission characters, and at most one marker after them.
    if chars.len() < 10 || chars.len() > 11 {
        return None;
    }
    // `@` (macOS: extended attributes) and `.` (GNU: an SELinux context) say
    // nothing about who may read the file. `+` (an ACL) does, and is refused.
    if chars.len() == 11 && !matches!(chars[10], '@' | '.') {
        return None;
    }
    let kind = match chars[0] {
        '-' => LsKind::Regular,
        'd' => LsKind::Directory,
        'l' => LsKind::Symlink,
        'p' => LsKind::Fifo,
        'c' | 'b' | 's' => LsKind::Other,
        _ => return None,
    };
    let mut mode = 0u32;
    for (index, &c) in chars[1..10].iter().enumerate() {
        let bit = 0o400 >> index;
        // Which special bit this execute position can carry, and its letter.
        let special = match index {
            2 => Some((0o4000, 's', 'S')),
            5 => Some((0o2000, 's', 'S')),
            8 => Some((0o1000, 't', 'T')),
            _ => None,
        };
        let plain = match index % 3 {
            0 => 'r',
            1 => 'w',
            _ => 'x',
        };
        match (c, special) {
            ('-', _) => {}
            (c, _) if c == plain => mode |= bit,
            (c, Some((extra, with_execute, _))) if c == with_execute => mode |= bit | extra,
            (c, Some((extra, _, without_execute))) if c == without_execute => mode |= extra,
            _ => return None,
        }
    }
    Some(LsEntry { kind, mode, uid })
}

#[cfg(test)]
mod tests {
    use super::{LsEntry, LsKind, parse_ls_line};

    fn line(mode: &str) -> String {
        format!("{mode}  1 501  20  64 Nov 21 10:00 /some/where/file")
    }

    #[test]
    fn each_kind_and_its_permissions_are_read() {
        let cases: [(&str, LsKind, u32); 6] = [
            ("-rw-------", LsKind::Regular, 0o600),
            ("drwx------", LsKind::Directory, 0o700),
            ("lrwxr-xr-x", LsKind::Symlink, 0o755),
            ("prw-------", LsKind::Fifo, 0o600),
            ("crw-rw-rw-", LsKind::Other, 0o666),
            ("-r--r--r--", LsKind::Regular, 0o444),
        ];
        for (text, kind, mode) in cases {
            assert_eq!(
                parse_ls_line(&line(text)),
                Some(LsEntry {
                    kind,
                    mode,
                    uid: 501
                }),
                "{text}"
            );
        }
    }

    #[test]
    fn setuid_setgid_and_sticky_stay_visible() {
        let cases: [(&str, u32); 6] = [
            ("-rwsr-xr-x", 0o4755),
            ("-rwSr--r--", 0o4644),
            ("-rwxr-sr-x", 0o2755),
            ("-rwxr-Sr--", 0o2744),
            ("drwxrwxrwt", 0o1777),
            ("drwxrwxrwT", 0o1776),
        ];
        for (text, mode) in cases {
            let entry = parse_ls_line(&line(text)).unwrap_or_else(|| panic!("{text}"));
            assert_eq!(entry.mode, mode, "{text}");
        }
    }

    #[test]
    fn a_trailing_extended_attribute_marker_is_read_past() {
        let entry = parse_ls_line(&line("-rw-------@")).expect("a line with @");
        assert_eq!(entry.mode, 0o600);
        assert_eq!(entry.uid, 501);
    }

    #[test]
    fn the_owner_is_the_third_field() {
        let entry = parse_ls_line("-rw-------  1 0  0  3 Jan  1 00:00 f").unwrap();
        assert_eq!(entry.uid, 0);
    }

    #[test]
    fn a_line_of_another_shape_gives_nothing() {
        for malformed in [
            "",
            "total 0",
            "-rw 1 501 20 0 Nov 21 10:00 f",
            "?rw------- 1 501 20 0 Nov 21 10:00 f",
            "-rw------- 1 someone 20 0 Nov 21 10:00 f",
            "-rwxzwxrwx 1 501 20 0 Nov 21 10:00 f",
        ] {
            assert_eq!(parse_ls_line(malformed), None, "{malformed:?}");
        }
    }
}
