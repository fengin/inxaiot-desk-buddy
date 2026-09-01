param(
    [Parameter(Mandatory = $true)]
    [ValidateNotNullOrEmpty()]
    [string]$Path
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

function Find-SignTool {
    $command = Get-Command signtool.exe -ErrorAction SilentlyContinue
    if ($null -ne $command) {
        return $command.Source
    }
    $kits = "C:\Program Files (x86)\Windows Kits\10\bin"
    $candidate = Get-ChildItem $kits -Filter signtool.exe -File -Recurse -ErrorAction SilentlyContinue |
        Where-Object { $_.FullName -like "*\x64\signtool.exe" } |
        Sort-Object FullName -Descending |
        Select-Object -First 1
    if ($null -eq $candidate) {
        throw "未找到 Windows SDK signtool.exe"
    }
    return $candidate.FullName
}

function Find-SigningCertificate {
    param([string]$Thumbprint)

    foreach ($store in @("Cert:\CurrentUser\My", "Cert:\LocalMachine\My")) {
        $certificate = Get-ChildItem $store -ErrorAction SilentlyContinue |
            Where-Object { $_.Thumbprint -eq $Thumbprint } |
            Select-Object -First 1
        if ($null -ne $certificate) {
            return [PSCustomObject]@{
                Certificate = $certificate
                MachineStore = $store -like "Cert:\LocalMachine*"
            }
        }
    }
    throw "签名证书不存在：$Thumbprint"
}

$resolvedPath = [System.IO.Path]::GetFullPath($Path)
if (-not [System.IO.File]::Exists($resolvedPath)) {
    throw "待签名文件不存在：$resolvedPath"
}
$projectRoot = [System.IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
$targetRoot = [System.IO.Path]::GetFullPath((Join-Path $projectRoot "src-tauri\target"))
if (-not $resolvedPath.StartsWith($targetRoot + [System.IO.Path]::DirectorySeparatorChar, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "拒绝签名 target 目录之外的文件：$resolvedPath"
}

$rawThumbprint = $env:INX_SIGN_CERT_THUMBPRINT
if ($null -eq $rawThumbprint) {
    $rawThumbprint = ""
}
$thumbprint = (($rawThumbprint) -replace "\s", "").ToUpperInvariant()
if ($thumbprint -notmatch "^[0-9A-F]{40}$") {
    throw "INX_SIGN_CERT_THUMBPRINT 必须是40位SHA-1证书指纹"
}
$selection = Find-SigningCertificate -Thumbprint $thumbprint
$certificate = $selection.Certificate
if (-not $certificate.HasPrivateKey) {
    throw "签名证书没有可用私钥"
}
if ($certificate.NotBefore -gt (Get-Date) -or $certificate.NotAfter -le (Get-Date)) {
    throw "签名证书不在有效期内"
}
$codeSigningOid = "1.3.6.1.5.5.7.3.3"
if (-not ($certificate.EnhancedKeyUsageList.ObjectId.Value -contains $codeSigningOid)) {
    throw "证书缺少 Code Signing EKU"
}
$chain = [System.Security.Cryptography.X509Certificates.X509Chain]::new()
$chain.ChainPolicy.RevocationMode = [System.Security.Cryptography.X509Certificates.X509RevocationMode]::Online
$chain.ChainPolicy.RevocationFlag = [System.Security.Cryptography.X509Certificates.X509RevocationFlag]::ExcludeRoot
if (-not $chain.Build($certificate)) {
    $statuses = ($chain.ChainStatus | ForEach-Object { $_.Status.ToString() }) -join ","
    throw "签名证书信任链验证失败：$statuses"
}

$timestampUrl = $env:INX_SIGN_TIMESTAMP_URL
if ([string]::IsNullOrWhiteSpace($timestampUrl)) {
    $timestampUrl = "http://timestamp.digicert.com"
}
if ($timestampUrl -notmatch "^https?://") {
    throw "时间戳服务URL无效"
}
$signTool = Find-SignTool
$arguments = @("sign", "/sha1", $thumbprint, "/s", "My", "/fd", "SHA256", "/tr", $timestampUrl, "/td", "SHA256", "/v")
if ($selection.MachineStore) {
    $arguments += "/sm"
}
$arguments += $resolvedPath
& $signTool @arguments
if ($LASTEXITCODE -ne 0) {
    throw "signtool签名失败，退出码：$LASTEXITCODE"
}
& $signTool verify /pa /all /v $resolvedPath
if ($LASTEXITCODE -ne 0) {
    throw "signtool签名验证失败，退出码：$LASTEXITCODE"
}
$signature = Get-AuthenticodeSignature -FilePath $resolvedPath
if ($signature.Status -ne [System.Management.Automation.SignatureStatus]::Valid) {
    throw "Authenticode签名状态不是Valid：$($signature.Status)"
}
if ($signature.SignerCertificate.Thumbprint -ne $thumbprint) {
    throw "Authenticode签名证书与指定指纹不一致"
}
Write-Host "SIGNATURE_OK=$resolvedPath"
