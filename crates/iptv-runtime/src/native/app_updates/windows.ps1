# Run by the native update worker with private environment data and no profile.
# Keep the file locked from verification until CreateProcess has mapped it.
$ErrorActionPreference = 'Stop'
$path = $env:PATINA_STAGED_SETUP
$handle = $null
$hash = $null
$child = $null
try {
    $handle = [IO.File]::Open($path, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
    $hash = [Security.Cryptography.SHA256]::Create()
    $digest = [BitConverter]::ToString($hash.ComputeHash($handle)).Replace('-', '')
    if ($digest -ine $env:PATINA_SETUP_DIGEST) { exit 1 }
    $signature = Microsoft.PowerShell.Security\Get-AuthenticodeSignature -LiteralPath $path
    if ($signature.Status -ne 'Valid' -or !$signature.TimeStamperCertificate -or
        ![string]::Equals($signature.SignerCertificate.Subject, $env:PATINA_SETUP_SUBJECT,
            [StringComparison]::Ordinal)) { exit 1 }

    [Console]::Out.WriteLine('verified')
    [Console]::Out.Flush()
    if ([Console]::In.ReadLine() -cne 'install') { exit 1 }
    $start = New-Object Diagnostics.ProcessStartInfo
    $start.FileName = $path
    $start.Arguments = '/CURRENTUSER /NORESTART'
    $start.WorkingDirectory = [Environment]::SystemDirectory
    $start.UseShellExecute = $false
    $child = [Diagnostics.Process]::Start($start)
    if (!$child) { exit 1 }
    [Console]::Out.WriteLine('started')
    [Console]::Out.Flush()
    $handle.Dispose()
    $handle = $null
    $child.WaitForExit()
} finally {
    if ($handle) { $handle.Dispose() }
    if ($hash) { $hash.Dispose() }
    if ($child) { $child.Dispose() }
    if ($path) { Remove-Item -LiteralPath $path -Force -ErrorAction SilentlyContinue }
}
