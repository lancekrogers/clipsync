# OpenSSH Support Implementation Plan

## Overview
ClipSync needs to support standard OpenSSH format keys that users already have, not just PKCS8 format. This is a core requirement to leverage existing SSH infrastructure.

## Current State
- ✅ PKCS8 format support (working)
- ✅ Basic OpenSSH format detection
- ❌ OpenSSH private key parsing (incomplete)
- ❌ RSA key support (only Ed25519)
- ❌ Encrypted key support

## OpenSSH Private Key Format

### Structure
```
-----BEGIN OPENSSH PRIVATE KEY-----
[base64 encoded data]
-----END OPENSSH PRIVATE KEY-----
```

### Binary Format (after base64 decode)
```
"openssh-key-v1\0"    // Magic string (15 bytes + null)
ciphername            // string (e.g., "none" or "aes256-ctr")
kdfname               // string (e.g., "none" or "bcrypt")
kdfoptions            // string (salt and rounds for KDF)
number of keys        // uint32 (usually 1)
public key            // string (full public key data)
private key section   // encrypted/unencrypted private data
```

### Private Key Section Format
```
checkint1             // uint32 (random check value)
checkint2             // uint32 (must equal checkint1)
key type              // string (e.g., "ssh-rsa", "ssh-ed25519")
[key-specific data]   // Varies by key type
comment               // string
padding               // 1, 2, 3, 4, 5, 6, 7... to block size
```

## Implementation Tasks

### Phase 1: Core OpenSSH Parser
1. **Binary Format Parser** (`src/auth/openssh_parser.rs`)
   - [ ] Parse OpenSSH magic header
   - [ ] Extract cipher and KDF information
   - [ ] Handle unencrypted keys first
   - [ ] Parse public key section
   - [ ] Parse private key section with checksum validation

2. **Key Type Support**
   - [ ] Ed25519 private key extraction
   - [ ] RSA private key extraction
   - [ ] Convert extracted keys to formats Ring can use

### Phase 2: RSA Support
1. **RSA Key Handling** (`src/auth/keys.rs`)
   - [ ] Add RSA key type to KeyPair struct
   - [ ] Implement RSA operations using Ring or RustCrypto
   - [ ] Support common RSA key sizes (2048, 3072, 4096)

2. **RSA OpenSSH Format**
   - [ ] Parse RSA components (n, e, d, p, q, etc.)
   - [ ] Convert to PKCS8 for Ring compatibility

### Phase 3: Encrypted Key Support
1. **KDF Implementation**
   - [ ] BCrypt KDF for key derivation
   - [ ] Support for common ciphers (aes256-ctr, aes256-cbc)

2. **Interactive Passphrase**
   - [ ] Detect encrypted keys
   - [ ] Prompt for passphrase (with appropriate UI)
   - [ ] Decrypt private key section

### Phase 4: Testing & Edge Cases
1. **Comprehensive Testing**
   - [ ] Test with keys from different SSH implementations
   - [ ] Test various key sizes and types
   - [ ] Test encrypted vs unencrypted
   - [ ] Handle malformed keys gracefully

2. **Compatibility**
   - [ ] OpenSSH 7.8+ format (current)
   - [ ] Legacy formats if needed
   - [ ] PuTTY key format (stretch goal)

## Code Structure

```
src/auth/
├── mod.rs              # Main auth module
├── keys.rs             # Key management (update)
├── openssh/
│   ├── mod.rs          # OpenSSH module entry
│   ├── parser.rs       # Binary format parser
│   ├── ed25519.rs      # Ed25519-specific handling
│   ├── rsa.rs          # RSA-specific handling
│   └── encryption.rs   # Encrypted key support
```

## Dependencies
- Current: `ring` (for crypto operations)
- May need: 
  - `rsa` crate for RSA operations
  - `bcrypt` for KDF
  - `aes` for decryption
  - Or alternatives that work well with async

## Success Criteria
1. Can load standard `~/.ssh/id_rsa` and `~/.ssh/id_ed25519` keys
2. Works with encrypted keys (prompting for passphrase)
3. No need for users to convert or generate special keys
4. Clear error messages for unsupported formats
5. Performance comparable to SSH client

## Security Considerations
1. Never log private key material
2. Clear sensitive data from memory after use
3. Validate all input to prevent parsing vulnerabilities
4. Handle timing attacks in key operations