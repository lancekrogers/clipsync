# Sprint 2 - Agent 1: SSH Authentication Module

## Context
You are Agent 1 for Sprint 2 of the ClipSync project. Your focus is implementing the SSH authentication system that will secure peer-to-peer connections.

## Prerequisites
- Sprint 1 has been completed with:
  - Basic Rust project structure
  - Config module with SSH settings
  - Clipboard abstraction layer
  - SQLite database for history

## Your Tasks (Task 07)

### 1. SSH Authentication Module
Create the SSH authentication infrastructure in `src/auth/ssh.rs`:
- SSH key generation and management
- Public key authentication
- Key fingerprint verification
- Session establishment

### 2. Key Management Utilities
Implement utilities in `src/auth/keys.rs`:
- Generate new SSH key pairs (Ed25519)
- Load existing keys from disk
- Export public keys in OpenSSH format
- Secure key storage with proper permissions

### 3. Authorized Keys Management
Create functionality in `src/auth/authorized.rs`:
- Parse ~/.ssh/authorized_keys format
- Add/remove/list authorized peers
- Validate incoming connections against authorized keys
- Support for key comments and options

### 4. Authentication API
Define the public API that Agent 2 (Transport) will use:
```rust
pub trait Authenticator {
    async fn authenticate_peer(&self, peer_key: &PublicKey) -> Result<AuthToken>;
    async fn verify_token(&self, token: &AuthToken) -> Result<PeerId>;
}
```

## Key Design Decisions
- Use Ed25519 keys for modern security
- Store keys in standard SSH locations when possible
- Provide clear error messages for auth failures
- Support both interactive and non-interactive auth

## Integration Points
- Export a clean API for the transport layer (Agent 2)
- Use config module for SSH settings
- Store peer information in the database

## Testing Requirements
- Unit tests for key generation and parsing
- Integration tests for full auth flow
- Mock SSH server for testing
- Performance tests for key verification

## Success Criteria
- Generate and manage SSH keys
- Authenticate peers using public keys
- Parse and manage authorized_keys file
- Clean API for transport integration
- All tests passing

## Files to Create/Modify
- `src/auth/mod.rs` - Module declaration
- `src/auth/ssh.rs` - Core SSH authentication
- `src/auth/keys.rs` - Key management
- `src/auth/authorized.rs` - Authorized keys handling
- `src/lib.rs` - Export auth module
- Tests in `src/auth/` subdirectories

## Dependencies
- `ssh2` or `russh` crate for SSH protocol
- `ring` or similar for cryptography
- `dirs` for finding SSH directories

Remember to coordinate with Agent 2 who needs your authentication API.