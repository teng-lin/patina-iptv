[CmdletBinding()]
param([switch]$Check, [switch]$Release, [switch]$Run,
      [ValidateSet("full", "native")][string]$Tier = "full")
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
Push-Location $root
try {
    $env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
    $vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
    if (!(Test-Path $vswhere)) { throw 'Install Visual Studio Build Tools with Desktop development with C++.' }
    $vs = & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    if (!$vs) { throw 'The Visual Studio C++ workload is missing.' }
    # Cargo and cc discover the installed MSVC/SDK through Visual Studio's registry.
    # Keep the environment consistent with ordinary cargo commands to reuse native builds.
    if (Test-Path 'C:/Strawberry/perl/bin/perl.exe') {
        $env:PATH = "C:\Strawberry\perl\bin;C:\Strawberry\c\bin;$env:PATH"
    }
    if (!(Get-Command perl -ErrorAction SilentlyContinue)) {
        throw 'Install Strawberry Perl and put its perl/bin directory on PATH (SQLCipher builds OpenSSL).'
    }
    & "$PSScriptRoot/fetch-mpv-windows.ps1"
    $env:PATH = "$(Join-Path $root 'target/mpv');$env:PATH"
    if ($Check) {
        # The shared Rust plan selects the same features, ignored scenarios and
        # named libtest output as tools/check.sh. Platform extras stay here.
        & python "$PSScriptRoot/ci/pins.py" check-nsh
        if ($LASTEXITCODE -ne 0) { throw 'The NSH checkout is not at the commit in nsh.rev' }
        & cargo test --locked -p nsh-platform-windows --lib chrome::tests -- --format=pretty
        if ($LASTEXITCODE -ne 0) { throw 'Windows pointer regression failed' }
        $checkArgs = @("$PSScriptRoot/ci/check-plan.py", 'run', '--tier', $Tier, '--host', 'windows')
        if ($Release) { $checkArgs += '--release' }
        & python @checkArgs
        if ($LASTEXITCODE -ne 0) { throw 'Shared Rust validation failed' }
    }
    $buildArgs = @('build', '--locked', '-p', 'iptv-app')
    $profile = 'debug'
    if ($Release) { $buildArgs += '--release'; $profile = 'release' }
    if (!$Check) {
        & cargo @buildArgs
        if ($LASTEXITCODE -ne 0) { throw 'Windows app build failed' }
    }
    $output = Join-Path $root "target/$profile"
    Copy-Item -LiteralPath (Join-Path $root 'target/mpv/libmpv-2.dll') -Destination $output
    Write-Host "Built $output/iptv-app.exe"
    if ($Run) { & (Join-Path $output 'iptv-app.exe') }
} finally { Pop-Location }
