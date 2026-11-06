# Builds coppersmith.exe and wraps it in an installer, target\coppersmith-VERSION-setup.exe.
# Run in PowerShell on Windows with Rust, the Visual Studio Build Tools ("Desktop
# development with C++") and Inno Setup 6 (winget install JRSoftware.InnoSetup):
#   powershell -ExecutionPolicy Bypass -File packaging\windows\make-installer.ps1
$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..\..")
cargo build --release
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.*)"').Matches[0].Groups[1].Value

# The icon, from the same pictures the other platforms use: an .ico can hold
# PNGs as they are, so it's a small header, one entry per size, then the files.
$sizes = 16, 24, 32, 48, 64, 128, 256
$pngs = $sizes | ForEach-Object { , [IO.File]::ReadAllBytes((Resolve-Path "assets\icons\$_.png")) }
$ico = New-Object IO.MemoryStream
$w = New-Object IO.BinaryWriter $ico
$w.Write([uint16]0); $w.Write([uint16]1); $w.Write([uint16]$sizes.Count)
$offset = 6 + 16 * $sizes.Count
for ($i = 0; $i -lt $sizes.Count; $i++) {
    $side = [byte]($sizes[$i] % 256)   # 0 means 256
    $w.Write($side); $w.Write($side); $w.Write([byte]0); $w.Write([byte]0)
    $w.Write([uint16]1); $w.Write([uint16]32)
    $w.Write([uint32]$pngs[$i].Length); $w.Write([uint32]$offset)
    $offset += $pngs[$i].Length
}
foreach ($png in $pngs) { $w.Write($png) }
[IO.File]::WriteAllBytes("target\coppersmith.ico", $ico.ToArray())

$iscc = (Get-Command iscc -ErrorAction SilentlyContinue).Source
if (-not $iscc) {
    $iscc = @("${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
              "$env:ProgramFiles\Inno Setup 6\ISCC.exe",
              "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe") | Where-Object { Test-Path $_ } | Select-Object -First 1
}
if (-not $iscc) { throw "Inno Setup 6 isn't installed: winget install JRSoftware.InnoSetup" }
& $iscc /Q "/DVersion=$version" "/DRoot=$(Get-Location)" packaging\windows\coppersmith.iss
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
Write-Host "Made target\coppersmith-$version-setup.exe"
