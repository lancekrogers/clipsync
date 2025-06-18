#!/bin/bash
# Setup systemd-nspawn container for isolated clipsync testing

set -e

# Source local config if it exists
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")"; pwd)"
[ -f "$SCRIPT_DIR/local-config.sh" ] && source "$SCRIPT_DIR/local-config.sh"

CONTAINER_NAME="${CONTAINER_NAME:-clipsync-dev}"
CONTAINER_PATH="/var/lib/machines/$CONTAINER_NAME"

echo "Setting up systemd-nspawn container for clipsync development..."

# Install required packages
sudo pacman -S --needed arch-install-scripts systemd-container

# Create container directory
sudo mkdir -p "$CONTAINER_PATH"

# Bootstrap a minimal Arch installation
sudo pacstrap -c "$CONTAINER_PATH" base base-devel rustup git vim nano

# Configure the container
sudo systemd-nspawn -D "$CONTAINER_PATH" <<EOF
# Set root password
echo "root:clipsync" | chpasswd

# Create development user
useradd -m -G wheel developer
echo "developer:developer" | chpasswd

# Enable wheel group sudo access
echo "%wheel ALL=(ALL) NOPASSWD: ALL" > /etc/sudoers.d/wheel

# Install development dependencies
pacman -S --noconfirm wayland wayland-protocols libxkbcommon cairo pango gtk3 xorg-server-xwayland

# Setup locale
echo "en_US.UTF-8 UTF-8" > /etc/locale.gen
locale-gen
echo "LANG=en_US.UTF-8" > /etc/locale.conf
EOF

# Create a systemd service for easy container management
cat << 'EOF' | sudo tee "/etc/systemd/system/systemd-nspawn@$CONTAINER_NAME.service"
[Unit]
Description=Container %i
Documentation=man:systemd-nspawn(1)

[Service]
ExecStart=/usr/bin/systemd-nspawn --quiet --keep-unit --boot --link-journal=try-guest --network-veth --machine=%i
KillMode=mixed
Type=notify
RestartForceExitStatus=133
SuccessExitStatus=133

[Install]
WantedBy=machines.target
EOF

# Create start script
cat << 'EOF' | sudo tee /usr/local/bin/clipsync-container
#!/bin/bash
# Start the clipsync development container

case "$1" in
    start)
        sudo systemctl start systemd-nspawn@clipsync-dev
        ;;
    stop)
        sudo systemctl stop systemd-nspawn@clipsync-dev
        ;;
    shell)
        CURRENT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.."; pwd)"
        sudo systemd-nspawn -D /var/lib/machines/clipsync-dev --bind="$CURRENT_DIR":/home/developer/clipsync -u developer
        ;;
    *)
        echo "Usage: $0 {start|stop|shell}"
        echo "  start - Start the container in background"
        echo "  stop  - Stop the container"
        echo "  shell - Enter the container as developer user with clipsync mounted"
        exit 1
        ;;
esac
EOF

sudo chmod +x /usr/local/bin/clipsync-container

echo "Container setup complete!"
echo "Usage:"
echo "  clipsync-container shell  - Enter the development container"
echo "  clipsync-container start  - Start container in background"
echo "  clipsync-container stop   - Stop the container"