#!/bin/bash

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

echo -e "${BLUE}=== ClipSync Build Status ===${NC}\n"

# Source environment variables if .env exists
if [ -f .env ]; then
    source .env
fi

# 1. Check formatting
echo -e "${YELLOW}1. Formatting Check:${NC}"
if cargo fmt --check &>/dev/null; then
    echo -e "   ${GREEN}✓ Code is properly formatted${NC}"
else
    echo -e "   ${RED}✗ Code needs formatting (run: cargo fmt)${NC}"
fi

# 2. Count clippy warnings
echo -e "\n${YELLOW}2. Clippy Analysis:${NC}"
CLIPPY_OUTPUT=$(cargo clippy -- -D warnings 2>&1 || true)
CLIPPY_ERRORS=$(echo "$CLIPPY_OUTPUT" | grep -c "error:" || true)
if [ "$CLIPPY_ERRORS" -eq 0 ]; then
    echo -e "   ${GREEN}✓ No clippy warnings${NC}"
else
    echo -e "   ${RED}✗ Found $CLIPPY_ERRORS clippy warnings${NC}"
    echo -e "   ${BLUE}Common issues:${NC}"
    echo "$CLIPPY_OUTPUT" | grep "error:" | head -5 | sed 's/^/     /'
fi

# 3. Check if it builds
echo -e "\n${YELLOW}3. Build Status:${NC}"
if cargo check --all-targets &>/dev/null; then
    echo -e "   ${GREEN}✓ Project builds successfully${NC}"
else
    BUILD_OUTPUT=$(cargo check --all-targets 2>&1 || true)
    BUILD_ERRORS=$(echo "$BUILD_OUTPUT" | grep -c "error\[E" || true)
    echo -e "   ${RED}✗ Build failed with $BUILD_ERRORS errors${NC}"
    echo -e "   ${BLUE}First few errors:${NC}"
    echo "$BUILD_OUTPUT" | grep "error\[E" | head -3 | sed 's/^/     /'
fi

# 4. Git status
echo -e "\n${YELLOW}4. Git Status:${NC}"
MODIFIED_FILES=$(git status --porcelain | wc -l | tr -d ' ')
if [ "$MODIFIED_FILES" -eq 0 ]; then
    echo -e "   ${GREEN}✓ No uncommitted changes${NC}"
else
    echo -e "   ${YELLOW}⚠ $MODIFIED_FILES files with changes${NC}"
fi

# 5. Summary
echo -e "\n${BLUE}=== Summary ===${NC}"
echo -e "To test everything locally before committing:"
echo -e "  1. ${BLUE}cargo fmt${NC} - Fix formatting"
echo -e "  2. ${BLUE}cargo clippy --fix${NC} - Fix some clippy warnings automatically"
echo -e "  3. ${BLUE}cargo check${NC} - See all compilation errors"
echo -e "  4. ${BLUE}./scripts/test-local.sh${NC} - Run full test suite (non-blocking)"
echo -e "\nFor a clean commit history, fix all issues then commit once."