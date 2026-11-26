function Test-PatinaPublisher($path, $subject) {
    $signature = Microsoft.PowerShell.Security\Get-AuthenticodeSignature -LiteralPath $path
    return ($signature.Status -eq 'Valid' -and $signature.TimeStamperCertificate -and
        [string]::Equals($signature.SignerCertificate.Subject, $subject, [StringComparison]::Ordinal))
}
