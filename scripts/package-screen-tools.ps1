param(
    [Parameter(Mandatory = $true)][string]$OutputDirectory,
    [Parameter(Mandatory = $true)][string]$AndroidSdkRoot,
    [Parameter(Mandatory = $true)][string]$JavaRuntimeRoot
)
$ErrorActionPreference = 'Stop'
& node (Join-Path $PSScriptRoot 'package-screen-tools.mjs') --platform win32 --arch x64 --output $OutputDirectory --sdk $AndroidSdkRoot --java $JavaRuntimeRoot
if ($LASTEXITCODE -ne 0) { throw '随包工具生成或运行检查失败' }
