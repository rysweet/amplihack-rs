//! CC-AMPLIHACK-LBUG-PREBUILT-1917; PM328 comment6613591861.
//! These are internal Rust contract signatures proposed by the RED tests.
//! They do not introduce a stable application API or a privileged test command.

#[path = "../artifact_adversarial.rs"]
mod artifact_adversarial;
#[path = "../cache_concurrency.rs"]
mod cache_concurrency;
#[path = "../diagnostics.rs"]
mod diagnostics;
#[path = "../filesystem_safety.rs"]
mod filesystem_safety;
#[path = "../identity_compatibility.rs"]
mod identity_compatibility;
#[path = "../manifest_integrity.rs"]
mod manifest_integrity;
#[path = "../publication_protocol.rs"]
mod publication_protocol;
#[path = "../source_selection.rs"]
mod source_selection;
#[path = "../vendor_provenance.rs"]
mod vendor_provenance;

mod support {
    use flate2::{Compression, write::GzEncoder};
    use sha2::{Digest, Sha256};
    use tar::{Builder, EntryType, Header};

    pub fn digest(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    pub const KEY: &str = "lbug-0.15.4-aarch64-apple-darwin-appleclang17";

    pub fn sidecar(bytes: &[u8]) -> String {
        format!("{}  {KEY}.tar.gz\n", digest(bytes))
    }

    /// Build untrusted transport bytes independently of the implementation.
    /// Raw name assignment deliberately permits traversal that tar::set_path
    /// would reject before the system under test could inspect it.
    pub fn archive(entries: &[(&str, EntryType, &[u8])]) -> Vec<u8> {
        let mut builder = Builder::new(GzEncoder::new(Vec::new(), Compression::fast()));
        for (path, kind, bytes) in entries {
            assert!(path.len() < 100, "use a PAX fixture for longer names");
            let mut header = Header::new_ustar();
            header.as_mut_bytes()[..path.len()].copy_from_slice(path.as_bytes());
            header.set_entry_type(*kind);
            header.set_mode(0o644);
            header.set_uid(0);
            header.set_gid(0);
            header.set_mtime(0);
            if kind.is_symlink() || kind.is_hard_link() {
                header.set_link_name("../../escaped").unwrap();
            }
            header.set_size(bytes.len() as u64);
            header.set_cksum();
            builder.append(&header, *bytes).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap()
    }
}
