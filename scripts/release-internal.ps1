param(
    [string]$ArtifactRoot = "",
    [switch]$SkipQualityGate
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

function Invoke-NativeStep {
    param([string]$Name, [scriptblock]$Command)
    Write-Host "==> $Name"
    & $Command
    if ($LASTEXITCODE -ne 0) {
        throw "$Name 失败，退出码：$LASTEXITCODE"
    }
}

function Invoke-NativeText {
    param([string]$Name, [scriptblock]$Command)
    $output = & $Command | Out-String
    if ($LASTEXITCODE -ne 0) {
        throw "$Name 失败，退出码：$LASTEXITCODE"
    }
    return $output.Trim()
}

function Assert-UnsignedFile {
    param([string]$File)
    $signature = Get-AuthenticodeSignature -FilePath $File
    if ($signature.Status -ne [System.Management.Automation.SignatureStatus]::NotSigned) {
        throw "内部无签名策略要求NotSigned：$File，实际状态：$($signature.Status)"
    }
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
    throw "内部产物根目录必须位于源码仓库之外"
}
if (-not [System.IO.Directory]::Exists($artifactRootFull)) {
    throw "内部产物根目录必须由管理员预先创建并配置ACL：$artifactRootFull"
}
$artifactRootItem = Get-Item -LiteralPath $artifactRootFull
if (($artifactRootItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
    throw "内部产物根目录不能是联接或符号链接"
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
    throw "内部产物根目录向Everyone、Authenticated Users或Users开放写权限"
}

Push-Location $projectRoot
$staging = $null
try {
    $status = Invoke-NativeText "读取Git状态" { git status --porcelain=v1 --untracked-files=all }
    if (-not [string]::IsNullOrWhiteSpace($status)) {
        throw "发布要求Git工作区完全干净"
    }
    $commit = (Invoke-NativeText "读取Git提交" { git rev-parse HEAD }).ToLowerInvariant()
    $tree = (Invoke-NativeText "读取Git Tree" { git rev-parse 'HEAD^{tree}' }).ToLowerInvariant()
    $branch = Invoke-NativeText "读取Git分支" { git branch --show-current }
    if ($commit -notmatch "^[0-9a-f]{40}$" -or $tree -notmatch "^[0-9a-f]{40}$") {
        throw "Git提交或Tree ID无效"
    }
    $shortCommit = $commit.Substring(0, 12)
    $tauriConfig = [System.IO.File]::ReadAllText((Join-Path $projectRoot "src-tauri\tauri.conf.json")) | ConvertFrom-Json
    $version = [string]$tauriConfig.version
    if ($version -notmatch "^\d+\.\d+\.\d+([-.][0-9A-Za-z.-]+)?$") {
        throw "Tauri版本号无效：$version"
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
        $afterGate = Invoke-NativeText "复查质量门禁后的Git状态" { git status --porcelain=v1 --untracked-files=all }
        if (-not [string]::IsNullOrWhiteSpace($afterGate)) {
            throw "质量门禁产生了未提交源码或Schema变更"
        }
    }

    Invoke-NativeStep "生成CycloneDX SBOM" {
        & (Join-Path $PSScriptRoot "generate-sbom.ps1") -OutputPath (Join-Path $staging "sbom.cdx.json") -Commit $commit
    }
    $buildStarted = (Get-Date).ToUniversalTime().AddMinutes(-1)
    Invoke-NativeStep "构建内部NSIS安装包" {
        pnpm tauri build --bundles nsis --config src-tauri/tauri.release.conf.json
    }
    $buildDependencyMetadata = Join-Path $projectRoot "src-tauri\target\release\deps\inxaiot_desk_buddy_lib.d"
    if (-not [System.IO.File]::Exists($buildDependencyMetadata)) {
        throw "缺少Rust Release构建依赖元数据，无法验证内嵌提交"
    }
    $expectedBuildCommit = "# env-dep:INX_BUILD_GIT_COMMIT=$shortCommit"
    $buildMetadataLines = [System.IO.File]::ReadAllLines($buildDependencyMetadata)
    if ($buildMetadataLines -notcontains $expectedBuildCommit) {
        throw "Rust Release构建未绑定当前干净Git提交"
    }
    $afterBuildStatus = Invoke-NativeText "复查发布构建后的Git状态" { git status --porcelain=v1 --untracked-files=all }
    $afterBuildCommit = (Invoke-NativeText "复查发布构建后的Git提交" { git rev-parse HEAD }).ToLowerInvariant()
    $afterBuildTree = (Invoke-NativeText "复查发布构建后的Git Tree" { git rev-parse 'HEAD^{tree}' }).ToLowerInvariant()
    if (-not [string]::IsNullOrWhiteSpace($afterBuildStatus) -or
        $afterBuildCommit -ne $commit -or $afterBuildTree -ne $tree) {
        throw "发布构建期间源码、Git提交或Tree发生变化"
    }
    $portableSource = Join-Path $projectRoot "src-tauri\target\release\inxaiot-desk-buddy.exe"
    $installerSource = Get-ChildItem (Join-Path $projectRoot "src-tauri\target\release\bundle\nsis") -Filter "*.exe" -File |
        Where-Object { $_.LastWriteTimeUtc -ge $buildStarted } |
        Sort-Object LastWriteTimeUtc -Descending |
        Select-Object -First 1
    if (-not (Test-Path -LiteralPath $portableSource) -or $null -eq $installerSource) {
        throw "Tauri没有生成预期的裸程序或NSIS安装包"
    }
    Assert-UnsignedFile -File $portableSource
    Assert-UnsignedFile -File $installerSource.FullName

    $portableTarget = Join-Path $staging "inxaiot-desk-buddy-$version-x64.exe"
    $installerTarget = Join-Path $staging "inxaiot-desk-buddy-$version-x64-setup.exe"
    Copy-Item -LiteralPath $portableSource -Destination $portableTarget
    Copy-Item -LiteralPath $installerSource.FullName -Destination $installerTarget
    $roles = [ordered]@{
        (Split-Path -Leaf $portableTarget) = "portable"
        (Split-Path -Leaf $installerTarget) = "installer"
        "sbom.cdx.json" = "sbom"
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
    $manifest = [ordered]@{
        schemaVersion = 2
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
        integrity = [ordered]@{
            policy = "internal-unsigned-sha256"
            authenticodeRequired = $false
            trustBoundary = "restricted-ntfs-acl-and-source-repository"
        }
        rollback = [ordered]@{
            strategy = "verify-hash-and-run-prior-immutable-installer"
            allowDowngrade = $true
            artifactRoot = $artifactRootFull
        }
        files = $files
    }
    $manifestPath = Join-Path $staging "release-manifest.json"
    [System.IO.File]::WriteAllText($manifestPath, ($manifest | ConvertTo-Json -Depth 20), [System.Text.UTF8Encoding]::new($false))
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
    Write-Host "RELEASE_TREE=$tree"
    Write-Host "RELEASE_INTEGRITY_POLICY=internal-unsigned-sha256"
}
finally {
    if ($null -ne $staging -and (Test-Path -LiteralPath $staging)) {
        $stagingFull = [System.IO.Path]::GetFullPath($staging)
        if ($stagingFull.StartsWith($artifactRootFull + [System.IO.Path]::DirectorySeparatorChar, [System.StringComparison]::OrdinalIgnoreCase) -and
            (Split-Path -Leaf $stagingFull).StartsWith(".staging-inxaiot-desk-buddy-")) {
            Remove-Item -LiteralPath $stagingFull -Recurse -Force
        }
    }
    Pop-Location
}
