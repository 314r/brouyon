//! Whether Brouyon looks, and takes and names its keys, as a Mac app does.

/// Everything that differs on a Mac asks this. Both sides then compile on both systems,
/// and `cargo test --features mac-look` tests a Mac's on Linux. Only what calls AppKit,
/// or what GPUI has on a Mac alone, asks for the real system, in `macos.rs`.
pub const MAC: bool = cfg!(any(target_os = "macos", feature = "mac-look"));
