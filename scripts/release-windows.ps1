param(
    [string]$ArtifactRoot = "",
    [switch]$SkipQualityGate
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

# 历史命令兼容入口；正式内部发布统一由无签名发布链负责。
& (Join-Path $PSScriptRoot "release-internal.ps1") -ArtifactRoot $ArtifactRoot -SkipQualityGate:$SkipQualityGate
