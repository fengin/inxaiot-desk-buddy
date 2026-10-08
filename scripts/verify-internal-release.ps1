param(
    [Parameter(Mandatory = $true)]
    [ValidateNotNullOrEmpty()]
    [string]$ArtifactSet
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

function Invoke-GitText {
    param([scriptblock]$Command)
    $output = & $Command | Out-String
    if ($LASTEXITCODE -ne 0) {
        throw "Git来源复验失败，退出码：$LASTEXITCODE"
    }
    return $output.Trim().ToLowerInvariant()
}

$root = [System.IO.Path]::GetFullPath($ArtifactSet)
if (-not [System.IO.Directory]::Exists($root)) {
    throw "产物集目录不存在：$root"
}
$rootItem = Get-Item -LiteralPath $root
if (($rootItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
    throw "产物集目录不能是联接或符号链接"
}
$manifestPath = Join-Path $root "release-manifest.json"
$checksumsPath = Join-Path $root "checksums.sha256"
if (-not (Test-Path -LiteralPath $manifestPath) -or -not (Test-Path -LiteralPath $checksumsPath)) {
    throw "产物集缺少release-manifest.json或checksums.sha256"
}
if ((Test-Path -LiteralPath (Join-Path $root "release-manifest.p7s")) -or
    (Test-Path -LiteralPath (Join-Path $root "signing-certificate.cer"))) {
    throw "内部无签名产物集不得混入历史签名文件"
}
$manifest = [System.IO.File]::ReadAllText($manifestPath) | ConvertFrom-Json
if ($manifest.schemaVersion -notin @(2, 3, 4) -or $manifest.integrity.policy -ne "internal-unsigned-sha256" -or
    $manifest.integrity.authenticodeRequired -ne $false -or $manifest.git.dirty -ne $false) {
    throw "内部产物清单策略或Git状态无效"
}
$requiredRoles = @("portable", "sbom")
if ($manifest.schemaVersion -eq 4) { $requiredRoles += 'screen-tool' }
if ($manifest.schemaVersion -eq 2) {
    # 保留历史NSIS产物的复验能力，不将旧安装包当成新便携版。
    $requiredRoles += "installer"
}
elseif ($manifest.distribution.os -ne "windows" -or
    $manifest.distribution.architecture -ne "x64" -or
    $manifest.distribution.package -ne $(if ($manifest.schemaVersion -eq 4) { 'portable-directory' } else { 'portable-exe' })) {
    throw "免安装产物平台或分发类型无效"
}
$commit = ([string]$manifest.git.commit).ToLowerInvariant()
$tree = ([string]$manifest.git.tree).ToLowerInvariant()
if ($commit -notmatch "^[0-9a-f]{40}$" -or $tree -notmatch "^[0-9a-f]{40}$") {
    throw "清单Git提交或Tree ID无效"
}
$expectedDirectory = "inxaiot-desk-buddy-$($manifest.version)-$($commit.Substring(0, 12))"
if ((Split-Path -Leaf $root) -ne $expectedDirectory) {
    throw "产物目录名与版本/提交不一致"
}

$manifestNames = @{}
function Assert-RelativeArtifactName([string]$Name) {
    if ([string]::IsNullOrWhiteSpace($Name) -or [IO.Path]::IsPathRooted($Name) -or $Name.Contains('\') -or $Name.Contains(':') -or ($Name.Split('/') | Where-Object { $_ -in @('', '.', '..') })) {
        throw "产物相对路径非法：$Name"
    }
    $resolved = [IO.Path]::GetFullPath((Join-Path $root $Name))
    if (-not $resolved.StartsWith($root + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) { throw '产物路径越界' }
}
$roleCounts = @{}
foreach ($entry in $manifest.files) {
    $name = [string]$entry.name
    Assert-RelativeArtifactName $name
    if (($manifest.schemaVersion -lt 4 -and [System.IO.Path]::GetFileName($name) -ne $name) -or $manifestNames.ContainsKey($name)) {
        throw "清单文件名非法或重复：$name"
    }
    $manifestNames[$name] = $true
    $role = [string]$entry.role
    if ($role -notin $requiredRoles) {
        throw "清单文件角色非法：$role"
    }
    if (-not $roleCounts.ContainsKey($role)) {
        $roleCounts[$role] = 0
    }
    $roleCounts[$role] += 1
    $filePath = Join-Path $root $name
    if (-not [System.IO.File]::Exists($filePath)) {
        throw "清单文件不存在：$name"
    }
    $file = Get-Item -LiteralPath $filePath
    if ($file.Length -ne [long]$entry.size) {
        throw "文件大小不一致：$name"
    }
    $hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $filePath).Hash.ToLowerInvariant()
    if ($hash -ne [string]$entry.sha256) {
        throw "文件哈希不一致：$name"
    }
    if ($role -in @("portable", "installer")) {
        $authenticode = Get-AuthenticodeSignature -FilePath $filePath
        if ($authenticode.Status -ne [System.Management.Automation.SignatureStatus]::NotSigned) {
            throw "内部无签名策略要求NotSigned：$name，实际状态：$($authenticode.Status)"
        }
    }
}
foreach ($requiredRole in $requiredRoles) {
    if ($requiredRole -eq 'screen-tool' -and $roleCounts.ContainsKey($requiredRole) -and $roleCounts[$requiredRole] -gt 0) { continue }
    if (-not $roleCounts.ContainsKey($requiredRole) -or $roleCounts[$requiredRole] -ne 1) {
        throw "清单必须且只能包含一个$requiredRole文件"
    }
}

$checksumNames = @{}
foreach ($line in [System.IO.File]::ReadAllLines($checksumsPath)) {
    if ($line -notmatch "^([0-9a-f]{64})  (.+)$") {
        throw "checksums.sha256行格式无效"
    }
    $hash = $matches[1]
    $name = $matches[2]
    Assert-RelativeArtifactName $name
    if ($checksumNames.ContainsKey($name)) {
        throw "checksums.sha256文件名重复：$name"
    }
    $checksumNames[$name] = $true
    $path = Join-Path $root $name
    if (-not [System.IO.File]::Exists($path)) {
        throw "checksums.sha256引用文件不存在：$name"
    }
    $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $path).Hash.ToLowerInvariant()
    if ($actual -ne $hash) {
        throw "checksums.sha256校验失败：$name"
    }
}
foreach ($required in @($manifestNames.Keys) + @("release-manifest.json")) {
    if (-not $checksumNames.ContainsKey($required)) {
        throw "checksums.sha256未覆盖：$required"
    }
}
$actualCoveredNames = @(Get-ChildItem $root -File -Recurse |
    Where-Object { $_.Name -ne "checksums.sha256" } |
    ForEach-Object { [IO.Path]::GetRelativePath($root, $_.FullName).Replace('\', '/') } |
    Sort-Object)
$declaredCoveredNames = @($checksumNames.Keys | Sort-Object)
if (($actualCoveredNames -join [Environment]::NewLine) -ne ($declaredCoveredNames -join [Environment]::NewLine)) {
    throw "checksums.sha256与产物目录文件集合不一致"
}
foreach ($file in Get-ChildItem $root -File -Recurse) {
    if (-not $file.IsReadOnly) {
        throw "产物文件未设置只读属性：$($file.Name)"
    }
}

if ($manifest.schemaVersion -eq 4) {
    & node (Join-Path $PSScriptRoot 'package-screen-tools.mjs') --platform win32 --arch x64 --output $root --verify
    if ($LASTEXITCODE -ne 0) { throw '智能屏工具不完整或无法运行' }
}

$projectRoot = [System.IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
Push-Location $projectRoot
try {
    $null = Invoke-GitText { git cat-file -e "$commit`^{commit}" }
    $actualTree = Invoke-GitText { git rev-parse "$commit`^{tree}" }
    if ($actualTree -ne $tree) {
        throw "清单Git Tree与来源提交不一致"
    }
}
finally {
    Pop-Location
}
Write-Host "INTERNAL_RELEASE_VERIFIED=$root"
Write-Host "RELEASE_COMMIT=$commit"
Write-Host "RELEASE_TREE=$tree"
Write-Host "RELEASE_VERSION=$($manifest.version)"
Write-Host "RELEASE_INTEGRITY_POLICY=$($manifest.integrity.policy)"
