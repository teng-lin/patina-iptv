# Run by the native update worker with private environment data and no profile.
$ErrorActionPreference = 'Stop'
$path = $env:PATINA_STAGED_SETUP
$handle = $null
$hash = $null
$child = $null
$viewer = $null
try {
    $viewer = [Diagnostics.Process]::GetProcessById([int]$env:PATINA_UPDATE_PARENT)
    # Acquire the process handle now so a recycled PID cannot redirect the wait.
    $null = $viewer.Handle
    $handle = [IO.File]::Open($path, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
    $hash = [Security.Cryptography.SHA256]::Create()
    $digest = [BitConverter]::ToString($hash.ComputeHash($handle)).Replace('-', '')
    if ($digest -ine $env:PATINA_SETUP_DIGEST) { [Console]::WriteLine('digest'); exit 1 }
    $signature = Microsoft.PowerShell.Security\Get-AuthenticodeSignature -LiteralPath $path
    if ($signature.Status -ne 'Valid' -or !$signature.TimeStamperCertificate -or
        ![string]::Equals($signature.SignerCertificate.Subject, $env:PATINA_SETUP_SUBJECT,
            [StringComparison]::Ordinal)) { [Console]::WriteLine('signature'); exit 1 }

    [Console]::Out.WriteLine('verified')
    [Console]::Out.Flush()
    if ([Console]::In.ReadLine() -cne 'install') { exit 1 }
    if (!$viewer.WaitForExit(45000)) { exit 1 }

    $start = New-Object Diagnostics.ProcessStartInfo
    $start.FileName = $path
    $start.Arguments = '/CURRENTUSER /NORESTART /NOCLOSEAPPLICATIONS'
    $start.WorkingDirectory = [Environment]::SystemDirectory
    $start.UseShellExecute = $false
    foreach ($key in @('PATINA_STAGED_SETUP', 'PATINA_SETUP_DIGEST', 'PATINA_SETUP_SUBJECT', 'PATINA_UPDATE_PARENT', 'PSModulePath')) {
        $start.EnvironmentVariables.Remove($key)
    }
    $child = [Diagnostics.Process]::Start($start)
    if (!$child) { exit 1 }
    $child.WaitForExit()
} finally {
    if ($child) { $child.Dispose() }
    if ($viewer) { $viewer.Dispose() }
    if ($handle) { $handle.Dispose() }
    if ($hash) { $hash.Dispose() }
    if ($path) { Remove-Item -LiteralPath $path -Force -ErrorAction SilentlyContinue }
}
