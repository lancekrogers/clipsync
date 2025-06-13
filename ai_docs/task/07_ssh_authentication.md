# Task 07: SSH Authentication Module

## Objective
Implement SSH key-based authentication for secure peer-to-peer connections.

## Steps

1. **Create src/transport/ssh.rs**
   - SSH key loading and parsing
   - Public key authentication
   - Authorized keys management

2. **Implement SSH authentication**
   ```rust
   pub struct SshAuthenticator {
       private_key: SshKey,
       authorized_keys: Vec<PublicKey>,
   }
   
   impl SshAuthenticator {
       pub fn new(key_path: &Path, authorized_keys_path: &Path) -> Result<Self>;
       pub fn authenticate_peer(&self, peer_key: &[u8]) -> Result<bool>;
       pub fn sign_challenge(&self, challenge: &[u8]) -> Result<Vec<u8>>;
       pub fn verify_signature(&self, key: &PublicKey, data: &[u8], sig: &[u8]) -> bool;
   }
   ```

3. **Add key format support**
   - RSA keys (id_rsa)
   - Ed25519 keys (id_ed25519)
   - ECDSA keys
   - OpenSSH format parsing

4. **Implement authorized_keys parsing**
   - Read ~/.config/clipsync/authorized_keys
   - Support standard OpenSSH format
   - Handle comments and options

5. **Create authentication handshake**
   - Challenge-response protocol
   - Mutual authentication
   - Session key derivation

6. **Add key management utilities**
   - Generate new key pairs
   - Export public keys
   - Fingerprint calculation

## Success Criteria
- SSH keys load correctly
- Authentication works with standard SSH keys
- Authorized keys file compatible with OpenSSH
- Secure against replay attacks