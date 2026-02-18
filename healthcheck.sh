#!/usr/bin/env bash
# Example healthcheck for git bisect run
# Tests if the persona responds correctly to a fixed prompt
# Exit 0 = good commit, Exit 1 = bad commit
echo "Testing persona at $(git rev-parse HEAD)..."
# Replace with your actual test:
# openclaw run --prompt "Who are you?" --temperature 0 | grep -q "expected response"
exit 0
