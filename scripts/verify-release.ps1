param(
    [Parameter(Mandatory = $true)]
    [ValidateNotNullOrEmpty()]
    [string]$ArtifactSet
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

# 兼容历史入口：统一复验内部无签名产物集。
& (Join-Path $PSScriptRoot "verify-internal-release.ps1") -ArtifactSet $ArtifactSet
return

$root = [System.IO.Path]::GetFullPath($ArtifactSet)
if (-not [System.IO.Directory]::Exists($root)) {
    throw "产物集目录不存在：$root"
}
$manifestPath = Join-Path $root "release-manifest.json"
$signaturePath = Join-Path $root "release-manifest.p7s"
if (-not (Test-Path -LiteralPath $manifestPath) -or -not (Test-Path -LiteralPath $signaturePath)) {
    throw "产物集缺少清单或CMS签名"
}
$manifestBytes = [System.IO.File]::ReadAllBytes($manifestPath)
$manifest = [System.Text.Encoding]::UTF8.GetString($manifestBytes) | ConvertFrom-Json
Add-Type -AssemblyName System.Security.Cryptography.Pkcs
$contentInfo = [System.Security.Cryptography.Pkcs.ContentInfo]::new($manifestBytes)
$cms = [System.Security.Cryptography.Pkcs.SignedCms]::new($contentInfo, $true)
$cms.Decode([System.IO.File]::ReadAllBytes($signaturePath))
$cms.CheckSignature($true)
if ($cms.SignerInfos.Count -ne 1) {
    throw "CMS签名者数量不是1"
}
$signer = $cms.SignerInfos[0].Certificate
if ($signer.Thumbprint -ne $manifest.signing.thumbprint) {
    throw "CMS签名证书与清单不一致"
}
$chain = [System.Security.Cryptography.X509Certificates.X509Chain]::new()
$chain.ChainPolicy.RevocationMode = [System.Security.Cryptography.X509Certificates.X509RevocationMode]::Online
$chain.ChainPolicy.RevocationFlag = [System.Security.Cryptography.X509Certificates.X509RevocationFlag]::ExcludeRoot
if (-not $chain.Build($signer)) {
    $statuses = ($chain.ChainStatus | ForEach-Object { $_.Status.ToString() }) -join ","
    throw "产物签名证书信任链失败：$statuses"
}

foreach ($entry in $manifest.files) {
    $filePath = Join-Path $root ([string]$entry.name)
    if (-not [System.IO.File]::Exists($filePath)) {
        throw "清单文件不存在：$($entry.name)"
    }
    $file = Get-Item -LiteralPath $filePath
    if ($file.Length -ne [long]$entry.size) {
        throw "文件大小不一致：$($entry.name)"
    }
    $hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $filePath).Hash.ToLowerInvariant()
    if ($hash -ne [string]$entry.sha256) {
        throw "文件哈希不一致：$($entry.name)"
    }
    if ($entry.role -in @("portable", "installer")) {
        $authenticode = Get-AuthenticodeSignature -FilePath $filePath
        if ($authenticode.Status -ne [System.Management.Automation.SignatureStatus]::Valid) {
            throw "Authenticode签名无效：$($entry.name)，状态：$($authenticode.Status)"
        }
        if ($authenticode.SignerCertificate.Thumbprint -ne $manifest.signing.thumbprint) {
            throw "Authenticode证书与清单不一致：$($entry.name)"
        }
    }
}
Write-Host "RELEASE_VERIFIED=$root"
Write-Host "RELEASE_COMMIT=$($manifest.git.commit)"
Write-Host "RELEASE_VERSION=$($manifest.version)"
Write-Host "SIGNING_CERTIFICATE=$($manifest.signing.thumbprint)"
