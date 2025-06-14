#!/bin/bash

echo "Testing ClipSync daemon functionality..."

# Build the project
echo "Building project..."
cargo build --bin clipsync

# Test status when daemon is not running
echo -e "\n1. Testing status (daemon not running):"
./target/debug/clipsync status

# Start daemon in background
echo -e "\n2. Starting daemon:"
./target/debug/clipsync start

# Give it a moment to start
sleep 2

# Check status
echo -e "\n3. Checking status (daemon should be running):"
./target/debug/clipsync status

# Try to start again (should say already running)
echo -e "\n4. Trying to start daemon again:"
./target/debug/clipsync start

# Stop daemon
echo -e "\n5. Stopping daemon:"
./target/debug/clipsync stop

# Give it a moment to stop
sleep 1

# Check status again
echo -e "\n6. Final status check (daemon should be stopped):"
./target/debug/clipsync status

echo -e "\nDaemon test complete!"