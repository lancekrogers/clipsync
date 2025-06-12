#!/bin/bash

# Source environment variables if .env exists
if [ -f .env ]; then
    source .env
fi

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo -e "${YELLOW}Running local tests (non-blocking)...${NC}\n"

# Track overall status
OVERALL_STATUS=0

# 1. Format check
echo -e "${YELLOW}1. Checking code formatting...${NC}"
if cargo fmt --check 2>&1; then
    echo -e "${GREEN}✓ Formatting check passed${NC}\n"
else
    echo -e "${RED}✗ Formatting issues found. Run 'cargo fmt' to fix.${NC}\n"
    OVERALL_STATUS=1
fi

# 2. Clippy linting
echo -e "${YELLOW}2. Running clippy...${NC}"
if cargo clippy -- -D warnings 2>&1; then
    echo -e "${GREEN}✓ Clippy check passed${NC}\n"
else
    echo -e "${RED}✗ Clippy warnings found${NC}\n"
    OVERALL_STATUS=1
fi

# 3. Build check
echo -e "${YELLOW}3. Running cargo check...${NC}"
if cargo check --all-targets 2>&1; then
    echo -e "${GREEN}✓ Build check passed${NC}\n"
else
    echo -e "${RED}✗ Build check failed${NC}\n"
    OVERALL_STATUS=1
fi

# 4. Run tests
echo -e "${YELLOW}4. Running tests...${NC}"
if cargo test 2>&1; then
    echo -e "${GREEN}✓ All tests passed${NC}\n"
else
    echo -e "${RED}✗ Some tests failed${NC}\n"
    OVERALL_STATUS=1
fi

# 5. Check for uncommitted changes
echo -e "${YELLOW}5. Checking for uncommitted changes...${NC}"
if [[ -n $(git status -s) ]]; then
    echo -e "${YELLOW}⚠ You have uncommitted changes:${NC}"
    git status -s
    echo ""
fi

# Summary
echo -e "\n${YELLOW}=== Summary ===${NC}"
if [ $OVERALL_STATUS -eq 0 ]; then
    echo -e "${GREEN}All checks passed! ✓${NC}"
else
    echo -e "${RED}Some checks failed. Please fix the issues above before committing.${NC}"
fi

exit $OVERALL_STATUS