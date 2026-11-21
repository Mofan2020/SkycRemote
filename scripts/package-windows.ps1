# Build the Windows installer.
#
# The installer is a program of its own, with the files that are
# installed folded into it.  It is not NSIS and not any other packer:
# crates/skycremote-installer puts the files where they belong itself.
#
# What comes out:
#
#   target/dist/SkycRemote-Setup-<version>-x64.exe
#
# Usage: pwsh scripts/package-windows.ps1

$ErrorActionPreference = "Stop"

$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$version = (Select-String -Path "crates/skycremote-app/Cargo.toml" -Pattern '^version = "(.*)"' | Select-Object -First 1).Matches[0].Groups[1].Value
$out = Join-Path $root "target/dist"
New-Item -ItemType Directory -Force $out | Out-Null

Write-Host "building SkycRemote $version"

cargo build --release --bin skycremote --bin skycremote-cli
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# The files that get installed: the program, and the command line
# tool in a folder of its own so that the folder on the PATH holds
# nothing but the tool.
$payload = Join-Path $root "target/payload"
Remove-Item -Recurse -Force $payload -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force (Join-Path $payload "cli") | Out-Null
Copy-Item "target/release/skycremote.exe" (Join-Path $payload "SkycRemote.exe")
Copy-Item "target/release/skycremote-cli.exe" (Join-Path $payload "cli/skycremote.exe")

# The installer, with those files inside it.
$env:SKYCREMOTE_PAYLOAD = $payload
cargo build --release --bin skycremote-setup
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$setup = Join-Path $out "SkycRemote-Setup-$version-x64.exe"
Copy-Item "target/release/skycremote-setup.exe" $setup

Write-Host ""
Write-Host "done:"
Write-Host "  $setup"
