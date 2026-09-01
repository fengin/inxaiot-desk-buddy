param(
    [string]$ArtifactRoot = "",
    [switch]$SkipQualityGate
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

function Invoke-NativeStep {
    param(
        [string]$Name,
        [scriptblock]$Command
    )
    Write-Host "==> $Name"
    & $Command
    if ($LASTEXITCODE -ne 0) {
        throw "$Name 失败，退出码：$LASTEXITCODE"
    }
}

function Invoke-NativeText {
    param(
        [string]$Name,
        [scriptblock]$Command
    )
    $output = & $Command | Out-String
    if ($LASTEXITCODE -ne 0) {
        throw "$Name 失败，退出码：$LASTEXITCODE"
    }
    return $output.Trim()
}

function Find-SigningCertificate {
    param([string]$Thumbprint)
    foreach ($store in @("Cert:\CurrentUser\My", "Cert:\LocalMachine\My")) {
        $certificate = Get-ChildItem $store -ErrorAction SilentlyContinue |
            Where-Object { $_.Thumbprint -eq $Thumbprint } |
            Select-Object -First 1
        if ($null -ne $certificate) {
            return $certificate
        }
    }
    throw "签名证书不存在：$Thumbprint"
}

function Assert-SignedFile {
    param(
        [string]$File,
        [string]$Thumbprint
    )
    $signature = Get-AuthenticodeSignature -FilePath $File
    if ($signature.Status -ne [System.Management.Automation.SignatureStatus]::Valid) {
        throw "Authenticode签名无效：$File，状态：$($signature.Status)"
    }
    if ($signature.SignerCertificate.Thumbprint -ne $Thumbprint) {
        throw "签名证书不一致：$File"
    }
}

function New-DetachedCmsSignature {
    param(
        [string]$ContentPath,
        [string]$SignaturePath,
        [System.Security.Cryptography.X509Certificates.X509Certificate2]$Certificate
    )
    Add-Type -AssemblyName System.Security.Cryptography.Pkcs
    $content = [System.IO.File]::ReadAllBytes($ContentPath)
    $contentInfo = [System.Security.Cryptography.Pkcs.ContentInfo]::new($content)
    $signedCms = [System.Security.Cryptography.Pkcs.SignedCms]::new($contentInfo, $true)
    $signer = [System.Security.Cryptography.Pkcs.CmsSigner]::new($Certificate)
    $signer.IncludeOption = [System.Security.Cryptography.X509Certificates.X509IncludeOption]::EndCertOnly
    $signedCms.ComputeSignature($signer, $true)
    [System.IO.File]::WriteAllBytes($SignaturePath, $signedCms.Encode())

    $verification = [System.Security.Cryptography.Pkcs.SignedCms]::new($contentInfo, $true)
    $verification.Decode([System.IO.File]::ReadAllBytes($SignaturePath))
    $verification.CheckSignature($true)
}

$projectRoot = [System.IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
if ([string]::IsNullOrWhiteSpace($ArtifactRoot)) {
    $ArtifactRoot = $env:INX_RELEASE_ARTIFACT_ROOT
}
if ([string]::IsNullOrWhiteSpace($ArtifactRoot)) {
    $ArtifactRoot = "D:\inxaiot-release-artifacts"
}
$artifactRootFull = [System.IO.Path]::GetFullPath($ArtifactRoot)
if ($artifactRootFull.StartsWith($projectRoot + [System.IO.Path]::DirectorySeparatorChar, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "正式产物根目录必须位于源码仓库之外"
}
if (-not [System.IO.Directory]::Exists($artifactRootFull)) {
    throw "正式产物根目录必须由管理员预先创建并配置ACL：$artifactRootFull"
}
$artifactRootItem = Get-Item -LiteralPath $artifactRootFull
if (($artifactRootItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
    throw "正式产物根目录不能是联接或符号链接"
}
$broadWriteSids = @("S-1-1-0", "S-1-5-11", "S-1-5-32-545")
$writeRights = [System.Security.AccessControl.FileSystemRights]::Write -bor
    [System.Security.AccessControl.FileSystemRights]::Modify -bor
    [System.Security.AccessControl.FileSystemRights]::FullControl
$unsafeRules = @((Get-Acl -LiteralPath $artifactRootFull).Access | Where-Object {
    if ($_.AccessControlType -ne [System.Security.AccessControl.AccessControlType]::Allow) {
        return $false
    }
    try {
        $sid = $_.IdentityReference.Translate([System.Security.Principal.SecurityIdentifier]).Value
    }
    catch {
        return $true
    }
    return $sid -in $broadWriteSids -and ($_.FileSystemRights -band $writeRights) -ne 0
})
if ($unsafeRules.Count -gt 0) {
    throw "正式产物根目录向Everyone、Authenticated Users或Users开放写权限"
}

Push-Location $projectRoot
$staging = $null
$generatedTauriConfig = $null
try {
    $status = Invoke-NativeText "读取Git状态" { git status --porcelain=v1 --untracked-files=all }
    if (-not [string]::IsNullOrWhiteSpace($status)) {
        throw "发布要求Git工作区完全干净"
    }
    $commit = (Invoke-NativeText "读取Git提交" { git rev-parse HEAD }).ToLowerInvariant()
    if ($commit -notmatch "^[0-9a-f]{40}$") {
        throw "Git提交ID无效"
    }
    $tree = (Invoke-NativeText "读取Git Tree" { git rev-parse 'HEAD^{tree}' }).ToLowerInvariant()
    $branch = Invoke-NativeText "读取Git分支" { git branch --show-current }
    $shortCommit = $commit.Substring(0, 12)
    $tauriConfig = [System.IO.File]::ReadAllText((Join-Path $projectRoot "src-tauri\tauri.conf.json")) | ConvertFrom-Json
    $version = [string]$tauriConfig.version
    if ($version -notmatch "^\d+\.\d+\.\d+([-.][0-9A-Za-z.-]+)?$") {
        throw "Tauri版本号无效：$version"
    }

    $rawThumbprint = $env:INX_SIGN_CERT_THUMBPRINT
    if ($null -eq $rawThumbprint) {
        $rawThumbprint = ""
    }
    $thumbprint = (($rawThumbprint) -replace "\s", "").ToUpperInvariant()
    if ($thumbprint -notmatch "^[0-9A-F]{40}$") {
        throw "INX_SIGN_CERT_THUMBPRINT 必须是40位SHA-1证书指纹"
    }
    $certificate = Find-SigningCertificate -Thumbprint $thumbprint
    if (-not $certificate.HasPrivateKey) {
        throw "签名证书没有可用私钥"
    }

    $artifactSetName = "inxaiot-desk-buddy-$version-$shortCommit"
    $finalDirectory = Join-Path $artifactRootFull $artifactSetName
    if (Test-Path -LiteralPath $finalDirectory) {
        throw "同版本同提交产物已经存在，禁止覆盖：$finalDirectory"
    }
    $staging = Join-Path $artifactRootFull (".staging-$artifactSetName-" + [Guid]::NewGuid().ToString("N"))
    $null = New-Item -ItemType Directory -Path $staging

    if (-not $SkipQualityGate) {
        & (Join-Path $PSScriptRoot "quality-gate.ps1")
        if ($LASTEXITCODE -ne 0) {
            throw "完整质量门禁失败"
        }
    }

    Invoke-NativeStep "生成CycloneDX SBOM" {
        & (Join-Path $PSScriptRoot "generate-sbom.ps1") -OutputPath (Join-Path $staging "sbom.cdx.json") -Commit $commit
    }
    $releaseConfigPath = Join-Path $projectRoot "src-tauri\tauri.release.conf.json"
    $releaseConfig = [System.IO.File]::ReadAllText($releaseConfigPath) | ConvertFrom-Json
    $signArguments = @($releaseConfig.bundle.windows.signCommand.args)
    $relativeSignScriptIndex = [Array]::IndexOf($signArguments, "scripts/sign-windows.ps1")
    if ($relativeSignScriptIndex -lt 0) {
        throw "Tauri发布配置缺少受控签名脚本占位路径"
    }
    $releaseConfig.bundle.windows.signCommand.args[$relativeSignScriptIndex] = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot "sign-windows.ps1"))
    $generatedConfigRoot = Join-Path $projectRoot "src-tauri\target\release-evidence"
    $null = New-Item -ItemType Directory -Path $generatedConfigRoot -Force
    $generatedTauriConfig = Join-Path $generatedConfigRoot ("tauri.release." + [Guid]::NewGuid().ToString("N") + ".json")
    [System.IO.File]::WriteAllText($generatedTauriConfig, ($releaseConfig | ConvertTo-Json -Depth 30), [System.Text.UTF8Encoding]::new($false))
    $buildStarted = (Get-Date).ToUniversalTime().AddMinutes(-1)
    Invoke-NativeStep "构建并签名NSIS安装包" {
        pnpm tauri build --bundles nsis --config $generatedTauriConfig
    }

    $portableSource = Join-Path $projectRoot "src-tauri\target\release\inxaiot-desk-buddy.exe"
    $installerSource = Get-ChildItem (Join-Path $projectRoot "src-tauri\target\release\bundle\nsis") -Filter "*.exe" -File |
        Where-Object { $_.LastWriteTimeUtc -ge $buildStarted } |
        Sort-Object LastWriteTimeUtc -Descending |
        Select-Object -First 1
    if (-not (Test-Path -LiteralPath $portableSource) -or $null -eq $installerSource) {
        throw "Tauri没有生成预期的裸程序或NSIS安装包"
    }
    Assert-SignedFile -File $portableSource -Thumbprint $thumbprint
    Assert-SignedFile -File $installerSource.FullName -Thumbprint $thumbprint

    $portableTarget = Join-Path $staging "inxaiot-desk-buddy-$version-x64.exe"
    $installerTarget = Join-Path $staging "inxaiot-desk-buddy-$version-x64-setup.exe"
    Copy-Item -LiteralPath $portableSource -Destination $portableTarget
    Copy-Item -LiteralPath $installerSource.FullName -Destination $installerTarget
    Export-Certificate -Cert $certificate -FilePath (Join-Path $staging "signing-certificate.cer") -Force | Out-Null

    $roles = [ordered]@{
        (Split-Path -Leaf $portableTarget) = "portable"
        (Split-Path -Leaf $installerTarget) = "installer"
        "sbom.cdx.json" = "sbom"
        "signing-certificate.cer" = "public-signing-certificate"
    }
    $files = @()
    foreach ($name in $roles.Keys) {
        $file = Get-Item -LiteralPath (Join-Path $staging $name)
        $files += [ordered]@{
            name = $name
            role = $roles[$name]
            size = $file.Length
            sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $file.FullName).Hash.ToLowerInvariant()
        }
    }
    $timestampUrl = $env:INX_SIGN_TIMESTAMP_URL
    if ([string]::IsNullOrWhiteSpace($timestampUrl)) {
        $timestampUrl = "http://timestamp.digicert.com"
    }
    $manifest = [ordered]@{
        schemaVersion = 1
        product = "INX 实施工作台"
        identifier = "com.inxaiot.desk-buddy"
        version = $version
        git = [ordered]@{ commit = $commit; tree = $tree; branch = $branch; dirty = $false }
        build = [ordered]@{
            timeUtc = (Get-Date).ToUniversalTime().ToString("o")
            ciProvider = if ($env:JENKINS_URL) { "Jenkins" } else { "local-trusted-runner" }
            ciBuildUrl = [string]$env:BUILD_URL
            runner = [System.Security.Principal.WindowsIdentity]::GetCurrent().Name
        }
        signing = [ordered]@{
            subject = $certificate.Subject
            thumbprint = $thumbprint
            notBeforeUtc = $certificate.NotBefore.ToUniversalTime().ToString("o")
            notAfterUtc = $certificate.NotAfter.ToUniversalTime().ToString("o")
            timestampAuthority = $timestampUrl
            digest = "SHA-256"
        }
        rollback = [ordered]@{
            strategy = "verify-and-run-prior-immutable-installer"
            allowSignedDowngrade = $true
            artifactRoot = $artifactRootFull
        }
        files = $files
    }
    $manifestPath = Join-Path $staging "release-manifest.json"
    [System.IO.File]::WriteAllText($manifestPath, ($manifest | ConvertTo-Json -Depth 20), [System.Text.UTF8Encoding]::new($false))
    $signaturePath = Join-Path $staging "release-manifest.p7s"
    New-DetachedCmsSignature -ContentPath $manifestPath -SignaturePath $signaturePath -Certificate $certificate

    $checksumFiles = Get-ChildItem $staging -File | Sort-Object Name
    $checksumLines = foreach ($file in $checksumFiles) {
        $hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $file.FullName).Hash.ToLowerInvariant()
        "$hash  $($file.Name)"
    }
    [System.IO.File]::WriteAllLines((Join-Path $staging "checksums.sha256"), $checksumLines, [System.Text.UTF8Encoding]::new($false))
    foreach ($file in Get-ChildItem $staging -File) {
        $file.IsReadOnly = $true
    }
    Move-Item -LiteralPath $staging -Destination $finalDirectory
    $staging = $null
    Write-Host "ARTIFACT_SET=$finalDirectory"
    Write-Host "RELEASE_COMMIT=$commit"
    Write-Host "SIGNING_CERTIFICATE=$thumbprint"
}
finally {
    if ($null -ne $generatedTauriConfig -and (Test-Path -LiteralPath $generatedTauriConfig)) {
        $generatedConfigFull = [System.IO.Path]::GetFullPath($generatedTauriConfig)
        $allowedConfigRoot = [System.IO.Path]::GetFullPath((Join-Path $projectRoot "src-tauri\target\release-evidence"))
        if ($generatedConfigFull.StartsWith($allowedConfigRoot + [System.IO.Path]::DirectorySeparatorChar, [System.StringComparison]::OrdinalIgnoreCase) -and
            (Split-Path -Leaf $generatedConfigFull).StartsWith("tauri.release.")) {
            Remove-Item -LiteralPath $generatedConfigFull -Force
        }
    }
    if ($null -ne $staging -and (Test-Path -LiteralPath $staging)) {
        $stagingFull = [System.IO.Path]::GetFullPath($staging)
        if ($stagingFull.StartsWith($artifactRootFull + [System.IO.Path]::DirectorySeparatorChar, [System.StringComparison]::OrdinalIgnoreCase) -and
            (Split-Path -Leaf $stagingFull).StartsWith(".staging-inxaiot-desk-buddy-")) {
            Remove-Item -LiteralPath $stagingFull -Recurse -Force
        }
    }
    Pop-Location
}
