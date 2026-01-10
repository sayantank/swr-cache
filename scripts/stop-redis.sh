#!/bin/bash
set -e

echo "Stopping Redis..."
docker compose down

echo "Redis stopped"
