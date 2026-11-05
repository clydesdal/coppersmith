# Builds coppersmith.exe and puts it in a zip in target\. Run in PowerShell on
# Windows with Rust and the Visual Studio Build Tools ("Desktop development
# with C++") installed:   powershell -ExecutionPolicy Bypass -File packaging\windows\make-zip.ps1
$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..\..")
cargo build --release
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.*)"').Matches[0].Groups[1].Value
$out = "target\coppersmith-$version-windows"
if (Test-Path $out) { Remove-Item -Recurse -Force $out }
New-Item -ItemType Directory $out | Out-Null
Copy-Item target\release\coppersmith.exe $out
Copy-Item README.md $out
Compress-Archive -Force -Path $out -DestinationPath "$out.zip"
Write-Host "Made $out.zip"
