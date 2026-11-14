# Pinned x64 libmpv runtime. Build inputs stay under the ignored target tree.
[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$destination = Join-Path $root 'target/mpv'
$pin = Get-Content -Raw (Join-Path $PSScriptRoot 'mpv-runtime/runtime.json') | ConvertFrom-Json
if ($pin.schema -ne 1 -or $pin.sha256 -notmatch '^[a-f0-9]{64}$' -or $pin.dll_sha256 -notmatch '^[a-f0-9]{64}$') {
    throw 'Invalid pinned libmpv runtime manifest'
}
$archive = python (Join-Path $PSScriptRoot 'ci/native_candidates.py') windows
if ($LASTEXITCODE -ne 0) { throw 'Could not fetch the verified native runtime.' }
$archive = $archive.Trim()
Expand-Archive -LiteralPath $archive -DestinationPath $destination -Force
if ((Get-FileHash (Join-Path $destination 'libmpv-2.dll') -Algorithm SHA256).Hash -ne $pin.dll_sha256) {
    throw 'libmpv DLL checksum mismatch'
}
Write-Host "libmpv ready in $destination"
