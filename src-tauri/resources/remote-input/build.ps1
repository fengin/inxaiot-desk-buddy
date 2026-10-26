param([string]$Sdk = 'D:\env\AndroidSdk')
$ErrorActionPreference = 'Stop'
$resourceRoot = $PSScriptRoot
$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $resourceRoot '..\..\..'))
$buildRoot = Join-Path $repoRoot ('.review-tools\remote-input-build\' + [guid]::NewGuid().ToString('N'))
$classes = Join-Path $buildRoot 'classes'
$testClasses = Join-Path $buildRoot 'tests'
$dex = Join-Path $buildRoot 'dex'
New-Item -ItemType Directory -Force -Path $classes, $testClasses, $dex | Out-Null
$androidJar = Join-Path $Sdk 'platforms\android-30\android.jar'
$uiautomatorJar = Join-Path $Sdk 'platforms\android-30\uiautomator.jar'
$testBaseJar = Join-Path $Sdk 'platforms\android-30\optional\android.test.base.jar'
$sourceFiles = @(Get-ChildItem -LiteralPath (Join-Path $resourceRoot 'src') -Recurse -Filter '*.java' | ForEach-Object { $_.FullName })
& javac --release 8 -encoding UTF-8 -classpath "$androidJar;$uiautomatorJar;$testBaseJar" -d $classes @sourceFiles
if ($LASTEXITCODE -ne 0) { throw 'Java 编译失败' }
& javac --release 8 -encoding UTF-8 -classpath $classes -d $testClasses (Join-Path $resourceRoot 'test\com\inx\remote\TextProtocolTest.java')
if ($LASTEXITCODE -ne 0) { throw '协议测试编译失败' }
& java -classpath "$classes;$testClasses" com.inx.remote.TextProtocolTest
if ($LASTEXITCODE -ne 0) { throw '协议测试失败' }
$classFiles = @(Get-ChildItem -LiteralPath $classes -Recurse -Filter '*.class' | ForEach-Object { $_.FullName })
& (Join-Path $Sdk 'build-tools\35.0.1\d8.bat') --min-api 24 --lib $androidJar --classpath $uiautomatorJar --classpath $testBaseJar --output $dex @classFiles
if ($LASTEXITCODE -ne 0) { throw 'DEX 编译失败' }
& jar --create --date=2026-01-01T00:00:00Z --file (Join-Path $resourceRoot 'focused-text.jar') -C $dex classes.dex
if ($LASTEXITCODE -ne 0) { throw 'JAR 打包失败' }
Get-FileHash -LiteralPath (Join-Path $resourceRoot 'focused-text.jar') -Algorithm SHA256 | Format-List
$jarHash = (Get-FileHash -LiteralPath (Join-Path $resourceRoot 'focused-text.jar') -Algorithm SHA256).Hash.ToLowerInvariant()
[System.IO.File]::WriteAllText((Join-Path $resourceRoot 'focused-text.sha256'), $jarHash + "`n", [System.Text.UTF8Encoding]::new($false))
$sourceHashes = [ordered]@{}
$manifestFiles = @($sourceFiles) + @(Join-Path $resourceRoot 'test\com\inx\remote\TextProtocolTest.java') + @($PSCommandPath)
foreach ($sourceFile in ($manifestFiles | Sort-Object)) {
    $relative = [System.IO.Path]::GetRelativePath($resourceRoot, $sourceFile).Replace('\', '/')
    $sourceHashes[$relative] = (Get-FileHash -LiteralPath $sourceFile -Algorithm SHA256).Hash.ToLowerInvariant()
}
$manifest = [ordered]@{ jar = $jarHash; sources = $sourceHashes }
[System.IO.File]::WriteAllText((Join-Path $resourceRoot 'source-manifest.json'), ($manifest | ConvertTo-Json -Depth 4) + "`n", [System.Text.UTF8Encoding]::new($false))
