//! Minimal viewer crate placeholder. The UI crate drives the rendering;
//! this crate can provide helpers in future PRs. For now, it's a stub.

pub fn is_binary(buf: &[u8]) -> bool {
    buf.contains(&0)
}
