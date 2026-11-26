//! The files of `amplihack config`, shared by the command-line program and
//! the hook program (issue #3247).
//!
//! [`files`] says where the user file and the project file are, [`read`]
//! reads and parses one of them, and [`text_switch`] gives the value of the
//! two keys that switch off text amplihack inserts into a session. The
//! command-line program's `user_config` module uses the same code, so the
//! hook and `amplihack config get` cannot disagree. Nothing here starts a
//! process or writes a file.

pub mod files;
pub mod read;
pub mod text_switch;
