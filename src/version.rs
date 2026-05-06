//! Build/version strings baked in at compile time by build.rs.

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const BUILD_DATE: &str = env!("FREEPLAY_BUILD_DATE");
/// Short git hash (e.g. "abc1234"). Empty string in non-git build environments
/// (e.g. when building from a release tarball without a git checkout).
pub const GIT_HASH: &str = match option_env!("FREEPLAY_GIT_HASH") {
    Some(h) => h,
    None => "",
};

/// Short build tag for the footer, e.g. "v0.1.0 • 2026-04-21" or
/// "v0.1.0 • 2026-04-21 (abc123)" when git is available.
pub fn footer_string() -> String {
    match option_env!("FREEPLAY_GIT_HASH") {
        Some(h) if !h.is_empty() => format!("v{VERSION}  {BUILD_DATE}  ({h})"),
        _ => format!("v{VERSION}  {BUILD_DATE}"),
    }
}
