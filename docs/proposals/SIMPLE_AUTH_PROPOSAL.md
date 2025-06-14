# Simplified ClipSync Authentication Proposal

## Current Pain Points
1. Users must manually exchange SSH public keys between devices
2. No CLI commands exist for auth management (despite docs claiming they do)
3. authorized_keys file must be manually synchronized between devices
4. Setup is complex and error-prone

## Proposed Solution

### 1. Auto-discovery of Public Key
- Config only needs `ssh_key = "~/.ssh/id_ed25519"`
- System automatically finds `~/.ssh/id_ed25519.pub`
- Public key is included in mDNS advertisement

### 2. Trust On First Use (TOFU) Model
- When devices discover each other via mDNS, they exchange public keys
- User is prompted to trust new devices:
  ```
  New device discovered: "laptop" (fingerprint: abc123...)
  Trust this device? [y/N]
  ```
- Once trusted, the key is added to authorized_keys automatically

### 3. Simple Implementation

#### In mDNS TXT records, add:
```
txt_data = {
  "id": "device-uuid",
  "pubkey": "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAI...",
  "name": "device-name"
}
```

#### On discovery:
1. Extract public key from mDNS TXT record
2. Check if already in authorized_keys
3. If not, prompt user to trust
4. If trusted, add to authorized_keys

### 4. Benefits
- Zero manual key exchange needed
- Works automatically on same network
- Still secure (user must approve each device)
- Matches user expectations from similar tools

### 5. Implementation Steps
1. Update Config to auto-discover .pub file ✓
2. Add public key to mDNS advertisement
3. Add trust prompt on new device discovery
4. Auto-update authorized_keys when trusted
5. Remove need for manual key management

This would reduce setup from multiple manual steps to just:
1. Install ClipSync on both devices
2. Run `clipsync start` on both
3. Accept trust prompt when devices find each other
4. Done! Clipboards now sync automatically