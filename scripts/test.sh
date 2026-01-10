#!/bin/bash
set -e

# Start Redis if not running
if ! docker compose ps redis 2>/dev/null | grep -q "running"; then
  ./scripts/start-redis.sh
fi

echo "Running tests..."
cargo test "$@"
