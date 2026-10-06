//! Path helpers.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// Split a path into its non-empty components.
pub fn components(path: &str) -> Vec<String> {
    path.split('/').filter(|p| !p.is_empty()).map(|p| p.to_string()).collect()
}

/// Join a directory and a relative path.
pub fn join(dir: &str, rel: &str) -> String {
    if rel.starts_with('/') {
        return rel.to_string();
    }
    let mut s = dir.trim_end_matches('/').to_string();
    s.push('/');
    s.push_str(rel);
    s
}

/// The directory part of a path ("/boot/grub/grub.cfg" -> "/boot/grub").
pub fn parent(path: &str) -> &str {
    match path.trim_end_matches('/').rfind('/') {
        Some(0) | None => "/",
        Some(i) => &path[..i],
    }
}
