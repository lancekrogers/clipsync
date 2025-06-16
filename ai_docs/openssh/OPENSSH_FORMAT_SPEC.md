# OpenSSH Private Key Format Specification

## Format Overview

OpenSSH private keys use a custom binary format wrapped in PEM-style headers.

## Detailed Binary Structure

### 1. PEM Wrapper
```
-----BEGIN OPENSSH PRIVATE KEY-----
[base64-encoded binary data]
-----END OPENSSH PRIVATE KEY-----
```

### 2. Binary Format (after base64 decode)

All multi-byte integers are in network byte order (big-endian).

#### Header
```c
// Magic string
char magic[15] = "openssh-key-v1";
char null_terminator = '\0';

// Encryption info
uint32_t cipher_name_len;
char cipher_name[cipher_name_len];      // e.g., "none", "aes256-ctr"

uint32_t kdf_name_len;
char kdf_name[kdf_name_len];           // e.g., "none", "bcrypt"

uint32_t kdf_options_len;
char kdf_options[kdf_options_len];     // KDF-specific data

// Key count (usually 1)
uint32_t num_keys;

// Public key(s)
for (i = 0; i < num_keys; i++) {
    uint32_t pubkey_len;
    char pubkey_blob[pubkey_len];      // Full public key data
}

// Private key section
uint32_t private_section_len;
char private_section[private_section_len];  // May be encrypted
```

#### Private Section Format (when decrypted)
```c
// Check values for successful decryption
uint32_t check1;                        // Random value
uint32_t check2;                        // Must equal check1

// For each key
for (i = 0; i < num_keys; i++) {
    // Key type
    uint32_t keytype_len;
    char keytype[keytype_len];          // "ssh-ed25519", "ssh-rsa", etc.
    
    // Key-specific data (see below)
    [varies by key type]
    
    // Comment
    uint32_t comment_len;
    char comment[comment_len];
}

// Padding (1, 2, 3, ... up to cipher block size)
char padding[];
```

### 3. Key Type Specific Data

#### Ed25519 Format
```c
// Public key (duplicated from header)
uint32_t pubkey_len;                    // Always 32 for Ed25519
char pubkey[32];

// Private key
uint32_t privkey_len;                   // Always 64 for Ed25519
char privkey[64];                       // 32-byte private + 32-byte public
```

#### RSA Format
```c
// Public components
uint32_t n_len;
char n[n_len];                          // Modulus
uint32_t e_len;
char e[e_len];                          // Public exponent

// Private components
uint32_t d_len;
char d[d_len];                          // Private exponent
uint32_t iqmp_len;
char iqmp[iqmp_len];                    // q^-1 mod p
uint32_t p_len;
char p[p_len];                          // Prime 1
uint32_t q_len;
char q[q_len];                          // Prime 2
```

### 4. Encryption Details

#### BCrypt KDF
When `kdf_name` is "bcrypt", `kdf_options` contains:
```c
uint32_t salt_len;
char salt[salt_len];                    // Usually 16 bytes
uint32_t rounds;                        // Usually 16 or higher
```

#### Cipher Modes
- `aes256-ctr`: AES-256 in CTR mode
- `aes256-cbc`: AES-256 in CBC mode (older)
- `aes128-ctr`: AES-128 in CTR mode
- `aes128-cbc`: AES-128 in CBC mode

### 5. String Encoding
All strings in the format use the SSH wire format:
```c
uint32_t length;                        // Big-endian
char data[length];                      // UTF-8 bytes
```

## Implementation Notes

1. **Padding**: The padding at the end ensures the private section is a multiple of the cipher block size (8 bytes for unencrypted)

2. **Check Values**: The duplicate check values (check1, check2) verify successful decryption

3. **Key Material**: Ed25519 private keys include both private and public parts (64 bytes total)

4. **Endianness**: All integers are big-endian (network byte order)

## Example Parse Flow

1. Strip PEM headers and base64 decode
2. Verify magic string "openssh-key-v1\0"
3. Read cipher and KDF info
4. Extract public key blob
5. If encrypted:
   - Derive key using KDF
   - Decrypt private section
6. Parse private section:
   - Verify check values match
   - Extract key type and components
   - Validate padding

## Common Pitfalls

1. **String Length**: Don't forget the 4-byte length prefix for all strings
2. **Padding**: Must be sequential (1, 2, 3, ...) not just zeros
3. **Ed25519 Size**: Private key is 64 bytes (includes public), not 32
4. **Check Values**: Must be random, not zero (common implementation error)