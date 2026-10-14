//! Platform-independent core of the desktop build.
//!
//! Kept separate from `main.rs` so the data layer can be unit-tested on any OS, including in
//! CI containers without a webview — the Windows-only parts live in the binary.

pub mod paths;
pub mod store;
