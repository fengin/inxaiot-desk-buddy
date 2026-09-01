param(
    [Parameter(Mandatory = $true)]
    [ValidateNotNullOrEmpty()]
    [string]$ArtifactSet
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

# 历史命令兼容入口；统一验证内部无签名产物集。
& (Join-Path $PSScriptRoot "verify-internal-release.ps1") -ArtifactSet $ArtifactSet
