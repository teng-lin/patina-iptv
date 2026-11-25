[CmdletBinding()]
param([ValidateSet('Inputs', 'Package')][string]$Phase)
$ErrorActionPreference = 'Stop'
if ($Phase -eq 'Inputs') {
    # Match the shipping build script's command lookup order. Get-Command may
    # report several installed matches; ordinary invocation uses the first.
    $env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
    if (Test-Path 'C:/Strawberry/perl/bin/perl.exe') {
        $env:PATH = "C:\Strawberry\perl\bin;C:\Strawberry\c\bin;$env:PATH"
    }
    $vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
    $vs = & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    if ($LASTEXITCODE -ne 0 -or !$vs) { throw 'MSVC installation not found.' }
    $vc = (Get-Content -Raw "$vs/VC/Auxiliary/Build/Microsoft.VCToolsVersion.default.txt").Trim()
    $kits = Get-ItemPropertyValue 'HKLM:\SOFTWARE\Microsoft\Windows Kits\Installed Roots' KitsRoot10
    $sdks = @(Get-ChildItem "$kits/Include" -Directory |
        Where-Object { Test-Path (Join-Path $_.FullName 'um/Windows.h') } |
        Sort-Object { [version]$_.Name } | Select-Object -ExpandProperty Name)
    if (!$vc -or !$sdks.Count -or !$env:ImageOS -or !$env:ImageVersion) { throw 'Native build identity missing.' }
    $tools = @{}
    foreach ($name in @('perl', 'nasm', 'cmake', 'ninja')) {
        $tools[$name] = (Get-Command $name -CommandType Application -ErrorAction Stop | Select-Object -First 1).Source
    }
    $tools['cl'] = "$vs/VC/Tools/MSVC/$vc/bin/Hostx64/x64/cl.exe"
    $tools['link'] = "$vs/VC/Tools/MSVC/$vc/bin/Hostx64/x64/link.exe"
    $trees = @{
        'msvc-include' = "$vs/VC/Tools/MSVC/$vc/include"
        'msvc-lib' = "$vs/VC/Tools/MSVC/$vc/lib/x64"
        'msvc-bin' = "$vs/VC/Tools/MSVC/$vc/bin/Hostx64/x64"
        'perl-lib' = 'C:/Strawberry/perl/lib'
    }
    # cc/rustc discover SDKs through the registry. Bind every discoverable
    # version rather than guessing which one a dependency's build script chose.
    foreach ($sdk in $sdks) {
        $trees["sdk-$sdk-include"] = "$kits/Include/$sdk"
        $trees["sdk-$sdk-lib"] = "$kits/Lib/$sdk"
        $trees["sdk-$sdk-bin"] = "$kits/bin/$sdk/x64"
    }
    @{
        image_os = $env:ImageOS; image_version = $env:ImageVersion
        msvc = $vc; windows_sdks = $sdks; tools = $tools; trees = $trees
        # Fixed, nonsecret settings only. Do not serialize the environment.
        architecture = $env:PROCESSOR_ARCHITECTURE
    } | ConvertTo-Json -Depth 5 -Compress
    exit 0
}

# The original package script keeps icon, locked notices, runtime/license and
# MSVC redistribution checks. The original smoke test installs, launches and
# uninstalls using a temporary synthetic profile.
./tools/package-windows.ps1 -SkipBuild
if ($LASTEXITCODE -ne 0) { throw 'Windows packaging failed.' }
./tools/ci/check-windows-package.ps1
if ($LASTEXITCODE -ne 0) { throw 'Windows installer smoke check failed.' }
$output = "target/media-sbom/Patina-${env:VERSION}-windows-media-runtime-sbom.cdx.json"
python tools/ci/sbom.py media-runtime target/mpv/components.json $output "$env:VERSION" `
    --platform windows --manifest tools/mpv-runtime/runtime.json
if ($LASTEXITCODE -ne 0) { throw 'Media runtime SBOM failed.' }
python tools/ci/build-inventory.py "target/media-sbom/Patina-$env:VERSION-windows-build-environment.json"
if ($LASTEXITCODE -ne 0) { throw 'Build inventory failed.' }
