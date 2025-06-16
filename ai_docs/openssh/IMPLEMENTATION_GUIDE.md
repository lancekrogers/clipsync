# OpenSSH Implementation Guide

## Step-by-Step Implementation

### Step 1: Create OpenSSH Module Structure

```rust
// src/auth/openssh/mod.rs
pub mod parser;
pub mod ed25519;
pub mod rsa;
pub mod encryption;

pub use parser::{parse_openssh_private_key, OpenSshKey};
```

### Step 2: Core Parser Implementation

```rust
// src/auth/openssh/parser.rs
use std::io::{Cursor, Read};
use byteorder::{BigEndian, ReadBytesExt};

pub struct OpenSshKey {
    pub cipher_name: String,
    pub kdf_name: String,
    pub kdf_options: Vec<u8>,
    pub public_key: Vec<u8>,
    pub private_section: Vec<u8>,
    pub is_encrypted: bool,
}

pub fn parse_openssh_private_key(data: &[u8]) -> Result<OpenSshKey, Error> {
    let mut cursor = Cursor::new(data);
    
    // Read and verify magic
    let mut magic = [0u8; 15];
    cursor.read_exact(&mut magic)?;
    if &magic != b"openssh-key-v1" {
        return Err(Error::InvalidMagic);
    }
    
    // Skip null terminator
    cursor.read_u8()?;
    
    // Read cipher name
    let cipher_name = read_string(&mut cursor)?;
    let kdf_name = read_string(&mut cursor)?;
    let kdf_options = read_bytes(&mut cursor)?;
    
    // Read number of keys
    let num_keys = cursor.read_u32::<BigEndian>()?;
    if num_keys != 1 {
        return Err(Error::MultipleKeysNotSupported);
    }
    
    // Read public key
    let public_key = read_bytes(&mut cursor)?;
    
    // Read private section
    let private_section = read_bytes(&mut cursor)?;
    
    Ok(OpenSshKey {
        is_encrypted: cipher_name != "none",
        cipher_name,
        kdf_name,
        kdf_options,
        public_key,
        private_section,
    })
}

fn read_string(cursor: &mut Cursor<&[u8]>) -> Result<String, Error> {
    let bytes = read_bytes(cursor)?;
    String::from_utf8(bytes).map_err(|_| Error::InvalidUtf8)
}

fn read_bytes(cursor: &mut Cursor<&[u8]>) -> Result<Vec<u8>, Error> {
    let len = cursor.read_u32::<BigEndian>()? as usize;
    let mut bytes = vec![0u8; len];
    cursor.read_exact(&mut bytes)?;
    Ok(bytes)
}
```

### Step 3: Private Section Parser

```rust
// src/auth/openssh/parser.rs (continued)
pub struct PrivateKeyData {
    pub key_type: String,
    pub private_data: KeyTypeData,
    pub comment: String,
}

pub enum KeyTypeData {
    Ed25519 {
        public: [u8; 32],
        private: [u8; 64],  // 32-byte private + 32-byte public
    },
    Rsa {
        n: Vec<u8>,  // modulus
        e: Vec<u8>,  // public exponent
        d: Vec<u8>,  // private exponent
        iqmp: Vec<u8>,  // q^-1 mod p
        p: Vec<u8>,  // prime1
        q: Vec<u8>,  // prime2
    },
}

pub fn parse_private_section(data: &[u8]) -> Result<PrivateKeyData, Error> {
    let mut cursor = Cursor::new(data);
    
    // Read and verify check values
    let check1 = cursor.read_u32::<BigEndian>()?;
    let check2 = cursor.read_u32::<BigEndian>()?;
    if check1 != check2 {
        return Err(Error::InvalidCheckValues);
    }
    
    // Read key type
    let key_type = read_string(&mut cursor)?;
    
    // Parse key-specific data
    let private_data = match key_type.as_str() {
        "ssh-ed25519" => parse_ed25519_private(&mut cursor)?,
        "ssh-rsa" => parse_rsa_private(&mut cursor)?,
        _ => return Err(Error::UnsupportedKeyType(key_type)),
    };
    
    // Read comment
    let comment = read_string(&mut cursor)?;
    
    // Verify padding
    verify_padding(&mut cursor)?;
    
    Ok(PrivateKeyData {
        key_type,
        private_data,
        comment,
    })
}
```

### Step 4: Ed25519 Implementation

```rust
// src/auth/openssh/ed25519.rs
use ring::signature::Ed25519KeyPair;

pub fn parse_ed25519_private(cursor: &mut Cursor<&[u8]>) -> Result<KeyTypeData, Error> {
    // Read public key (32 bytes)
    let public_bytes = read_bytes(cursor)?;
    if public_bytes.len() != 32 {
        return Err(Error::InvalidKeySize);
    }
    
    // Read private key (64 bytes: 32 private + 32 public)
    let private_bytes = read_bytes(cursor)?;
    if private_bytes.len() != 64 {
        return Err(Error::InvalidKeySize);
    }
    
    let mut public = [0u8; 32];
    let mut private = [0u8; 64];
    public.copy_from_slice(&public_bytes);
    private.copy_from_slice(&private_bytes);
    
    Ok(KeyTypeData::Ed25519 { public, private })
}

pub fn ed25519_to_ring_keypair(data: &KeyTypeData) -> Result<Ed25519KeyPair, Error> {
    match data {
        KeyTypeData::Ed25519 { private, .. } => {
            // Extract the 32-byte seed (first half of private key)
            let seed = &private[..32];
            
            // Convert to PKCS8 format for Ring
            let pkcs8 = ed25519_seed_to_pkcs8(seed)?;
            
            Ed25519KeyPair::from_pkcs8(&pkcs8)
                .map_err(|_| Error::KeyConversionFailed)
        }
        _ => Err(Error::WrongKeyType),
    }
}

fn ed25519_seed_to_pkcs8(seed: &[u8]) -> Result<Vec<u8>, Error> {
    // PKCS8 structure for Ed25519
    let mut pkcs8 = vec![
        0x30, 0x2e,  // SEQUENCE (46 bytes)
        0x02, 0x01, 0x00,  // INTEGER 0 (version)
        0x30, 0x05,  // SEQUENCE (5 bytes) - AlgorithmIdentifier
        0x06, 0x03, 0x2b, 0x65, 0x70,  // OID for Ed25519
        0x04, 0x22,  // OCTET STRING (34 bytes)
        0x04, 0x20,  // OCTET STRING (32 bytes) - the key
    ];
    pkcs8.extend_from_slice(seed);
    Ok(pkcs8)
}
```

### Step 5: RSA Implementation

```rust
// src/auth/openssh/rsa.rs
use rsa::{RsaPrivateKey, BigUint};

pub fn parse_rsa_private(cursor: &mut Cursor<&[u8]>) -> Result<KeyTypeData, Error> {
    let n = read_bytes(cursor)?;  // modulus
    let e = read_bytes(cursor)?;  // public exponent
    let d = read_bytes(cursor)?;  // private exponent
    let iqmp = read_bytes(cursor)?;  // q^-1 mod p
    let p = read_bytes(cursor)?;  // prime1
    let q = read_bytes(cursor)?;  // prime2
    
    Ok(KeyTypeData::Rsa { n, e, d, iqmp, p, q })
}

pub fn rsa_to_pkcs8(data: &KeyTypeData) -> Result<Vec<u8>, Error> {
    match data {
        KeyTypeData::Rsa { n, e, d, p, q, .. } => {
            // Convert to rsa crate's key type
            let private_key = RsaPrivateKey::from_components(
                BigUint::from_bytes_be(n),
                BigUint::from_bytes_be(e),
                BigUint::from_bytes_be(d),
                vec![
                    BigUint::from_bytes_be(p),
                    BigUint::from_bytes_be(q),
                ],
            )?;
            
            // Export as PKCS8
            private_key.to_pkcs8_der()
                .map(|der| der.as_bytes().to_vec())
                .map_err(|_| Error::KeyConversionFailed)
        }
        _ => Err(Error::WrongKeyType),
    }
}
```

### Step 6: Integration with Existing Code

```rust
// Update src/auth/keys.rs
impl KeyPair {
    pub fn from_private_key_bytes(key_data: &[u8]) -> Result<Self, AuthError> {
        // Try PKCS8 first
        if let Ok(key_pair) = Ed25519KeyPair::from_pkcs8(key_data) {
            // ... existing code
        }
        
        // Try OpenSSH format
        if let Ok(key_str) = std::str::from_utf8(key_data) {
            if key_str.starts_with("-----BEGIN OPENSSH PRIVATE KEY-----") {
                return Self::from_openssh_private_key(key_str);
            }
        }
        
        // ... rest of existing code
    }
    
    fn from_openssh_private_key(pem_str: &str) -> Result<Self, AuthError> {
        // Strip PEM headers and decode base64
        let base64_data = pem_str
            .lines()
            .filter(|line| !line.starts_with("-----"))
            .collect::<String>();
        
        let binary_data = base64::decode(&base64_data)
            .map_err(|e| AuthError::InvalidKeyFormat(format!("Invalid base64: {}", e)))?;
        
        // Parse OpenSSH format
        let openssh_key = openssh::parse_openssh_private_key(&binary_data)
            .map_err(|e| AuthError::InvalidKeyFormat(format!("OpenSSH parse error: {}", e)))?;
        
        if openssh_key.is_encrypted {
            return Err(AuthError::InvalidKeyFormat(
                "Encrypted keys not yet supported. Please decrypt with: ssh-keygen -p -N \"\" -f <keyfile>".to_string()
            ));
        }
        
        // Parse private section
        let private_key = openssh::parse_private_section(&openssh_key.private_section)
            .map_err(|e| AuthError::InvalidKeyFormat(format!("Private section parse error: {}", e)))?;
        
        // Convert to our KeyPair format
        match private_key.private_data {
            KeyTypeData::Ed25519 { .. } => {
                let ring_keypair = openssh::ed25519::ed25519_to_ring_keypair(&private_key.private_data)?;
                // ... create KeyPair
            }
            KeyTypeData::Rsa { .. } => {
                // For now, convert to PKCS8 and error
                return Err(AuthError::InvalidKeyFormat(
                    "RSA keys require additional implementation".to_string()
                ));
            }
        }
    }
}
```

## Testing Strategy

1. **Unit Tests**: Test each parser component
2. **Integration Tests**: Test with real SSH keys
3. **Compatibility Tests**: Keys from different SSH versions
4. **Error Cases**: Malformed keys, wrong formats

## Dependencies to Add

```toml
[dependencies]
byteorder = "1.5"  # For reading binary data
rsa = "0.9"       # For RSA support
# Optional for encryption support later:
# bcrypt-pbkdf = "0.10"  # For KDF
# aes = "0.8"            # For decryption
```