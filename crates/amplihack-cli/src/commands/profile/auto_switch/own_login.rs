//! Whether a profile holds a login of its own, as auto-switch asks it
//! (issues #2885 and #2573). `auto-switch status`, the rerun selection, the
//! takeover check and the overflow checks all ask here, so they give one
//! answer, and on macOS it is the launch guard's answer.
//!
//! Before, each of those places counted every macOS profile as holding a
//! login, because a Keychain login leaves no file to find. Now a macOS
//! profile with no login file is resolved once through `kind::Ctx`, which is
//! where the Keychain is asked. Nothing here starts `security` itself.
//!
//! Off macOS the login file alone decides, with one `stat`, as before.
//!
//! One limit: [`OwnLogin::File`] means only that the login file exists. The
//! launch guard goes further and reads it, so a profile whose login file is
//! unreadable or holds no OAuth login is `File` here and still refused at
//! launch. The two agree on the Keychain answers, not on the file's content.

use super::fsx;
use super::state::CREDS;
use crate::commands::profile::keychain::{self, Cause, Keychain};
use crate::commands::profile::kind::{Ctx, Kind};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::store::ProfileStore;
use std::cell::{OnceCell, RefCell};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Where a profile's own login is, if it has one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnLogin {
    /// `<root>/<name>/.credentials.json` exists.
    File,
    /// macOS, no login file: an item in the login Keychain.
    Keychain,
    /// macOS, no login file: an API-key profile, whose credential is kept
    /// outside the profile directory. It still counts for selection, where
    /// its own check (`OverflowInputs::check_note`) decides.
    KeyProfile,
    /// macOS, no login file: the Keychain could not be read. Not a login,
    /// and not an API-key profile either.
    KeychainUnreadable(Cause),
    /// No login of its own.
    None,
}

impl OwnLogin {
    /// Whether a move onto the profile can find a login.
    pub fn holds(self) -> bool {
        match self {
            Self::File | Self::Keychain | Self::KeyProfile => true,
            Self::KeychainUnreadable(_) | Self::None => false,
        }
    }
}

/// The own-login answers for one command, each worked out once. The kinds
/// (and with them the Keychain) are read only when a macOS profile without a
/// login file is first asked about.
pub struct OwnLogins<'a> {
    store: &'a ProfileStore,
    home: Option<PathBuf>,
    macos: bool,
    kinds: OnceCell<Ctx>,
    seen: RefCell<BTreeMap<ProfileName, OwnLogin>>,
}

impl<'a> OwnLogins<'a> {
    /// `home` is the home directory a Keychain lookup is given; without one
    /// a macOS profile that needs the Keychain reads as unreadable.
    pub fn new(store: &'a ProfileStore, home: Option<&Path>) -> Self {
        Self {
            store,
            home: home.map(Path::to_path_buf),
            macos: keychain::Env::ambient(home).macos(),
            kinds: OnceCell::new(),
            seen: RefCell::new(BTreeMap::new()),
        }
    }

    /// Whether the Keychain can be asked at all: only on macOS.
    pub fn macos(&self) -> bool {
        self.macos
    }

    /// Where `name`'s own login is. The second call for a name reads
    /// nothing.
    pub fn of(&self, name: &ProfileName) -> OwnLogin {
        if let Some(known) = self.seen.borrow().get(name) {
            return *known;
        }
        let found = self.read(name);
        self.seen.borrow_mut().insert(name.clone(), found);
        found
    }

    fn read(&self, name: &ProfileName) -> OwnLogin {
        if fsx::exists(&self.store.root().join(name.as_str()).join(CREDS)) {
            return OwnLogin::File;
        }
        if !self.macos {
            return OwnLogin::None;
        }
        let kinds = self
            .kinds
            .get_or_init(|| Ctx::load(self.store, self.home.as_deref()));
        let resolved = kinds.resolve_kind(self.store, name);
        match (resolved.kind, resolved.keychain) {
            (Kind::Platform, _) => OwnLogin::KeyProfile,
            (Kind::Subscription, Some(Keychain::Present)) => OwnLogin::Keychain,
            (Kind::Subscription, Some(Keychain::Unreadable(cause))) => {
                OwnLogin::KeychainUnreadable(cause)
            }
            (Kind::Subscription, Some(Keychain::Absent) | None) => OwnLogin::None,
        }
    }

    /// Why `name` was passed over, when the reason is an unreadable
    /// Keychain.
    pub fn unreadable(&self, name: &ProfileName) -> Option<Cause> {
        match self.of(name) {
            OwnLogin::KeychainUnreadable(cause) => Some(cause),
            OwnLogin::File | OwnLogin::Keychain | OwnLogin::KeyProfile | OwnLogin::None => None,
        }
    }
}

/// The one line a selection prints for a profile it passed over because the
/// Keychain could not be read. It names the profile and the fixed cause, and
/// nothing about the account.
pub fn unreadable_note(name: &ProfileName, cause: Cause) -> String {
    format!(
        "auto-switch: profile {name} was passed over: it has no login file and the macOS Keychain could not be read ({cause})"
    )
}
