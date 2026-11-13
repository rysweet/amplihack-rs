//! Clear the default and rotation pin under their existing lock.
use super::{ProfileStore, Released};
/// `profile switch --none`: removes the rotation pin and the default
/// pointer as one operation under the rotate lock; what was there. Off Unix
/// there is no rotator and so no pin, and only the pointer is removed.
pub(in crate::commands::profile) fn release(
    store: &ProfileStore,
) -> Result<Released, crate::commands::profile::ProfileError> {
    #[cfg(unix)]
    {
        super::switch::release(store)
    }
    #[cfg(not(unix))]
    {
        let default = crate::commands::profile::pointer::exists(store.root());
        crate::commands::profile::pointer::clear(store.root())?;
        Ok(Released {
            pin: false,
            default,
        })
    }
}
