#!/bin/bash
set -e

echo "Starting Redis..."
docker compose up -d redis

echo "Waiting for Redis to be ready..."
until docker compose exec redis redis-cli ping 2>/dev/null | grep -q PONG; do
  sleep 1
done

echo "Redis is ready on localhost:6379"
