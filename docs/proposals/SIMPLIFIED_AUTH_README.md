# ClipSync Simplified Authentication

ClipSync now features a zero-configuration authentication system that makes setup as simple as:

1. Install ClipSync on both devices
2. Run `clipsync start` on both
3. Accept trust prompt when devices find each other
4. Done! Clipboards now sync automatically

## How It Works

### 1. Automatic Public Key Discovery
- Config only needs the private key path (e.g., `~/.ssh/id_ed25519`)
- System automatically finds the corresponding `.pub` file
- No manual public key configuration required

### 2. mDNS Service Announcement with Public Keys
When a device starts ClipSync:
- It includes its SSH public key in the mDNS TXT records
- Other devices on the network can discover both the service and the public key
- Everything happens automatically on the local network

### 3. Trust On First Use (TOFU) Model
When devices discover each other:
```
=== New Device Discovered ===
Device Name: laptop
Device ID: 71e47505-c967-4b86-a319-ee483ee9e9ff
Address: 192.168.1.100:8484
SSH Fingerprint: SHA256:klGvenycIoKpDVwOpCL5LsXEvhdpEaSvt3a8hO4X9oo

Do you want to trust this device?
[y] Yes, trust this device
[n] No, reject this device
[i] Ignore for now (ask again later)

Your choice [y/n/i]: 
```

### 4. Automatic Key Management
Once you trust a device:
- Its public key is automatically added to `~/.config/clipsync/authorized_keys`
- The trust decision is saved in `~/.config/clipsync/trusted_devices.json`
- Future connections happen automatically without prompts

## Configuration

The minimal configuration now looks like:
```toml
[auth]
ssh_key = "~/.ssh/id_ed25519"
# That's it! No need to specify public key or manage authorized_keys
```

## Security Features

1. **Local Network Only**: Device discovery only works on the local network
2. **SSH Key Verification**: Uses standard SSH Ed25519 keys for authentication
3. **Trust Persistence**: Trust decisions are saved and survive restarts
4. **Fingerprint Display**: Shows SSH key fingerprints for verification
5. **Explicit Trust**: Users must explicitly trust each device

## Technical Details

### mDNS TXT Records
The service now advertises:
```
txt_data = {
  "id": "device-uuid",
  "pubkey": "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAI...",
  "version": "1.0.0",
  "platform": "macos"
}
```

### Trust Storage
Trust decisions are stored in `~/.config/clipsync/trusted_devices.json`:
```json
{
  "SHA256:abc123...": {
    "peer_id": "uuid",
    "peer_name": "laptop",
    "fingerprint": "SHA256:abc123...",
    "first_seen": 1234567890,
    "trusted_at": 1234567890,
    "is_trusted": true
  }
}
```

### Authorized Keys Management
The `~/.config/clipsync/authorized_keys` file is automatically maintained:
```
# ClipSync authorized keys file
# This file contains public keys authorized to connect to this ClipSync instance

ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAI... ClipSync: laptop (71e47505-c967-4b86-a319-ee483ee9e9ff)
```

## Comparison with Manual Setup

### Before (Manual):
1. Generate SSH keys on both devices
2. Copy public key from device A
3. Add to authorized_keys on device B
4. Copy public key from device B
5. Add to authorized_keys on device A
6. Start ClipSync on both devices

### After (Automatic):
1. Start ClipSync on both devices
2. Accept trust prompt
3. Done!

## Implementation Components

1. **Trust Manager** (`src/auth/trust.rs`): Handles trust decisions and persistence
2. **Enhanced mDNS** (`src/discovery/mdns.rs`): Includes public keys in announcements
3. **Trust Integration** (`src/discovery/trust_integration.rs`): Connects discovery with trust
4. **Auto Key Management** (`src/auth/authorized.rs`): Automatic authorized_keys updates

## Future Enhancements

1. **GUI Trust Dialogs**: Native OS dialogs for trust prompts
2. **QR Code Verification**: Optional QR codes for out-of-band verification
3. **Trust Revocation UI**: Easy way to revoke trust for devices
4. **Trust Sync**: Sync trust decisions across devices
5. **Temporary Trust**: Time-limited trust for guest devices