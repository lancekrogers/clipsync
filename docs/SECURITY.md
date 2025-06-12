# 🔒 ClipSync Security Guide

Comprehensive security documentation covering ClipSync's security model, best practices, and threat analysis.

## 🛡️ Security Overview

ClipSync is designed with **security-first principles** to protect your clipboard data during synchronization across devices. Our security model ensures that only your authorized devices can access your clipboard content, and all communication is encrypted end-to-end.

### Core Security Principles

1. **Zero Trust Architecture**: No device is trusted by default
2. **End-to-End Encryption**: All data encrypted in transit and at rest
3. **No Cloud Dependency**: Direct peer-to-peer communication only
4. **Perfect Forward Secrecy**: Session keys are ephemeral
5. **Minimal Attack Surface**: Lean codebase with security audits

## 🔐 Authentication Model

### SSH Key-Based Authentication

ClipSync uses **SSH Ed25519 keys** for device authentication, providing strong cryptographic guarantees:

```
Device A                    Device B
┌─────────────┐             ┌─────────────┐
│ Private Key │◄───────────►│ Public Key  │
│ Public Key  │             │ Private Key │
└─────────────┘             └─────────────┘
      │                           │
      ▼                           ▼
┌─────────────┐             ┌─────────────┐
│ Authorized  │             │ Authorized  │
│ Keys File   │             │ Keys File   │
└─────────────┘             └─────────────┘
```

### Key Generation and Management

**Secure Key Generation:**
```bash
# Generate ClipSync-specific Ed25519 key pair
ssh-keygen -t ed25519 -f ~/.ssh/id_ed25519_clipsync -C "clipsync-$(hostname)-$(date +%Y%m%d)"

# Set restrictive permissions
chmod 600 ~/.ssh/id_ed25519_clipsync
chmod 644 ~/.ssh/id_ed25519_clipsync.pub
```

**Key Exchange Process:**
1. Each device generates its own Ed25519 key pair
2. Public keys are manually exchanged between trusted devices
3. Public keys are stored in `authorized_keys` file
4. Private keys never leave the device

**Security Properties:**
- **Ed25519**: 128-bit security level, quantum-resistant candidate
- **Key Size**: 32-byte private keys, 32-byte public keys
- **Performance**: Fast signature generation and verification
- **Collision Resistance**: Cryptographically secure hash functions

### Authentication Flow

```
Device A                                    Device B
   │                                           │
   ├── 1. WebSocket Connection ──────────────► │
   │                                           │
   ├── 2. Send Public Key + Challenge ──────► │
   │                                           │
   │ ◄── 3. Verify Key, Send Signed Response ─┤
   │                                           │
   ├── 4. Verify Signature ──────────────────► │
   │                                           │
   ├── 5. Send Session Key (Encrypted) ─────► │
   │                                           │
   │ ◄── 6. Acknowledge Session Key ──────────┤
   │                                           │
   │ ◄══► 7. Encrypted Communication ◄══════► │
```

## 🔒 Encryption Architecture

### Transport Layer Security

**Protocol Stack:**
```
Application Layer    │ ClipSync Protocol (JSON/Binary)
                    │
Session Layer       │ AES-256-GCM Encryption
                    │ Perfect Forward Secrecy
                    │
Transport Layer     │ WebSocket over TCP
                    │ Optional TLS 1.3
                    │
Network Layer       │ IP (Local Network Only)
```

### Encryption Algorithms

**Symmetric Encryption:**
- **Algorithm**: AES-256-GCM (Galois/Counter Mode)
- **Key Size**: 256 bits (32 bytes)
- **IV/Nonce**: 96 bits (12 bytes), randomly generated
- **Authentication**: Built-in AEAD (Authenticated Encryption with Associated Data)

**Key Derivation:**
- **Function**: Argon2id (memory-hard, side-channel resistant)
- **Iterations**: 100,000 (configurable)
- **Memory**: 64 MB
- **Parallelism**: 4 threads
- **Salt**: 128-bit random salt per device pair

**Session Key Management:**
```rust
// Pseudocode for session key derivation
fn derive_session_key(shared_secret: &[u8], salt: &[u8]) -> [u8; 32] {
    argon2id_derive(
        shared_secret,
        salt,
        iterations: 100_000,
        memory: 64 * 1024 * 1024,  // 64 MB
        parallelism: 4,
        output_length: 32
    )
}
```

### Data-at-Rest Protection

**Clipboard History Encryption:**
- **Database**: SQLite with SQLCipher
- **Encryption**: AES-256-CBC
- **Key Storage**: Separate encrypted key file
- **Page Size**: 4096 bytes (encrypted)

**Key File Protection:**
```bash
# History encryption key location
~/.config/clipsync/history.key        # Linux
~/Library/Application Support/clipsync/history.key  # macOS

# Permissions
chmod 600 history.key                 # Read/write owner only
```

**Encryption Key Derivation:**
```
Master Password (derived from SSH key)
           │
           ▼
    PBKDF2-SHA256 (100,000 iterations)
           │
           ▼
    History Database Key (256-bit)
```

## 🌐 Network Security

### Network Architecture

ClipSync operates exclusively on **local area networks (LAN)** with no internet connectivity required:

```
┌─────────────────┐    Local Network    ┌─────────────────┐
│   Device A      │ ◄─────────────────► │   Device B      │
│ 192.168.1.10    │   Direct P2P Comm  │ 192.168.1.20    │
│ Port 8484       │                     │ Port 8484       │
└─────────────────┘                     └─────────────────┘
         │                                       │
         ▼                                       ▼
┌─────────────────┐                     ┌─────────────────┐
│ mDNS Discovery  │                     │ mDNS Discovery  │
│ _clipsync._tcp  │                     │ _clipsync._tcp  │
└─────────────────┘                     └─────────────────┘
```

### Service Discovery Security

**mDNS/DNS-SD Security:**
- Service announcements limited to local network
- No sensitive information in service records
- Device fingerprinting through service names
- Automatic network change detection

**Discovery Information:**
```
Service Type:    _clipsync._tcp.local
Instance Name:   hostname-clipsync
Port:           8484
TXT Records:    version=1.0.0
                protocol=clipsync-v1
                auth=ssh-ed25519
```

### Network Protocol Security

**WebSocket Security Enhancements:**
```rust
// Connection security measures
struct SecureWebSocket {
    // Rate limiting
    max_connections_per_ip: 3,
    connection_timeout: Duration::from_secs(30),
    
    // Message limits
    max_message_size: 5 * 1024 * 1024,  // 5MB
    max_messages_per_second: 10,
    
    // Security headers
    require_origin_check: false,  // Local network only
    disable_compression: false,   // Prevent compression attacks
}
```

**Anti-Spoofing Measures:**
- Source IP validation within local network ranges
- Connection rate limiting per IP address
- Message size and frequency limits
- Automatic blacklisting of malicious peers

## 🔍 Threat Model Analysis

### Threat Actors

**1. Local Network Attacker**
- **Capability**: Can intercept/modify network traffic on same LAN
- **Goal**: Eavesdrop on clipboard content or inject malicious data
- **Mitigation**: End-to-end encryption, device authentication

**2. Malicious Device on Network**
- **Capability**: Valid network access, attempts unauthorized connection
- **Goal**: Access clipboard data without authorization
- **Mitigation**: SSH key authentication, authorized keys validation

**3. Compromised Device**
- **Capability**: Has valid SSH keys, but device is compromised
- **Goal**: Abuse legitimate access to steal or modify clipboard data
- **Mitigation**: Key rotation, audit logging, device monitoring

**4. Physical Access Attacker**
- **Capability**: Physical access to device
- **Goal**: Extract encryption keys or clipboard history
- **Mitigation**: File system encryption, secure key storage

### Attack Scenarios and Mitigations

#### Scenario 1: Man-in-the-Middle Attack

**Attack:**
```
Attacker intercepts WebSocket traffic between devices:

Device A ──────X Attacker X────────► Device B
                    │
                    ▼
              [Intercepted Data]
```

**Mitigation:**
- **End-to-end encryption** with AES-256-GCM
- **SSH key authentication** prevents unauthorized decryption
- **Perfect forward secrecy** limits exposure if keys compromised
- **Message authentication** detects tampering

#### Scenario 2: Unauthorized Device Connection

**Attack:**
```
Malicious Device attempts to connect:

Attacker Device ──────► Target Device
     │                       │
     ├── Fake SSH Key        │
     │                       ▼
     ▼                  [Reject Connection]
[Connection Denied]
```

**Mitigation:**
- **Authorized keys file** restricts access to known devices
- **Strict host key checking** prevents unknown connections
- **Connection logging** for audit trail

#### Scenario 3: Clipboard History Theft

**Attack:**
Physical access to device, attempts to read clipboard history database.

**Mitigation:**
- **SQLCipher encryption** protects database contents
- **Separate key file** with restricted permissions
- **Key derivation** from SSH private key
- **File system permissions** (600) prevent other users

#### Scenario 4: Replay Attack

**Attack:**
Attacker captures and replays clipboard sync messages.

**Mitigation:**
- **Message timestamps** prevent old message replay
- **Sequence numbers** ensure message ordering
- **Session keys** expire after timeout
- **Nonce-based encryption** prevents identical ciphertexts

### Risk Assessment Matrix

| Threat | Likelihood | Impact | Risk Level | Mitigation Status |
|--------|------------|--------|------------|-------------------|
| MITM on Local Network | Medium | High | Medium | ✅ Fully Mitigated |
| Unauthorized Device | High | High | High | ✅ Fully Mitigated |
| Key Compromise | Low | High | Medium | ✅ Mostly Mitigated |
| Physical Access | Low | Medium | Low | ✅ Mostly Mitigated |
| Replay Attack | Low | Low | Low | ✅ Fully Mitigated |
| DoS Attack | Medium | Low | Low | ⚠️ Partially Mitigated |

## 🔧 Security Configuration

### Secure Configuration Template

```toml
# Maximum security configuration
listen_addr = "127.0.0.1:8484"         # Localhost only (testing)
# listen_addr = "192.168.1.100:8484"   # Specific IP (production)

[auth]
ssh_key = "~/.ssh/id_ed25519_clipsync"  # Dedicated ClipSync key
authorized_keys = "~/.config/clipsync/authorized_keys"
strict_host_key_checking = true         # Reject unknown peers
allow_self_signed = false               # Require proper SSH signatures

[clipboard]
max_size = 1_048_576                    # 1MB limit (reduce attack surface)
history_size = 10                       # Limited history
excluded_apps = [                       # Don't sync from security apps
    "1Password", "Bitwarden", "KeePassXC", "LastPass",
    "Keychain Access", "Authy", "Google Authenticator"
]

[security]
encryption = "aes-256-gcm"              # Strong encryption
key_derivation = "argon2id"             # Memory-hard KDF
key_iterations = 200_000                # Higher iteration count
compression = "none"                    # Disable to prevent compression attacks
encrypt_history = true                  # Encrypt local database
audit_enabled = true                    # Enable audit logging

[logging]
level = "info"                          # Standard logging
audit_file = "~/.config/clipsync/audit.log"
[logging.modules]
"clipsync::auth" = "info"              # Log authentication events
"clipsync::transport" = "warn"         # Log transport errors only
```

### Key Rotation Procedure

**Regular Key Rotation (Recommended: Every 6 months)**

```bash
#!/bin/bash
# Key rotation script

# 1. Generate new key pair
ssh-keygen -t ed25519 -f ~/.ssh/id_ed25519_clipsync_new -C "clipsync-$(hostname)-$(date +%Y%m%d)"

# 2. Backup old keys
cp ~/.ssh/id_ed25519_clipsync ~/.ssh/id_ed25519_clipsync.old
cp ~/.ssh/id_ed25519_clipsync.pub ~/.ssh/id_ed25519_clipsync.pub.old

# 3. Replace keys
mv ~/.ssh/id_ed25519_clipsync_new ~/.ssh/id_ed25519_clipsync
mv ~/.ssh/id_ed25519_clipsync_new.pub ~/.ssh/id_ed25519_clipsync.pub

# 4. Update configuration
clipsync config edit
# Verify ssh_key path points to new key

# 5. Exchange new public keys with all devices
echo "New public key (share with other devices):"
cat ~/.ssh/id_ed25519_clipsync.pub

# 6. Restart ClipSync
clipsync restart

# 7. Test connectivity
clipsync peers

# 8. Remove old keys after confirming all devices updated
# rm ~/.ssh/id_ed25519_clipsync.old*
```

### Security Hardening Checklist

**File System Security:**
- [ ] SSH private keys have 600 permissions
- [ ] Config directory has 700 permissions  
- [ ] History database has 600 permissions
- [ ] Audit logs have 600 permissions
- [ ] No world-readable ClipSync files

**Network Security:**
- [ ] Firewall allows ClipSync port only from trusted networks
- [ ] ClipSync bound to specific IP (not 0.0.0.0)
- [ ] Regular monitoring of network connections
- [ ] No ClipSync traffic leaves local network

**Authentication Security:**
- [ ] Unique SSH keys for ClipSync (not shared with SSH)
- [ ] Regular key rotation (every 6 months)
- [ ] Authorized keys file regularly audited
- [ ] Unknown devices automatically rejected

**Configuration Security:**
- [ ] Strong encryption algorithms selected
- [ ] High iteration counts for key derivation
- [ ] Audit logging enabled
- [ ] Sensitive applications excluded from sync
- [ ] Configuration file regularly validated

## 📊 Security Monitoring

### Audit Logging

**Enable Comprehensive Audit Logging:**
```toml
[security]
audit_enabled = true
audit_file = "~/.config/clipsync/audit.log"
audit_level = "info"

[logging]
level = "info"
file = true
[logging.modules]
"clipsync::auth" = "info"      # Authentication events
"clipsync::transport" = "info" # Network events
"clipsync::clipboard" = "warn" # Clipboard events (privacy)
```

**Audit Log Format:**
```json
{
  "timestamp": "2024-01-15T10:30:45Z",
  "level": "INFO", 
  "event": "authentication_success",
  "peer_id": "desktop-workstation",
  "peer_ip": "192.168.1.50",
  "auth_method": "ssh_ed25519",
  "session_id": "a1b2c3d4"
}

{
  "timestamp": "2024-01-15T10:31:02Z",
  "level": "WARN",
  "event": "authentication_failed", 
  "peer_ip": "192.168.1.99",
  "reason": "unknown_public_key",
  "attempts": 3
}
```

### Security Metrics

**Monitor these metrics for security issues:**

```bash
# Failed authentication attempts
grep "authentication_failed" ~/.config/clipsync/audit.log | wc -l

# Unique IP addresses connecting
grep "authentication_" ~/.config/clipsync/audit.log | cut -d'"' -f8 | sort -u

# Large payload transfers (potential data exfiltration)
grep "large_payload" ~/.config/clipsync/audit.log

# Connection patterns
grep "connection_established" ~/.config/clipsync/audit.log | \
  awk '{print $1}' | sort | uniq -c
```

### Automated Security Monitoring

**Security Alert Script:**
```bash
#!/bin/bash
# ~/.local/bin/clipsync-security-monitor

AUDIT_LOG="$HOME/.config/clipsync/audit.log"
ALERT_THRESHOLD=5

# Check for suspicious activity
if [ -f "$AUDIT_LOG" ]; then
    # Check for failed authentication attempts
    FAILED_AUTHS=$(grep "authentication_failed" "$AUDIT_LOG" | \
                   tail -100 | grep "$(date +%Y-%m-%d)" | wc -l)
    
    if [ "$FAILED_AUTHS" -gt "$ALERT_THRESHOLD" ]; then
        echo "SECURITY ALERT: $FAILED_AUTHS failed authentication attempts today"
        # Send notification or email
    fi
    
    # Check for unknown IP addresses
    UNKNOWN_IPS=$(grep "unknown_public_key" "$AUDIT_LOG" | \
                  tail -50 | cut -d'"' -f8 | sort -u)
    
    if [ -n "$UNKNOWN_IPS" ]; then
        echo "SECURITY ALERT: Unknown devices attempted connection:"
        echo "$UNKNOWN_IPS"
    fi
fi
```

**Add to crontab for regular monitoring:**
```bash
# Run security check every hour
0 * * * * ~/.local/bin/clipsync-security-monitor
```

## 🚨 Security Incident Response

### Suspected Compromise

**Immediate Actions:**
1. **Stop ClipSync service:** `clipsync stop`
2. **Isolate affected devices** from network
3. **Review audit logs** for unauthorized access
4. **Change all SSH keys** immediately
5. **Clear clipboard history:** `clipsync history --clear`

**Investigation Steps:**
```bash
# 1. Check for unauthorized connections
grep "authentication_success" ~/.config/clipsync/audit.log | \
  grep "$(date +%Y-%m-%d)"

# 2. Identify suspicious IP addresses
grep "authentication_" ~/.config/clipsync/audit.log | \
  cut -d'"' -f8 | sort | uniq -c | sort -nr

# 3. Check for data exfiltration
grep "large_payload\|clipboard_sync" ~/.config/clipsync/audit.log | \
  tail -100

# 4. Review system logs
journalctl -u clipsync --since "24 hours ago"  # Linux
log show --predicate 'process == "clipsync"' --last 24h  # macOS
```

**Recovery Actions:**
```bash
# 1. Generate new SSH keys
ssh-keygen -t ed25519 -f ~/.ssh/id_ed25519_clipsync_recovery

# 2. Remove all authorized keys
> ~/.config/clipsync/authorized_keys

# 3. Reset configuration
clipsync config init --force

# 4. Clear encrypted history
rm ~/.local/share/clipsync/history.db*
rm ~/.config/clipsync/history.key

# 5. Restart with new keys
clipsync start --foreground

# 6. Re-exchange keys with trusted devices only
```

### Vulnerability Disclosure

**Reporting Security Issues:**
- **Email**: security@clipsync.dev
- **GPG Key**: Available on GitHub repository
- **Response Time**: 48 hours for acknowledgment
- **Disclosure**: Coordinated disclosure after fix

**Information to Include:**
- Detailed vulnerability description
- Steps to reproduce
- Potential impact assessment
- Suggested mitigation (if any)
- Your contact information

## 🔮 Future Security Enhancements

### Planned Security Features

**Short Term (Next Release):**
- [ ] Hardware security module (HSM) support
- [ ] Multi-factor authentication options
- [ ] Enhanced audit logging with SIEM integration
- [ ] Automatic key rotation scheduling

**Medium Term:**
- [ ] Post-quantum cryptography preparation
- [ ] Zero-knowledge architecture improvements
- [ ] Distributed authentication model
- [ ] Enhanced anomaly detection

**Long Term:**
- [ ] Formal security verification
- [ ] Hardware-backed attestation
- [ ] Quantum-resistant algorithms
- [ ] Advanced threat protection

### Security Research

We welcome security research and responsible disclosure:

- **Bug Bounty**: Contact us for bug bounty program details
- **Academic Research**: We support academic security research
- **Security Audits**: Regular third-party security audits
- **Open Source**: All cryptographic code is open for review

## 📚 Security Resources

### Further Reading

- [Ed25519 Cryptographic Specification](https://tools.ietf.org/html/rfc8032)
- [AES-GCM Security Analysis](https://tools.ietf.org/html/rfc5116)
- [Argon2 Password Hashing](https://tools.ietf.org/html/rfc9106)
- [SSH Protocol Security](https://tools.ietf.org/html/rfc4251)

### Security Tools

**Recommended Security Tools:**
```bash
# Network analysis
nmap -sS -O target_ip              # Port scanning
wireshark                          # Packet analysis
tcpdump -i any port 8484          # Traffic monitoring

# Cryptographic verification
openssl verify -CAfile ca.pem cert.pem  # Certificate verification
ssh-keygen -l -f public_key.pub         # Key fingerprint

# File integrity
shasum -a 256 clipsync                   # Binary verification
find ~/.config/clipsync -type f -exec ls -la {} \;  # Permission audit
```

### Security Contacts

- **Security Team**: security@clipsync.dev
- **PGP Key**: [Download from GitHub](https://github.com/lancekrogers/clipsync/blob/main/SECURITY.asc)
- **Security Advisory**: [GitHub Security Advisories](https://github.com/lancekrogers/clipsync/security/advisories)

---

**Remember**: Security is a shared responsibility. While ClipSync provides strong cryptographic protections, users must follow security best practices for device management, key storage, and network security.

**Stay secure!** 🔐