# Pin the compiler independently of the hosted runner's preinstalled version.
$ErrorActionPreference = 'Stop'
$root = Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
$tools = Join-Path $root 'target/tools'
$destination = Join-Path $tools 'inno-7.1.1'
$compiler = Join-Path $destination 'ISCC.exe'
if (!(Test-Path -LiteralPath $compiler)) {
    New-Item -ItemType Directory -Force $tools | Out-Null
    $installer = Join-Path $tools 'innosetup-7.1.1-x64.exe'
    $hash = '8a5a6711c3766a6e5fb74f280ba0e3551e719ce94538d901fdbd1f8f9920b3f0'
    $candidate = $installer
    if (!(Test-Path -LiteralPath $installer)) {
        $candidate = "$installer.part"
        $url = 'https://github.com/jrsoftware/issrc/releases/download/is-7_1_1/innosetup-7.1.1-x64.exe'
        & curl.exe --fail --location --silent --show-error --retry 3 --connect-timeout 20 --max-time 150 --output $candidate $url
        if ($LASTEXITCODE -ne 0) { throw "Inno Setup download failed: $LASTEXITCODE" }
    }
    if ((Get-FileHash -LiteralPath $candidate -Algorithm SHA256).Hash -ne $hash) { throw 'Inno Setup checksum mismatch.' }
    if ($candidate -ne $installer) { Move-Item -LiteralPath $candidate -Destination $installer }
    $process = Start-Process -FilePath $installer -ArgumentList @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/CURRENTUSER', '/NOICONS', "/DIR=`"$destination`"") -WindowStyle Hidden -Wait -PassThru
    if ($process.ExitCode -ne 0) { throw "Inno Setup installation failed: $($process.ExitCode)" }
}
if (!(Test-Path -LiteralPath $compiler)) { throw 'Inno Setup compiler is missing.' }
$compiler
