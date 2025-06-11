#!/bin/bash

echo "Testing ClipSync key storage..."

# Clean up any existing key file
if [[ "$OSTYPE" == "darwin"* ]]; then
    KEY_DIR="$HOME/Library/Application Support/clipsync"
else
    KEY_DIR="$HOME/.config/clipsync"
fi
KEY_FILE="$KEY_DIR/history.key"

if [ -f "$KEY_FILE" ]; then
    echo "Removing existing key file: $KEY_FILE"
    rm -f "$KEY_FILE"
fi

# Test 1: Verify key generation
echo -e "\nTest 1: Key generation"
./target/debug/clipsync history --limit 1 2>&1 | grep -E "(Initializing|key saved)"

# Test 2: Verify key file exists and has correct permissions
echo -e "\nTest 2: Key file verification"
if [ -f "$KEY_FILE" ]; then
    echo "Key file created: $KEY_FILE"
    
    # Check permissions (should be 600 on Unix)
    if [[ "$OSTYPE" == "darwin"* ]] || [[ "$OSTYPE" == "linux-gnu"* ]]; then
        PERMS=$(stat -c "%a" "$KEY_FILE" 2>/dev/null || stat -f "%Lp" "$KEY_FILE")
        echo "Key file permissions: $PERMS"
        
        if [ "$PERMS" == "600" ]; then
            echo "✓ Permissions are correct (600)"
        else
            echo "✗ Permissions are incorrect (expected 600, got $PERMS)"
        fi
    fi
    
    # Check file size (should be 32 bytes)
    SIZE=$(wc -c < "$KEY_FILE")
    echo "Key file size: $SIZE bytes"
    if [ "$SIZE" -eq 32 ]; then
        echo "✓ Key size is correct (32 bytes)"
    else
        echo "✗ Key size is incorrect (expected 32, got $SIZE)"
    fi
else
    echo "✗ Key file was not created"
fi

# Test 3: Verify key reuse
echo -e "\nTest 3: Key reuse on subsequent runs"
./target/debug/clipsync history --limit 1 2>&1 | grep -E "(Initializing|key saved)"

echo -e "\nKey storage tests complete."