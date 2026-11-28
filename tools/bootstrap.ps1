# Idempotent developer setup for Patina IPTV on Windows (x64).
# Checks Visual Studio C++ Build Tools and Windows SDK, Strawberry Perl,
# Rust + wasm32 target, the pinned sibling NSH checkout and nsh-cli,
# the SHA-256-verified Windows libmpv runtime, bundled Roboto fonts,
# and runs `cargo check --locked -p iptv-app`.
[CmdletBinding()]
param(
    [switch]$CheckOnly,
    [switch]$AllowMinimalToolchain,
    [switch]$SkipNsh,
    [switch]$SkipRuntime,
    [switch]$SkipSmoke
)
$ErrorActionPreference = 'Stop'
$savedPath = $env:PATH
$savedToolchain = $env:RUSTUP_TOOLCHAIN
$savedAutoInstall = $env:RUSTUP_AUTO_INSTALL
$savedNoBytecode = $env:PYTHONDONTWRITEBYTECODE
$savedGitLocks = $env:GIT_OPTIONAL_LOCKS
$callerPwd = (Get-Location).Path
$root = Split-Path $PSScriptRoot -Parent

function Trim-TrailingSeparators([string]$PathText) {
    $rootPart = [System.IO.Path]::GetPathRoot($PathText)
    if ($PathText -eq $rootPart) { return $PathText }
    return $PathText.TrimEnd('\', '/')
}

function Resolve-CallerPath([string]$RawPath) {
    $full = if ([System.IO.Path]::IsPathRooted($RawPath)) {
        [System.IO.Path]::GetFullPath($RawPath)
    } else {
        [System.IO.Path]::GetFullPath((Join-Path $callerPwd $RawPath))
    }
    return Trim-TrailingSeparators $full
}

function Invoke-NativeProbe([scriptblock]$Command) {
    # Windows PowerShell 5.1 (and PS 7 with $PSNativeCommandUseErrorActionPreference)
    # turns redirected native stderr into terminating ErrorRecords under 'Stop'.
    $prevEap = $ErrorActionPreference
    $hasNativePref = Test-Path Variable:\PSNativeCommandUseErrorActionPreference
    if ($hasNativePref) { $prevNative = $PSNativeCommandUseErrorActionPreference }
    $ErrorActionPreference = 'Continue'
    if ($hasNativePref) { $PSNativeCommandUseErrorActionPreference = $false }
    try {
        $output = & $Command 2>&1
        return [pscustomobject]@{
            ExitCode = $LASTEXITCODE
            Output   = (($output | ForEach-Object { "$_" }) -join "`n").Trim()
        }
    } finally {
        $ErrorActionPreference = $prevEap
        if ($hasNativePref) { $PSNativeCommandUseErrorActionPreference = $prevNative }
    }
}

function Prepend-UniquePath([string]$Dir) {
    if (!(Test-Path -LiteralPath $Dir)) { return }
    $parts = @($env:PATH -split ';' | Where-Object { $_ -and ($_ -ne $Dir) })
    $env:PATH = (@($Dir) + $parts) -join ';'
}

Push-Location $root
try {
    $env:PYTHONDONTWRITEBYTECODE = '1'
    $env:GIT_OPTIONAL_LOCKS = '0'
    Prepend-UniquePath "$env:USERPROFILE\.cargo\bin"
    if (Test-Path -LiteralPath 'C:\Strawberry\perl\bin\perl.exe') {
        Prepend-UniquePath 'C:\Strawberry\c\bin'
        Prepend-UniquePath 'C:\Strawberry\perl\bin'
    }
    Prepend-UniquePath 'C:\Program Files\NASM'

    $failures = 0
    function Record-Missing([string]$Message, [string]$Hint = '') {
        Write-Host "  [missing] $Message" -ForegroundColor Yellow
        if ($Hint) {
            Write-Host "            $Hint"
        }
        $script:failures++
    }

    Write-Host '==> Checking Windows prerequisites'
    if (!(Get-Command git -ErrorAction SilentlyContinue)) {
        Record-Missing 'git is not on PATH' 'Install Git for Windows (winget install Git.Git).'
    } else {
        Write-Host '  [ok] git found'
    }

    $pythonOk = $false
    $pythonCmd = Get-Command python -ErrorAction SilentlyContinue
    if (!$pythonCmd) {
        Record-Missing 'python (>= 3.11) is not on PATH' 'Install Python 3.11+ (winget install Python.Python.3.12).'
    } else {
        $pyProbe = Invoke-NativeProbe { python -B -c "import sys; raise SystemExit(0 if sys.version_info >= (3, 11) else 1)" }
        if ($pyProbe.ExitCode -ne 0) {
            $pyVer = (Invoke-NativeProbe { python --version }).Output
            Record-Missing 'python >= 3.11 is required' "Found $pyVer"
        } else {
            $pythonOk = $true
            Write-Host '  [ok] python >= 3.11 found'
        }
    }

    $vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
    if (!(Test-Path -LiteralPath $vswhere)) {
        Record-Missing 'Visual Studio Installer (vswhere.exe) not found' `
            'Install Visual Studio 2022 Build Tools with "Desktop development with C++" and a Windows 10/11 SDK.'
    } else {
        $vsProbe = Invoke-NativeProbe { & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath }
        if ($vsProbe.ExitCode -ne 0 -or !$vsProbe.Output) {
            Record-Missing 'Visual Studio C++ x64 build tools component is missing' `
                'Enable "Desktop development with C++" (Microsoft.VisualStudio.Component.VC.Tools.x86.x64) in Visual Studio Installer.'
        } else {
            Write-Host "  [ok] Visual Studio C++ Build Tools found ($($vsProbe.Output))"
        }
    }

    $kits = try {
        Get-ItemPropertyValue 'HKLM:\SOFTWARE\Microsoft\Windows Kits\Installed Roots' KitsRoot10 -ErrorAction Stop
    } catch {
        $null
    }
    $sdk = @()
    if ($kits -and (Test-Path -LiteralPath (Join-Path $kits 'Include'))) {
        $sdk = @(Get-ChildItem (Join-Path $kits 'Include') -Directory |
            Where-Object { Test-Path -LiteralPath (Join-Path $_.FullName 'um/Windows.h') } |
            Select-Object -ExpandProperty Name)
    }
    if (!$sdk.Count) {
        Record-Missing 'Windows 10/11 SDK headers (um/Windows.h) not found' `
            'Select a Windows 10 or Windows 11 SDK component in Visual Studio Installer.'
    } else {
        Write-Host "  [ok] Windows SDK found ($($sdk -join ', '))"
    }

    if (!(Get-Command perl -ErrorAction SilentlyContinue)) {
        Record-Missing 'Strawberry Perl is not on PATH (SQLCipher builds bundled OpenSSL)' `
            'Install Strawberry Perl (winget install StrawberryPerl.StrawberryPerl) and ensure perl\bin and c\bin are on PATH.'
    } else {
        Write-Host '  [ok] Perl found on PATH'
    }

    if (!$CheckOnly -and $failures -gt 0) {
        throw "Missing $failures Windows prerequisite(s) listed above."
    }

    $nshRev = ''
    $toolchain = ''
    if ($pythonOk) {
        $pinLines = & python -B "$PSScriptRoot/ci/pins.py"
        if ($LASTEXITCODE -ne 0) {
            throw 'Could not read nsh.rev or rust-toolchain.toml via tools/ci/pins.py.'
        }
        foreach ($line in $pinLines) {
            $parts = $line -split '=', 2
            if ($parts.Count -eq 2) {
                if ($parts[0] -eq 'nsh') { $nshRev = $parts[1] }
                elseif ($parts[0] -eq 'toolchain') { $toolchain = $parts[1] }
            }
        }
    } else {
        $nshRev = (Get-Content -Raw (Join-Path $root 'nsh.rev')).Trim()
        $tcText = Get-Content -Raw (Join-Path $root 'rust-toolchain.toml')
        if ($tcText -match '(?m)^\s*channel\s*=\s*"([^"]+)"') {
            $toolchain = $Matches[1]
        }
    }
    if (!$nshRev -or !$toolchain) {
        throw 'Could not determine nsh.rev or rust-toolchain.toml pin.'
    }

    Write-Host "==> Checking Rust toolchain ($toolchain)"
    function Invoke-RustupProbe([scriptblock]$Command) {
        $prevAuto = $env:RUSTUP_AUTO_INSTALL
        $env:RUSTUP_AUTO_INSTALL = '0'
        try {
            return Invoke-NativeProbe $Command
        } finally {
            $env:RUSTUP_AUTO_INSTALL = $prevAuto
        }
    }

    if (Get-Command rustup -ErrorAction SilentlyContinue) {
        $toolchainReady = $false
        $whichProbe = Invoke-RustupProbe { rustup which --toolchain $toolchain cargo }
        if ($whichProbe.ExitCode -eq 0) {
            $toolchainReady = $true
            Write-Host "  [ok] Rust $toolchain is installed"
        } elseif ($CheckOnly) {
            Record-Missing "Rust toolchain $toolchain is not installed" `
                "Run: rustup toolchain install $toolchain --component clippy,rustfmt"
        } else {
            Write-Host "  [install] Installing Rust $toolchain"
            & rustup toolchain install $toolchain --component clippy,rustfmt
            if ($LASTEXITCODE -ne 0) { throw "Failed to install Rust $toolchain" }
            $toolchainReady = $true
        }

        if ($toolchainReady) {
            $missingComponents = @()
            if ((Invoke-RustupProbe { rustup which --toolchain $toolchain cargo-clippy }).ExitCode -ne 0) {
                $missingComponents += 'clippy'
            }
            if ((Invoke-RustupProbe { rustup which --toolchain $toolchain rustfmt }).ExitCode -ne 0) {
                $missingComponents += 'rustfmt'
            }
            if ($missingComponents.Count -eq 0) {
                Write-Host "  [ok] Components clippy and rustfmt are installed for $toolchain"
            } elseif ($CheckOnly -and $AllowMinimalToolchain) {
                Write-Host "  [note] Optional minimal-toolchain components ($($missingComponents -join ', ')) are not installed"
            } elseif ($CheckOnly) {
                Record-Missing "Rust components ($($missingComponents -join ', ')) are missing for $toolchain" `
                    "Run: rustup component add --toolchain $toolchain $($missingComponents -join ' ')"
            } else {
                Write-Host "  [install] Adding Rust components ($($missingComponents -join ', ')) to $toolchain"
                & rustup component add --toolchain $toolchain @missingComponents
                if ($LASTEXITCODE -ne 0) { throw 'Failed to install clippy/rustfmt components' }
            }

            $targetProbe = Invoke-RustupProbe { rustup target list --installed --toolchain $toolchain }
            $targets = $targetProbe.Output -split '\r?\n'
            if ($targets -contains 'wasm32-unknown-unknown') {
                Write-Host "  [ok] Target wasm32-unknown-unknown is installed for $toolchain"
            } elseif ($CheckOnly -and $AllowMinimalToolchain) {
                Write-Host '  [note] Optional minimal-toolchain target wasm32-unknown-unknown is not installed'
            } elseif ($CheckOnly) {
                Record-Missing "Rust target wasm32-unknown-unknown is missing for $toolchain" `
                    "Run: rustup target add --toolchain $toolchain wasm32-unknown-unknown"
            } else {
                Write-Host "  [install] Adding target wasm32-unknown-unknown to $toolchain"
                & rustup target add --toolchain $toolchain wasm32-unknown-unknown
                if ($LASTEXITCODE -ne 0) { throw 'Failed to install wasm32-unknown-unknown target' }
            }
        }
    } elseif (Get-Command cargo -ErrorAction SilentlyContinue) {
        $rustcCmd = Get-Command rustc -ErrorAction SilentlyContinue
        $rustcProbe = if ($rustcCmd) { Invoke-NativeProbe { rustc --version } } else { $null }
        $rustcVer = if ($rustcProbe -and $rustcProbe.ExitCode -eq 0) { $rustcProbe.Output } else { 'rustc not found' }
        if ($rustcVer -notmatch "^rustc\s+$([regex]::Escape($toolchain))(?:\s|\(|$)") {
            Record-Missing "Standalone Rust compiler ($rustcVer) does not match pinned toolchain $toolchain" `
                "Install rustup (https://rustup.rs) or Rust $toolchain."
        } else {
            $missingComponents = @()
            if (!(Get-Command cargo-clippy -ErrorAction SilentlyContinue)) { $missingComponents += 'clippy' }
            if (!(Get-Command rustfmt -ErrorAction SilentlyContinue)) { $missingComponents += 'rustfmt' }
            if ($missingComponents.Count -gt 0) {
                if ($CheckOnly -and $AllowMinimalToolchain) {
                    Write-Host "  [note] Optional minimal-toolchain components ($($missingComponents -join ', ')) are not installed"
                } else {
                    Record-Missing "Rust components ($($missingComponents -join ', ')) are missing for standalone Rust $toolchain" `
                        "Install rustup or the $($missingComponents -join ', ') tools for Rust $toolchain."
                }
            }
            $wasmLib = Invoke-NativeProbe { rustc --print target-libdir --target wasm32-unknown-unknown }
            $wasmReady = $wasmLib.ExitCode -eq 0 -and $wasmLib.Output -and
                (Test-Path -LiteralPath $wasmLib.Output) -and
                (@(Get-ChildItem -LiteralPath $wasmLib.Output -Filter 'libcore*.rlib' -File -ErrorAction SilentlyContinue).Count -gt 0)
            if ($wasmReady) {
                Write-Host "  [ok] Standalone Rust $toolchain and wasm32-unknown-unknown target verified"
            } elseif ($CheckOnly -and $AllowMinimalToolchain) {
                Write-Host "  [ok] Standalone Rust $toolchain verified"
                Write-Host '  [note] Optional minimal-toolchain target wasm32-unknown-unknown is not installed'
            } else {
                Record-Missing "Rust target wasm32-unknown-unknown is missing for standalone Rust $toolchain" `
                    "Install rustup or add the wasm32-unknown-unknown target sysroot for Rust $toolchain."
            }
        }
        if (!$CheckOnly -and $failures -gt 0) {
            throw "Standalone Rust installation does not satisfy pinned toolchain $toolchain."
        }
    } else {
        Record-Missing 'Neither rustup nor cargo is on PATH' 'Install rustup (https://rustup.rs) with the x86_64-pc-windows-msvc host.'
        if (!$CheckOnly) { throw 'Rust toolchain is missing.' }
    }

    $defaultNshDir = Trim-TrailingSeparators ([System.IO.Path]::GetFullPath((Join-Path (Split-Path $root -Parent) 'nsh-patina')))
    $nshDir = if ($env:PATINA_NSH_DIR) { Resolve-CallerPath $env:PATINA_NSH_DIR } else { $defaultNshDir }
    if (!$CheckOnly -and !$SkipSmoke -and !$SkipNsh -and ($nshDir -ne $defaultNshDir)) {
        throw "PATINA_NSH_DIR ($nshDir) differs from ..\nsh-patina ($defaultNshDir), which Cargo.toml resolves; pass -SkipSmoke when testing an isolated NSH directory."
    }
    $nshTargetDir = Join-Path $nshDir 'target'
    $nshExe = Join-Path $nshTargetDir 'release/nsh.exe'
    $nshStamp = Join-Path $nshTargetDir 'release/.patina-nsh-cli.rev'

    if ($SkipNsh) {
        Write-Host '==> Skipping NSH checkout and nsh-cli (-SkipNsh)'
    } else {
        Write-Host "==> Checking pinned NSH checkout ($nshDir @ $($nshRev.Substring(0, 12)))"
        if ($CheckOnly) {
            $nshDotGit = Join-Path $nshDir '.git'
            if (!(Test-Path -LiteralPath $nshDir -PathType Container) -or !(Test-Path -LiteralPath $nshDotGit)) {
                Record-Missing "NSH checkout is missing at $nshDir" 'Run: ./tools/bootstrap.ps1'
            } elseif ($pythonOk) {
                $pinCheck = Invoke-NativeProbe { python -B "$PSScriptRoot/ci/pins.py" check-nsh $nshDir }
                if ($pinCheck.ExitCode -ne 0) {
                    Record-Missing 'NSH checkout is drifted or dirty' $pinCheck.Output
                } else {
                    Write-Host '  [ok] NSH checkout matches nsh.rev'
                }
            }
            $desiredStamp = "${nshRev}:${toolchain}"
            $existingStamp = if (Test-Path -LiteralPath $nshStamp) { (Get-Content -Raw -LiteralPath $nshStamp).Trim() } else { '' }
            if (!(Test-Path -LiteralPath $nshExe)) {
                Record-Missing "nsh-cli binary is not built at $nshExe" `
                    "Run: cargo build --release --locked --manifest-path `"$nshDir/Cargo.toml`" --target-dir `"$nshTargetDir`" -p nsh-cli"
            } elseif ($env:IPTV_ALLOW_NSH_DRIFT -ne '1' -and $existingStamp -ne $desiredStamp) {
                Record-Missing "nsh-cli binary at $nshExe is missing a clean stamp for $($nshRev.Substring(0, 12)) ($toolchain)" `
                    'Re-run ./tools/bootstrap.ps1 to verify and stamp nsh-cli.'
            } else {
                Write-Host "  [ok] nsh-cli binary found at $nshExe"
            }
        } else {
            $item = Get-Item -LiteralPath $nshDir -Force -ErrorAction SilentlyContinue
            if ($null -eq $item) {
                Write-Host "  [clone] Cloning teng-lin/nsh (branch patina) into $nshDir"
                & git clone --branch patina https://github.com/teng-lin/nsh.git $nshDir
                if ($LASTEXITCODE -ne 0) { throw 'Failed to clone teng-lin/nsh' }
                & git -C $nshDir checkout --detach $nshRev
                if ($LASTEXITCODE -ne 0) { throw "Failed to check out $nshRev" }
            } elseif (!(Test-Path -LiteralPath $nshDir -PathType Container) -or !(Test-Path -LiteralPath (Join-Path $nshDir '.git'))) {
                throw "$nshDir exists and is not a Git repository."
            } else {
                $statusProbe = Invoke-NativeProbe { git -C $nshDir status --porcelain }
                if ($statusProbe.ExitCode -ne 0) { throw "git status failed in $nshDir" }
                $revProbe = Invoke-NativeProbe { git -C $nshDir rev-parse HEAD }
                if ($revProbe.ExitCode -ne 0) { throw "git rev-parse failed in $nshDir" }
                $currentRev = $revProbe.Output
                if ($statusProbe.Output) {
                    if ($env:IPTV_ALLOW_NSH_DRIFT -eq '1') {
                        Write-Warning "IPTV_ALLOW_NSH_DRIFT=1: leaving dirty NSH checkout untouched at $currentRev"
                    } else {
                        & python -B "$PSScriptRoot/ci/pins.py" check-nsh $nshDir
                        throw 'Refusing to modify dirty NSH checkout.'
                    }
                } elseif ($currentRev -ne $nshRev) {
                    if ($env:IPTV_ALLOW_NSH_DRIFT -eq '1') {
                        Write-Warning "IPTV_ALLOW_NSH_DRIFT=1: leaving NSH checkout at $currentRev (nsh.rev pins $nshRev)"
                    } else {
                        Write-Host "  [update] Checking out pinned NSH commit $nshRev"
                        $catProbe = Invoke-NativeProbe { git -C $nshDir cat-file -e "$nshRev^{commit}" }
                        if ($catProbe.ExitCode -ne 0) {
                            & git -C $nshDir fetch origin $nshRev
                            if ($LASTEXITCODE -ne 0) { throw "Failed to fetch $nshRev in $nshDir" }
                        }
                        & git -C $nshDir checkout --detach $nshRev
                        if ($LASTEXITCODE -ne 0) { throw "Failed to check out $nshRev in $nshDir" }
                    }
                } else {
                    Write-Host "  [ok] NSH checkout already clean at $nshRev"
                }
            }

            & python -B "$PSScriptRoot/ci/pins.py" check-nsh $nshDir
            if ($LASTEXITCODE -ne 0) { throw 'The NSH checkout does not satisfy tools/ci/pins.py.' }

            $actualRev = (Invoke-NativeProbe { git -C $nshDir rev-parse HEAD }).Output
            $desiredStamp = "${actualRev}:${toolchain}"
            $existingStamp = if (Test-Path -LiteralPath $nshStamp) { (Get-Content -Raw -LiteralPath $nshStamp).Trim() } else { '' }
            if ($env:IPTV_ALLOW_NSH_DRIFT -ne '1' -and (Test-Path -LiteralPath $nshExe) -and $existingStamp -eq $desiredStamp) {
                Write-Host "  [ok] nsh-cli already built for $($actualRev.Substring(0, 12))"
            } else {
                if ($env:IPTV_ALLOW_NSH_DRIFT -eq '1' -and (Test-Path -LiteralPath $nshStamp)) {
                    Remove-Item -LiteralPath $nshStamp -Force
                }
                Write-Host '  [build] Building nsh-cli (--release --locked)'
                $env:RUSTUP_TOOLCHAIN = $toolchain
                & cargo build --release --locked --manifest-path "$nshDir/Cargo.toml" --target-dir $nshTargetDir -p nsh-cli
                if ($LASTEXITCODE -ne 0) { throw 'nsh-cli build failed' }
                if (!(Test-Path -LiteralPath $nshExe)) {
                    throw "Expected nsh-cli binary at $nshExe after build."
                }
                if ($env:IPTV_ALLOW_NSH_DRIFT -ne '1') {
                    Set-Content -LiteralPath $nshStamp -Value $desiredStamp -NoNewline
                }
                Write-Host "  [ok] Built $nshExe"
            }
        }
    }

    $mpvDir = Join-Path $root 'target/mpv'
    $mpvDll = Join-Path $mpvDir 'libmpv-2.dll'
    if ($SkipRuntime) {
        Write-Host '==> Skipping guarded Windows libmpv runtime (-SkipRuntime)'
    } else {
        Write-Host "==> Checking guarded Windows libmpv runtime ($mpvDir)"
        $mpvItem = Get-Item -LiteralPath $mpvDir -Force -ErrorAction SilentlyContinue
        if ($null -ne $mpvItem -and ($mpvItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint)) {
            if ($CheckOnly) {
                Record-Missing "Refusing junction or symlinked runtime destination: $mpvDir" `
                    'Use a normal directory at target\mpv so extraction cannot overwrite a shared folder.'
            } else {
                throw "Refusing junction or symlinked runtime destination: $mpvDir"
            }
        } else {
            $pin = Get-Content -Raw (Join-Path $PSScriptRoot 'mpv-runtime/runtime.json') | ConvertFrom-Json
            if ($pin.schema -ne 1 -or $pin.sha256 -notmatch '^[a-f0-9]{64}$' -or $pin.dll_sha256 -notmatch '^[a-f0-9]{64}$') {
                throw 'Invalid pinned libmpv runtime manifest'
            }
            $dllValid = (Test-Path -LiteralPath $mpvDll) -and
                ((Get-FileHash -LiteralPath $mpvDll -Algorithm SHA256).Hash.ToLowerInvariant() -eq $pin.dll_sha256)
            if ($dllValid) {
                Write-Host "  [ok] Verified SHA-256 of libmpv-2.dll in $mpvDir"
            } elseif ($CheckOnly) {
                Record-Missing "Guarded libmpv runtime is missing or outdated in $mpvDir" 'Run: ./tools/fetch-mpv-windows.ps1'
            } else {
                Write-Host '  [fetch] Fetching and verifying Windows libmpv runtime'
                & "$PSScriptRoot/fetch-mpv-windows.ps1"
            }
        }
    }

    Write-Host '==> Checking bundled Roboto fonts'
    $fontDir = Join-Path $root 'crates/iptv-app/fonts'
    $missingFonts = @()
    foreach ($font in @('Roboto-Regular.woff2', 'Roboto-Medium.woff2', 'RobotoCondensed-Regular.woff2', 'OFL.txt')) {
        if (!(Test-Path -LiteralPath (Join-Path $fontDir $font))) {
            $missingFonts += $font
        }
    }
    if ($missingFonts.Count -eq 0) {
        Write-Host '  [ok] Bundled Roboto fonts are present'
    } else {
        Record-Missing "Missing bundled font files: $($missingFonts -join ', ')" `
            'Restore crates/iptv-app/fonts from Git (or run bash tools/fetch-fonts.sh).'
        if (!$CheckOnly) { throw 'Bundled fonts are missing.' }
    }

    if ($CheckOnly) {
        if ($failures -gt 0) {
            throw "Bootstrap check failed: $failures prerequisite issue(s) found."
        }
        Write-Host 'Bootstrap check passed: all checked prerequisites are ready.'
        return
    }

    if (!$SkipSmoke) {
        if (!$SkipNsh -and ([System.IO.Path]::GetFullPath($nshDir) -ne $defaultNshDir)) {
            throw "PATINA_NSH_DIR ($nshDir) differs from ..\nsh-patina ($defaultNshDir), which Cargo.toml resolves; pass -SkipSmoke when testing an isolated NSH directory."
        }
        Write-Host '==> Running Cargo smoke check'
        if (!$SkipRuntime) {
            Prepend-UniquePath $mpvDir
        }
        $env:RUSTUP_TOOLCHAIN = $toolchain
        if (!$SkipNsh -and !$SkipRuntime) {
            & cargo check --locked -p iptv-app
            if ($LASTEXITCODE -ne 0) { throw 'cargo check --locked -p iptv-app failed' }
            Write-Host '  [ok] cargo check --locked -p iptv-app passed'
        } else {
            Write-Host '  [note] Skipping cargo check -p iptv-app because -SkipNsh or -SkipRuntime was set'
        }
    }

    Write-Host ''
    Write-Host 'Bootstrap complete.'
    Write-Host 'Build and run with ./tools/build-windows.ps1 -Run (or add target\mpv to PATH for direct cargo commands).'
} finally {
    $env:PATH = $savedPath
    $env:RUSTUP_TOOLCHAIN = $savedToolchain
    $env:RUSTUP_AUTO_INSTALL = $savedAutoInstall
    $env:PYTHONDONTWRITEBYTECODE = $savedNoBytecode
    $env:GIT_OPTIONAL_LOCKS = $savedGitLocks
    Pop-Location
}
