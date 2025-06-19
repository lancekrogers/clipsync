# ClipSync Sandboxing Scripts

⚠️ **EXPERIMENTAL - UNDER ACTIVE DEVELOPMENT** ⚠️

These scripts provide various sandboxing options for safe development and testing of ClipSync. They are experimental and should be tested carefully in your environment before relying on them for security.

## Setup

1. Copy the example configuration:
```bash
cp local-config.example.sh local-config.sh
```

2. Edit `local-config.sh` with your preferences (this file is gitignored)

3. Make scripts executable:
```bash
chmod +x *.sh
```

## Available Scripts

- `safe-dev.sh` - Run clipsync with temporary home and restricted permissions
- `setup-sandbox-user.sh` - Create a dedicated test user for development
- `setup-nspawn-container.sh` - Create a systemd-nspawn container for full isolation
- `install-apparmor.sh` - Install AppArmor profile for production use

## Usage

See [SANDBOX.md](../SANDBOX.md) for detailed usage instructions.

## Note

All scripts use relative paths and environment variables, making them portable across different systems. User-specific configurations should be added to `local-config.sh` (not committed to git).