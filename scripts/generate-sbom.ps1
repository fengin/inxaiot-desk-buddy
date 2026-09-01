param(
    [Parameter(Mandatory = $true)]
    [ValidateNotNullOrEmpty()]
    [string]$OutputPath,
    [Parameter(Mandatory = $true)]
    [ValidatePattern("^[0-9a-fA-F]{40}$")]
    [string]$Commit
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

function Invoke-JsonCommand {
    param(
        [string]$Name,
        [scriptblock]$Command
    )
    $output = & $Command | Out-String
    if ($LASTEXITCODE -ne 0) {
        throw "$Name 失败，退出码：$LASTEXITCODE"
    }
    return $output | ConvertFrom-Json
}

function Add-Component {
    param(
        [hashtable]$Index,
        [string]$Type,
        [string]$Name,
        [string]$Version,
        [string]$Purl
    )
    if ([string]::IsNullOrWhiteSpace($Name) -or [string]::IsNullOrWhiteSpace($Version)) {
        return
    }
    $key = "$Type`:$Name`:$Version"
    if (-not $Index.ContainsKey($key)) {
        $Index[$key] = [ordered]@{
            type = "library"
            name = $Name
            version = $Version
            purl = $Purl
            "bom-ref" = $Purl
            properties = @(
                [ordered]@{ name = "inxaiot:ecosystem"; value = $Type }
            )
        }
    }
}

function Add-NpmTree {
    param(
        [hashtable]$Index,
        [object]$Node,
        [string]$NameHint = ""
    )
    if ($null -eq $Node) {
        return
    }
    $name = if ($Node.PSObject.Properties.Name -contains "name" -and $Node.name) { [string]$Node.name } else { $NameHint }
    $version = if ($Node.PSObject.Properties.Name -contains "version" -and $Node.version) { [string]$Node.version } else { "" }
    if ($name -and $version) {
        $purlName = [System.Uri]::EscapeDataString($name).Replace("%2F", "/")
        Add-Component -Index $Index -Type "npm" -Name $name -Version $version -Purl "pkg:npm/$purlName@$version"
    }
    if ($Node.PSObject.Properties.Name -contains "dependencies" -and $null -ne $Node.dependencies) {
        foreach ($dependency in $Node.dependencies.PSObject.Properties) {
            Add-NpmTree -Index $Index -Node $dependency.Value -NameHint $dependency.Name
        }
    }
}

$projectRoot = Split-Path -Parent $PSScriptRoot
$components = @{}
Push-Location (Join-Path $projectRoot "src-tauri")
try {
    $cargo = Invoke-JsonCommand "cargo metadata" { cargo metadata --format-version 1 --locked }
}
finally {
    Pop-Location
}
foreach ($package in $cargo.packages) {
    Add-Component -Index $components -Type "cargo" -Name ([string]$package.name) -Version ([string]$package.version) -Purl "pkg:cargo/$($package.name)@$($package.version)"
}

Push-Location $projectRoot
try {
    $npmRoots = Invoke-JsonCommand "pnpm list" { pnpm list --prod --json --depth Infinity }
}
finally {
    Pop-Location
}
foreach ($root in @($npmRoots)) {
    Add-NpmTree -Index $components -Node $root
}

$appVersion = ([System.IO.File]::ReadAllText((Join-Path $projectRoot "package.json")) | ConvertFrom-Json).version
$bom = [ordered]@{
    bomFormat = "CycloneDX"
    specVersion = "1.5"
    serialNumber = "urn:uuid:$([Guid]::NewGuid())"
    version = 1
    metadata = [ordered]@{
        timestamp = (Get-Date).ToUniversalTime().ToString("o")
        tools = [ordered]@{
            components = @(
                [ordered]@{ type = "application"; name = "inxaiot-release-pipeline"; version = "1" }
            )
        }
        component = [ordered]@{
            type = "application"
            name = "inxaiot-desk-buddy"
            version = $appVersion
            "bom-ref" = "pkg:generic/inxaiot-desk-buddy@$appVersion"
            properties = @(
                [ordered]@{ name = "inxaiot:git-commit"; value = $Commit.ToLowerInvariant() }
            )
        }
    }
    components = @($components.Values | Sort-Object { $_.purl })
}
$resolvedOutput = [System.IO.Path]::GetFullPath($OutputPath)
$parent = Split-Path -Parent $resolvedOutput
$null = New-Item -ItemType Directory -Path $parent -Force
[System.IO.File]::WriteAllText($resolvedOutput, ($bom | ConvertTo-Json -Depth 100), [System.Text.UTF8Encoding]::new($false))
$null = [System.IO.File]::ReadAllText($resolvedOutput) | ConvertFrom-Json
Write-Host "SBOM_COMPONENT_COUNT=$($components.Count)"
