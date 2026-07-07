# Signs release\Sticker Nah.exe with a self-signed code-signing certificate.
#
# Usage (from project root, after scripts\package-portable.ps1):
#     powershell -ExecutionPolicy Bypass -File scripts\sign.ps1
#
# WARNING: a self-signed certificate removes the Windows/SmartScreen "Unknown
# publisher" warning ONLY on machines where the certificate is trusted (see
# the Export block below). It does NOT remove the warning on other people's
# computers. For public release, a certificate from a CA (OV/EV) is needed.

$ErrorActionPreference = "Stop"
$exe = Join-Path $PSScriptRoot "..\release\Sticker Nah.exe"
if (-not (Test-Path $exe)) { throw "Not found: $exe. Build first: scripts\package-portable.ps1" }

# 1. Reuse the existing Sticker Nah certificate or create a new one (valid 5 years).
$subject = "CN=Alexander Kondratyev, O=Sticker Nah"
$cert = Get-ChildItem Cert:\CurrentUser\My |
    Where-Object { $_.Subject -eq $subject -and $_.HasPrivateKey } |
    Sort-Object NotAfter -Descending | Select-Object -First 1

if (-not $cert) {
    Write-Host "Creating self-signed code-signing certificate..."
    $cert = New-SelfSignedCertificate `
        -Type CodeSigningCert `
        -Subject $subject `
        -KeyAlgorithm RSA -KeyLength 3072 `
        -HashAlgorithm SHA256 `
        -CertStoreLocation Cert:\CurrentUser\My `
        -NotAfter (Get-Date).AddYears(5)
}
Write-Host "Certificate: $($cert.Thumbprint)"

# 2. Sign with a timestamp (signature stays valid after the certificate expires).
Set-AuthenticodeSignature -FilePath $exe -Certificate $cert `
    -HashAlgorithm SHA256 `
    -TimestampServer "http://timestamp.digicert.com" | Format-List

# 3. Verify.
Get-AuthenticodeSignature $exe | Format-List Status, StatusMessage, SignerCertificate

# --- Export, to trust the signature on OTHER machines you own ---
#
#   Public part only (.cer) -- for installing into trusted stores:
#       Export-Certificate -Cert $cert -FilePath release\StickerNah.cer
#
#   On the target machine (as admin):
#       Import-Certificate -FilePath StickerNah.cer -CertStoreLocation Cert:\LocalMachine\Root
#       Import-Certificate -FilePath StickerNah.cer -CertStoreLocation Cert:\LocalMachine\TrustedPublisher
#
#   Backup of the certificate WITH the private key (.pfx, keep secret!):
#       $pwd = Read-Host -AsSecureString "Password for .pfx"
#       Export-PfxCertificate -Cert $cert -FilePath StickerNah.pfx -Password $pwd
#
# certificateThumbprint in src-tauri/tauri.conf.json (bundle.windows) already
# points at this certificate -- `tauri build` (NSIS installer) signs automatically.
