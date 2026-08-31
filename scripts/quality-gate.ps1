$ErrorActionPreference = "Stop"

function Invoke-NativeStep {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Name,
        [Parameter(Mandatory = $true)]
        [scriptblock]$Command
    )

    Write-Host "==> $Name"
    & $Command
    if ($LASTEXITCODE -ne 0) {
        throw "$Name 失败，退出码：$LASTEXITCODE"
    }
}

$projectRoot = Split-Path -Parent $PSScriptRoot
Push-Location $projectRoot
try {
    Invoke-NativeStep "前端类型检查" { pnpm run typecheck }
    Invoke-NativeStep "前端严格Lint" { pnpm run lint }
    Invoke-NativeStep "前端自动化测试" { pnpm run test }
    Invoke-NativeStep "前端生产构建" { pnpm run build }

    Push-Location (Join-Path $projectRoot "src-tauri")
    try {
        Invoke-NativeStep "Rust格式检查" { cargo fmt --all --check }
        Invoke-NativeStep "Rust严格Clippy" { cargo clippy --all-targets --all-features -- -D warnings }
        Invoke-NativeStep "Rust非忽略测试" { cargo test --all-targets --all-features }
    }
    finally {
        Pop-Location
    }

    Invoke-NativeStep "Windows Release裸程序构建" { pnpm tauri build --no-bundle }
    Write-Host "全部本地质量门禁通过。"
}
finally {
    Pop-Location
}
