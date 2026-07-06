# build.ps1 - Windows PowerShell Build Script
#
# This script automates the process of building and running the Docker container
# with version information dynamically injected at build time.

# Stop script execution on any error
$ErrorActionPreference = "Stop"

$env:VERSION = (git describe --tags --always --dirty)
$env:COMMIT  = (git rev-parse --short HEAD)
$env:BUILD_DATE = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")

Write-Host "--- Building embedded image and starting services ---"
Write-Host "  Version: $env:VERSION"
Write-Host "  Commit: $env:COMMIT"
Write-Host "  Build Date: $env:BUILD_DATE"
Write-Host "----------------------------------------"

$env:CLI_PROXY_IMAGE = "ox-cli-proxy-api:local"

docker compose up -d --build --pull never --remove-orphans

Write-Host "Services are starting."
Write-Host "Run 'docker compose logs -f' to see the logs."
