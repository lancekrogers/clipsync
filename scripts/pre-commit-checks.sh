#!/bin/bash
set -e

# Source environment variables if .env exists
if [ -f .env ]; then
    source .env
fi

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo -e "${YELLOW}Running pre-commit checks...${NC}\n"

# 1. Format check
echo -e "${YELLOW}Checking code formatting...${NC}"
if cargo fmt --check; then
    echo -e "${GREEN}✓ Formatting check passed${NC}\n"
else
    echo -e "${RED}✗ Formatting issues found. Run 'cargo fmt' to fix.${NC}\n"
    exit 1
fi

# 2. Clippy linting
echo -e "${YELLOW}Running clippy...${NC}"
if cargo clippy -- -D warnings; then
    echo -e "${GREEN}✓ Clippy check passed${NC}\n"
else
    echo -e "${RED}✗ Clippy warnings found${NC}\n"
    exit 1
fi

# 3. Build check
echo -e "${YELLOW}Running cargo check...${NC}"
if cargo check --all-targets; then
    echo -e "${GREEN}✓ Build check passed${NC}\n"
else
    echo -e "${RED}✗ Build check failed${NC}\n"
    exit 1
fi

# 4. Run tests
echo -e "${YELLOW}Running tests...${NC}"
if cargo test; then
    echo -e "${GREEN}✓ All tests passed${NC}\n"
else
    echo -e "${RED}✗ Tests failed${NC}\n"
    exit 1
fi

# 5. Check for uncommitted changes
echo -e "${YELLOW}Checking for uncommitted changes...${NC}"
if [[ -n $(git status -s) ]]; then
    echo -e "${YELLOW}⚠ You have uncommitted changes:${NC}"
    git status -s
    echo ""
fi

echo -e "${GREEN}All pre-commit checks passed! ✓${NC}"