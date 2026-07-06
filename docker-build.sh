#!/usr/bin/env bash
#
# build.sh - Linux/macOS Build Script
#
# This script automates the process of building and running the Docker container
# with version information dynamically injected at build time.

set -euo pipefail

if [[ "${1:-}" != "" ]]; then
  echo "Error: unknown option '${1}'."
  echo "Usage: ./docker-build.sh"
  exit 1
fi

VERSION="$(git describe --tags --always --dirty)"
COMMIT="$(git rev-parse --short HEAD)"
BUILD_DATE="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

echo "--- Building embedded image and starting services ---"
echo "  Version: ${VERSION}"
echo "  Commit: ${COMMIT}"
echo "  Build Date: ${BUILD_DATE}"
echo "----------------------------------------"

export CLI_PROXY_IMAGE="ox-cli-proxy-api:local"
export VERSION
export COMMIT
export BUILD_DATE

docker compose up -d --build --pull never --remove-orphans

echo "Services are starting."
echo "Run 'docker compose logs -f' to see the logs."
