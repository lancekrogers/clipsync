//! OpenSSH private key format support

pub mod ed25519;
pub mod parser;

pub use parser::{parse_openssh_private_key, OpenSshKey, PrivateKeyData, KeyTypeData};