#!/bin/bash
# Safe development wrapper for clipsync
# This script runs clipsync with restricted permissions during development

set -e

# Source local config if it exists
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")"; pwd)"
[ -f "$SCRIPT_DIR/local-config.sh" ] && source "$SCRIPT_DIR/local-config.sh"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo -e "${GREEN}Running clipsync in safe development mode...${NC}"

# Create a temporary directory for this session
TEMP_HOME=$(mktemp -d /tmp/clipsync-dev.XXXXXX)
trap "rm -rf $TEMP_HOME" EXIT

# Copy necessary config files (but not sensitive ones)
mkdir -p "$TEMP_HOME/.config/clipsync"
mkdir -p "$TEMP_HOME/.local/share/clipsync"

# Create a minimal config if it doesn't exist
if [ ! -f "$TEMP_HOME/.config/clipsync/config.toml" ]; then
    cat > "$TEMP_HOME/.config/clipsync/config.toml" << 'EOF'
[network]
discovery_port = 45445
sync_port = 45446

[sync]
sync_interval = 5
max_history_size = 100

[security]
encryption_enabled = false
EOF
fi

# Set restrictive environment
export HOME="$TEMP_HOME"
export USER="clipsync-dev"
export SHELL="/bin/false"

# Unset sensitive environment variables
unset SSH_AUTH_SOCK
unset GPG_AGENT_INFO
unset GNUPGHOME
unset AWS_ACCESS_KEY_ID
unset AWS_SECRET_ACCESS_KEY
unset GITHUB_TOKEN

# Use firejail if available for additional sandboxing
if command -v firejail &> /dev/null; then
    echo -e "${YELLOW}Running with firejail sandboxing...${NC}"
    exec firejail \
        --noprofile \
        --private="$TEMP_HOME" \
        --private-dev \
        --nosound \
        --no3d \
        --nodbus \
        --caps.drop=all \
        --seccomp \
        --noroot \
        --nonewprivs \
        --noexec=/tmp \
        --noexec="$HOME" \
        --read-only=/etc \
        --read-only=/usr \
        --read-only=/bin \
        --read-only=/sbin \
        --whitelist="$PWD/target" \
        -- "$@"
else
    echo -e "${YELLOW}Firejail not found. Running with basic isolation...${NC}"
    echo -e "${RED}Consider installing firejail for better sandboxing: sudo pacman -S firejail${NC}"
    
    # Run with reduced privileges using systemd-run if available
    if command -v systemd-run &> /dev/null; then
        exec systemd-run \
            --uid=nobody \
            --gid=nobody \
            --setenv=HOME="$TEMP_HOME" \
            --setenv=USER="clipsync-dev" \
            --property=PrivateTmp=yes \
            --property=ProtectSystem=strict \
            --property=ProtectHome=yes \
            --property=NoNewPrivileges=yes \
            --property=RestrictNamespaces=yes \
            --property=RestrictRealtime=yes \
            --property=MemoryLimit=512M \
            --property=CPUQuota=50% \
            --wait \
            --pipe \
            -- "$@"
    else
        # Fallback: just run with modified environment
        exec "$@"
    fi
fi