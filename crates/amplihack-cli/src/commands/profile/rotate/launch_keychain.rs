//! Read-only launch storage matching Claude Code 2.1.322's I6/k1 convention:
//! `Claude Code[-custom-oauth]-credentials-<8 hex SHA-256 of NFC directory>`.
//! No Keychain prompt, write, cached credential or exported credential file.
#[cfg(debug_assertions)]
use super::super::LOGIN_UNTRUSTED;
use super::super::UNAVAILABLE;
use crate::commands::profile::auto_switch::kit::euid;
#[cfg(debug_assertions)]
use crate::commands::profile::auto_switch::kit::read_bounded_nofollow;
use sha2::{Digest, Sha256};
use std::ffi::{c_char, c_void};
#[cfg(debug_assertions)]
use std::os::unix::fs::MetadataExt;
use std::path::Path;

/// All fields are sensitive process-local continuity, never serialised.
#[derive(Clone, PartialEq, Eq)]
pub(in crate::commands::profile) struct Snapshot {
    credentials: Vec<u8>,
    continuity: Vec<u8>,
}
impl Snapshot {
    pub fn credentials(&self) -> &[u8] {
        &self.credentials
    }
}

pub(super) fn read(dir: &Path) -> Result<Option<Snapshot>, &'static str> {
    #[cfg(debug_assertions)]
    if let Some(path) = std::env::var_os("AMPLIHACK_TEST_KEYCHAIN_STORE") {
        return injected(dir, Path::new(&path));
    }
    // Unit readers use explicitly synthetic identities and isolated files.
    // They must never query a developer's login Keychain.
    if cfg!(test) {
        return Ok(None);
    }
    // Changing HOME does not isolate the login Keychain. An isolated process
    // must explicitly inject storage and must never fall through to the host.
    if std::env::var_os("AMPLIHACK_TEST_HOME_GUARD").is_some() {
        return Err(UNAVAILABLE);
    }
    native(dir)
}

#[cfg(debug_assertions)]
fn injected(dir: &Path, path: &Path) -> Result<Option<Snapshot>, &'static str> {
    let (bytes, meta) = read_bounded_nofollow(path, 1024 * 1024)
        .map_err(|_| LOGIN_UNTRUSTED)?
        .ok_or(LOGIN_UNTRUSTED)?;
    if meta.uid() != euid() || meta.mode() & 0o077 != 0 || meta.nlink() != 1 {
        return Err(LOGIN_UNTRUSTED);
    }
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| LOGIN_UNTRUSTED)?;
    if value["version"] != 1 || !value["entries"].is_object() {
        return Err(LOGIN_UNTRUSTED);
    }
    let Some(entry) = value["entries"].get(dir.to_str().ok_or(LOGIN_UNTRUSTED)?) else {
        return Ok(None);
    };
    let revision = entry["continuity"]
        .as_str()
        .filter(|v| !v.is_empty())
        .ok_or(UNAVAILABLE)?;
    let credentials = entry.get("credentials").ok_or(UNAVAILABLE)?;
    Ok(Some(Snapshot {
        credentials: serde_json::to_vec(credentials).map_err(|_| UNAVAILABLE)?,
        continuity: revision.as_bytes().to_vec(),
    }))
}

type Ref = *const c_void;
const UTF8: u32 = 0x08000100;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFStringCreateWithBytes(
        alloc: Ref,
        bytes: *const u8,
        len: isize,
        encoding: u32,
        external: bool,
    ) -> Ref;
    fn CFStringCreateMutableCopy(alloc: Ref, capacity: isize, source: Ref) -> Ref;
    fn CFStringNormalize(string: Ref, form: isize);
    fn CFStringGetLength(string: Ref) -> isize;
    fn CFStringGetMaximumSizeForEncoding(len: isize, encoding: u32) -> isize;
    fn CFStringGetCString(string: Ref, buffer: *mut c_char, size: isize, encoding: u32) -> bool;
    fn CFDictionaryCreateMutable(alloc: Ref, capacity: isize, keys: Ref, values: Ref) -> Ref;
    fn CFDictionarySetValue(dict: Ref, key: Ref, value: Ref);
    fn CFDataGetLength(data: Ref) -> isize;
    fn CFDataGetBytePtr(data: Ref) -> *const u8;
    fn CFDataGetTypeID() -> usize;
    fn CFGetTypeID(object: Ref) -> usize;
    fn CFRelease(object: Ref);
    static kCFBooleanTrue: Ref;
}
#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    fn SecItemCopyMatching(query: Ref, result: *mut Ref) -> i32;
    static kSecClass: Ref;
    static kSecClassGenericPassword: Ref;
    static kSecAttrService: Ref;
    static kSecAttrAccount: Ref;
    static kSecReturnData: Ref;
    static kSecUseAuthenticationUI: Ref;
    static kSecUseAuthenticationUIFail: Ref;
}

struct Owned(Ref);
impl Owned {
    fn new(ptr: Ref) -> Result<Self, &'static str> {
        if ptr.is_null() {
            Err(UNAVAILABLE)
        } else {
            Ok(Self(ptr))
        }
    }
    fn string(value: &str) -> Result<Self, &'static str> {
        // SAFETY: the byte slice lives through this copying constructor.
        Self::new(unsafe {
            CFStringCreateWithBytes(
                std::ptr::null(),
                value.as_ptr(),
                value.len() as isize,
                UTF8,
                false,
            )
        })
    }
}
impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: this wrapper owns one non-null Create/Copy reference.
        unsafe { CFRelease(self.0) };
    }
}

fn nfc(value: &str) -> Result<String, &'static str> {
    let source = Owned::string(value)?;
    // SAFETY: source is a live CFString. Form 2 is canonical composition (C).
    let normal = Owned::new(unsafe { CFStringCreateMutableCopy(std::ptr::null(), 0, source.0) })?;
    // SAFETY: normal owns a mutable CFString.
    unsafe { CFStringNormalize(normal.0, 2) };
    // SAFETY: normal is a live CFString; the buffer includes a terminator.
    let len = unsafe { CFStringGetMaximumSizeForEncoding(CFStringGetLength(normal.0), UTF8) };
    let size = len
        .checked_add(1)
        .filter(|n| (1..=1024 * 1024).contains(n))
        .ok_or(UNAVAILABLE)?;
    let mut bytes = vec![0u8; size as usize];
    // SAFETY: allocated buffer has the advertised size.
    if !unsafe { CFStringGetCString(normal.0, bytes.as_mut_ptr().cast(), size, UTF8) } {
        return Err(UNAVAILABLE);
    }
    let end = bytes.iter().position(|b| *b == 0).ok_or(UNAVAILABLE)?;
    String::from_utf8(bytes[..end].to_vec()).map_err(|_| UNAVAILABLE)
}

fn username() -> Result<String, &'static str> {
    let configured = match std::env::var("USER") {
        Ok(value) => Some(value).filter(|value| !value.is_empty()),
        Err(std::env::VarError::NotPresent) => None,
        // Do not guess a different Keychain account for an unrepresentable
        // environment value that Claude may interpret differently.
        Err(std::env::VarError::NotUnicode(_)) => return Err(UNAVAILABLE),
    };
    let user = if let Some(value) = configured {
        value
    } else {
        let mut entry = std::mem::MaybeUninit::<libc::passwd>::uninit();
        let mut result = std::ptr::null_mut();
        let mut buffer = vec![0u8; 16384];
        // SAFETY: entry, result and buffer are valid caller-owned storage.
        let rc = unsafe {
            libc::getpwuid_r(
                euid(),
                entry.as_mut_ptr(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut result,
            )
        };
        if rc != 0 || result.is_null() {
            return Err(UNAVAILABLE);
        }
        // SAFETY: successful getpwuid_r points the terminated name into buffer.
        unsafe { std::ffi::CStr::from_ptr((*result).pw_name) }
            .to_str()
            .map_err(|_| UNAVAILABLE)?
            .to_string()
    };
    Ok(
        if user
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        {
            user
        } else {
            "claude-code-user".into()
        },
    )
}

fn service_name(directory: &str, custom_oauth: bool) -> Result<String, &'static str> {
    let path = nfc(directory)?;
    let digest = format!("{:x}", Sha256::digest(path.as_bytes()));
    let suffix = if custom_oauth { "-custom-oauth" } else { "" };
    Ok(format!("Claude Code{suffix}-credentials-{}", &digest[..8]))
}

fn native(dir: &Path) -> Result<Option<Snapshot>, &'static str> {
    // The inspected production binary fixes its base environment to prod;
    // its custom OAuth URL selects this separate credential namespace.
    let custom = std::env::var_os("CLAUDE_CODE_CUSTOM_OAUTH_URL").is_some_and(|v| !v.is_empty());
    let service = Owned::string(&service_name(dir.to_str().ok_or(UNAVAILABLE)?, custom)?)?;
    let account = Owned::string(&username()?)?;
    // SAFETY: all query keys/values remain live until the query is destroyed.
    // No callbacks are needed; the dictionary does not own these references.
    let query = Owned::new(unsafe {
        CFDictionaryCreateMutable(std::ptr::null(), 0, std::ptr::null(), std::ptr::null())
    })?;
    let mut result = std::ptr::null();
    // SAFETY: the dictionary and all referenced keys/values are live.
    let status = unsafe {
        CFDictionarySetValue(query.0, kSecClass, kSecClassGenericPassword);
        CFDictionarySetValue(query.0, kSecAttrService, service.0);
        CFDictionarySetValue(query.0, kSecAttrAccount, account.0);
        CFDictionarySetValue(query.0, kSecReturnData, kCFBooleanTrue);
        CFDictionarySetValue(
            query.0,
            kSecUseAuthenticationUI,
            kSecUseAuthenticationUIFail,
        );
        SecItemCopyMatching(query.0, &mut result)
    };
    if status == -25300 {
        return Ok(None);
    } // errSecItemNotFound only
    if status != 0 {
        return Err(UNAVAILABLE);
    }
    let data = Owned::new(result)?;
    // SAFETY: inspect the returned type before accessing CFData's bytes.
    if unsafe { CFGetTypeID(data.0) != CFDataGetTypeID() } {
        return Err(UNAVAILABLE);
    }
    // SAFETY: the returned object was checked to be CFData.
    let len = unsafe { CFDataGetLength(data.0) };
    if !(1..=1024 * 1024).contains(&len) {
        return Err(UNAVAILABLE);
    }
    // SAFETY: CFData keeps its bounded buffer alive until after this copy.
    let bytes =
        unsafe { std::slice::from_raw_parts(CFDataGetBytePtr(data.0), len as usize) }.to_vec();
    Ok(Some(Snapshot {
        credentials: bytes,
        continuity: Vec::new(),
    }))
}

#[cfg(test)]
mod tests_keychain_lookup {
    #[test]
    fn configuration_directory_uses_claudes_canonical_unicode_composition() {
        // CoreFoundation only: this test neither reads nor opens a Keychain.
        assert_eq!(
            super::nfc("/isolated/cafe\u{301}").unwrap(),
            "/isolated/caf\u{e9}"
        );
        assert_eq!(
            super::service_name("/isolated/cafe\u{301}", false).unwrap(),
            "Claude Code-credentials-95e794c9"
        );
        assert_eq!(
            super::service_name("/isolated/caf\u{e9}", true).unwrap(),
            "Claude Code-custom-oauth-credentials-95e794c9"
        );
    }
}
