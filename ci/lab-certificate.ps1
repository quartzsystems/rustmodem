# Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
# licensed under the terms of the MIT license which can be found in the
# root directory of this project.

# Makes a lab certificate: a self-signed code-signing certificate named
# "RustModem Lab", in the current user's certificate store, where its
# private key stays. Writes the certificate, without the key, to a file,
# and prints the thumbprint that ci\build.ps1 signs with.
#
#   ci\lab-certificate.ps1 -Out C:\keys\rustmodem-lab.cer [-Years 2]
#
# Keep the file outside the repository. The private key never leaves the
# certificate store of the account that ran this. docs\SIGNING.md says
# what a machine that trusts this certificate is trusting.

param(
    [Parameter(Mandatory)]
    [string]$Out,
    [int]$Years = 2
)

$ErrorActionPreference = 'Stop'

# The name is the one the setup program finds the certificate by, to see
# whether it is trusted and to remove it.
$certificate = New-SelfSignedCertificate `
    -Type CodeSigningCert `
    -Subject 'CN=RustModem Lab' `
    -KeyUsage DigitalSignature `
    -KeyAlgorithm RSA -KeyLength 3072 `
    -HashAlgorithm SHA256 `
    -CertStoreLocation Cert:\CurrentUser\My `
    -NotAfter (Get-Date).AddYears($Years)

Export-Certificate -Cert $certificate -FilePath $Out | Out-Null
"certificate: $Out"
"thumbprint:  $($certificate.Thumbprint)"
