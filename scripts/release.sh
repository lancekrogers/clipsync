#!/bin/bash
set -e

# ClipSync Release Script
# This script automates the release process for ClipSync

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
CURRENT_VERSION=""
NEW_VERSION=""
RELEASE_BRANCH="release"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Print colored output
print_info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

print_success() {
    echo -e "${GREEN}[SUCCESS]${NC} $1"
}

print_warning() {
    echo -e "${YELLOW}[WARNING]${NC} $1"
}

print_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

# Get current version from Cargo.toml
get_current_version() {
    CURRENT_VERSION=$(grep '^version = ' "$PROJECT_ROOT/Cargo.toml" | head -1 | cut -d'"' -f2)
    print_info "Current version: $CURRENT_VERSION"
}

# Validate semantic version
validate_version() {
    if ! [[ "$1" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[a-zA-Z0-9.-]+)?$ ]]; then
        print_error "Invalid version format. Please use semantic versioning (e.g., 1.2.3 or 1.2.3-beta.1)"
        exit 1
    fi
}

# Prompt for new version
prompt_new_version() {
    echo ""
    echo "Current version: $CURRENT_VERSION"
    echo ""
    echo "What type of release is this?"
    echo "1) Patch (bug fixes)"
    echo "2) Minor (new features, backwards compatible)"
    echo "3) Major (breaking changes)"
    echo "4) Custom version"
    echo ""
    read -p "Select release type (1-4): " release_type

    IFS='.' read -ra VERSION_PARTS <<< "$CURRENT_VERSION"
    MAJOR="${VERSION_PARTS[0]}"
    MINOR="${VERSION_PARTS[1]}"
    PATCH="${VERSION_PARTS[2]%%[-+]*}"  # Remove any pre-release or build metadata

    case $release_type in
        1)
            NEW_VERSION="$MAJOR.$MINOR.$((PATCH + 1))"
            ;;
        2)
            NEW_VERSION="$MAJOR.$((MINOR + 1)).0"
            ;;
        3)
            NEW_VERSION="$((MAJOR + 1)).0.0"
            ;;
        4)
            read -p "Enter custom version: " NEW_VERSION
            validate_version "$NEW_VERSION"
            ;;
        *)
            print_error "Invalid selection"
            exit 1
            ;;
    esac

    echo ""
    print_info "New version will be: $NEW_VERSION"
    read -p "Continue? (y/n): " -n 1 -r
    echo ""
    if [[ ! $REPLY =~ ^[Yy]$ ]]; then
        print_warning "Release cancelled"
        exit 1
    fi
}

# Update version in files
update_version() {
    print_info "Updating version to $NEW_VERSION..."

    # Update Cargo.toml
    sed -i.bak "s/^version = \".*\"/version = \"$NEW_VERSION\"/" "$PROJECT_ROOT/Cargo.toml"
    rm "$PROJECT_ROOT/Cargo.toml.bak"

    # Update Cargo.lock
    (cd "$PROJECT_ROOT" && cargo update --workspace)

    # Update version in shared file for other agents
    echo "$NEW_VERSION" > "$PROJECT_ROOT/ai_docs/task/parallel/sprint4/shared/version.txt"

    print_success "Version updated in all files"
}

# Run pre-release checks
run_checks() {
    print_info "Running pre-release checks..."

    # Check for uncommitted changes
    if ! git diff-index --quiet HEAD --; then
        print_error "There are uncommitted changes. Please commit or stash them first."
        exit 1
    fi

    # Run tests
    print_info "Running tests..."
    (cd "$PROJECT_ROOT" && cargo test --all-features)

    # Run clippy
    print_info "Running clippy..."
    (cd "$PROJECT_ROOT" && cargo clippy -- -D warnings)

    # Check formatting
    print_info "Checking formatting..."
    (cd "$PROJECT_ROOT" && cargo fmt -- --check)

    # Build release binary
    print_info "Building release binary..."
    (cd "$PROJECT_ROOT" && cargo build --release)

    print_success "All checks passed!"
}

# Update CHANGELOG.md
update_changelog() {
    print_info "Updating CHANGELOG.md..."

    CHANGELOG_FILE="$PROJECT_ROOT/CHANGELOG.md"
    TEMP_FILE=$(mktemp)

    # Get the date
    DATE=$(date +%Y-%m-%d)

    # Get commit messages since last tag
    LAST_TAG=$(git describe --tags --abbrev=0 2>/dev/null || echo "")
    if [ -z "$LAST_TAG" ]; then
        CHANGES=$(git log --pretty=format:"- %s (%an)" --no-merges)
    else
        CHANGES=$(git log --pretty=format:"- %s (%an)" --no-merges "${LAST_TAG}..HEAD")
    fi

    # Create new changelog entry
    {
        echo "# Changelog"
        echo ""
        echo "## [$NEW_VERSION] - $DATE"
        echo ""
        
        # Group changes by type
        echo "### Added"
        echo "$CHANGES" | grep -i "feat:" | sed 's/feat: //i' || echo "- No new features"
        echo ""
        
        echo "### Changed"
        echo "$CHANGES" | grep -i "refactor:\|perf:" | sed 's/\(refactor:\|perf:\) //i' || echo "- No changes"
        echo ""
        
        echo "### Fixed"
        echo "$CHANGES" | grep -i "fix:" | sed 's/fix: //i' || echo "- No fixes"
        echo ""
        
        echo "### Security"
        echo "$CHANGES" | grep -i "security:" | sed 's/security: //i' || echo "- No security updates"
        echo ""
        
        # Add the rest of the existing changelog
        if [ -f "$CHANGELOG_FILE" ]; then
            tail -n +2 "$CHANGELOG_FILE"
        fi
    } > "$TEMP_FILE"

    mv "$TEMP_FILE" "$CHANGELOG_FILE"
    print_success "CHANGELOG.md updated"
}

# Commit release changes
commit_release() {
    print_info "Committing release changes..."

    git add -A
    git commit -m "chore: release v$NEW_VERSION

- Bump version to $NEW_VERSION
- Update CHANGELOG.md
- Update Cargo.lock"

    print_success "Release changes committed"
}

# Create git tag
create_tag() {
    print_info "Creating git tag v$NEW_VERSION..."

    # Create annotated tag
    git tag -a "v$NEW_VERSION" -m "Release v$NEW_VERSION

See CHANGELOG.md for details."

    print_success "Git tag created"
}

# Push to remote
push_release() {
    print_info "Pushing to remote repository..."

    # Push commits
    git push origin main

    # Push tag
    git push origin "v$NEW_VERSION"

    print_success "Release pushed to remote repository"
}

# Main release flow
main() {
    print_info "Starting ClipSync release process..."

    # Change to project root
    cd "$PROJECT_ROOT"

    # Ensure we're on main branch
    CURRENT_BRANCH=$(git branch --show-current)
    if [ "$CURRENT_BRANCH" != "main" ]; then
        print_error "You must be on the main branch to create a release"
        exit 1
    fi

    # Pull latest changes
    print_info "Pulling latest changes..."
    git pull origin main

    # Get current version
    get_current_version

    # Prompt for new version
    prompt_new_version

    # Run pre-release checks
    run_checks

    # Update version in files
    update_version

    # Update changelog
    update_changelog

    # Commit release changes
    commit_release

    # Create tag
    create_tag

    # Final confirmation
    echo ""
    print_warning "Ready to push release v$NEW_VERSION to remote repository"
    echo "This will trigger the automated release workflow."
    read -p "Push release? (y/n): " -n 1 -r
    echo ""

    if [[ $REPLY =~ ^[Yy]$ ]]; then
        push_release
        echo ""
        print_success "Release v$NEW_VERSION completed successfully!"
        echo ""
        echo "Next steps:"
        echo "1. Monitor the GitHub Actions release workflow"
        echo "2. Verify the release artifacts are uploaded"
        echo "3. Update the release notes on GitHub if needed"
        echo "4. Announce the release"
    else
        print_warning "Release not pushed. To push manually, run:"
        echo "  git push origin main"
        echo "  git push origin v$NEW_VERSION"
    fi
}

# Run main function
main "$@"