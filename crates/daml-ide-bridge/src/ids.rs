//! Short, stable identifiers for virtual resources, and the access token.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// A resource id is derived from its `daml://` URI so the same script always
/// gets the same page. It only has to be stable within one bridge process,
/// which is why the standard hasher is good enough.
pub fn resource_id(uri: &str) -> String {
    let mut hasher = DefaultHasher::new();
    uri.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// The HTTP server binds to loopback, but every other process on the machine
/// can reach loopback too, and these pages contain project source. Require a
/// token that only the bridge and the links it prints know.
pub fn access_token() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("no source of randomness");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_uri_hashes_the_same_way() {
        assert_eq!(resource_id("daml://x"), resource_id("daml://x"));
        assert_ne!(resource_id("daml://x"), resource_id("daml://y"));
    }

    #[test]
    fn a_token_is_32_hex_characters() {
        let token = access_token();
        assert_eq!(token.len(), 32);
        assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(token, access_token());
    }
}
