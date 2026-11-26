[CmdletBinding()]
param([switch]$SkipBuild)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
Push-Location $root
try {
    $env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
    $version = (& python -c "import tomllib; print(tomllib.load(open('Cargo.toml','rb'))['workspace']['package']['version'])").Trim()
    if ($LASTEXITCODE -ne 0 -or $version -notmatch '^\d+\.\d+\.\d+$') { throw 'Installer needs a numeric workspace version.' }
    if ($env:VERSION -and $env:VERSION -ne $version) { throw 'Release VERSION differs from Cargo.toml.' }
    if (!$SkipBuild) {
        & "$PSScriptRoot/build-windows.ps1" -Release
    }
    $exe = Join-Path $root 'target/release/iptv-app.exe'
    & python "$PSScriptRoot/ci/check-windows-icon.py" $exe
    if ($LASTEXITCODE -ne 0) { throw 'Executable icon verification failed.' }
    $compiler = & "$PSScriptRoot/ci/setup-inno.ps1"
    $stage = Join-Path $root "target/windows-package/$([guid]::NewGuid())/Patina"
    New-Item -ItemType Directory -Force $stage | Out-Null
    [IO.File]::WriteAllText((Join-Path $stage 'patina-package'), 'exe')
    Copy-Item -LiteralPath $exe -Destination (Join-Path $stage 'Patina.exe')
    Copy-Item -LiteralPath (Join-Path $root 'target/mpv/libmpv-2.dll') -Destination $stage
    Copy-Item -LiteralPath (Join-Path $root 'crates/iptv-app/icon.ico') -Destination $stage
    # App-local VC runtime: installation must not require Visual Studio or elevation.
    $vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
    $vs = & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    $runtime = Get-ChildItem -Path "$vs/VC/Redist/MSVC/*/x64/Microsoft.VC143.CRT" -Directory |
        Sort-Object FullName -Descending | Select-Object -First 1
    if (!$runtime) { throw 'Visual C++ x64 redistributable runtime is missing.' }
    foreach ($name in @('vcruntime140.dll', 'vcruntime140_1.dll')) {
        Copy-Item -LiteralPath (Join-Path $runtime.FullName $name) -Destination $stage
    }
    & python "$PSScriptRoot/ci/windows-notices.py" $stage
    if ($LASTEXITCODE -ne 0) { throw 'Windows package notices failed.' }
    $out = Join-Path $root 'dist'
    New-Item -ItemType Directory -Force $out | Out-Null
    & $compiler "/DAppVersion=$version" "/DStageDir=$stage" "/DOutputDir=$out" "$PSScriptRoot/ci/windows-installer.iss"
    if ($LASTEXITCODE -ne 0) { throw 'Windows installer compilation failed.' }
    Write-Host "Installer: $out/Patina-$version-windows-x86_64-setup.exe"
} finally { Pop-Location }
