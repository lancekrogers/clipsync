#!/bin/bash

# Script to remove Claude co-authored-by messages from git history

# First, stash any uncommitted changes
echo "Stashing uncommitted changes..."
git stash

# Create a backup branch just in case
echo "Creating backup branch..."
git branch backup-before-claude-removal

# Use git filter-branch to remove the co-authored-by lines
echo "Removing Claude co-authored-by messages..."
git filter-branch --msg-filter '
    sed -e "/Co-Authored-By: Claude <noreply@anthropic.com>/d" \
        -e "/🤖 Generated with \[Claude Code\]/d"
' --tag-name-filter cat -- --all

echo "Done! Your commits have been rewritten."
echo "If everything looks good, you can delete the backup branch with:"
echo "  git branch -D backup-before-claude-removal"
echo ""
echo "To restore your stashed changes, run:"
echo "  git stash pop"
echo ""
echo "WARNING: You'll need to force push if you've already pushed these commits:"
echo "  git push --force-with-lease origin develop"