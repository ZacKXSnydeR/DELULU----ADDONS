param(
  [Parameter(Mandatory = $true)]
  [string]$Version,
  [Parameter(Mandatory = $false)]
  [string]$RepoOwner = "REPO_OWNER",
  [Parameter(Mandatory = $false)]
  [string]$RepoName = "REPO_NAME"
)

$ErrorActionPreference = "Stop"

$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

Write-Host "[1/4] Building x64..." -ForegroundColor Cyan
cargo build --release --target x86_64-pc-windows-msvc

Write-Host "[2/4] Building x86..." -ForegroundColor Cyan
cargo build --release --target i686-pc-windows-msvc

$outDir = Join-Path $root "release-artifacts"
if (Test-Path $outDir) { Remove-Item -LiteralPath $outDir -Recurse -Force }
New-Item -ItemType Directory -Path $outDir | Out-Null

$x64Src = Join-Path $root "target\x86_64-pc-windows-msvc\release\embegator.exe"
$x86Src = Join-Path $root "target\i686-pc-windows-msvc\release\embegator.exe"
$x64Out = Join-Path $outDir "embegator-windows-x64.exe"
$x86Out = Join-Path $outDir "embegator-windows-x86.exe"

Copy-Item $x64Src $x64Out -Force
Copy-Item $x86Src $x86Out -Force

$x64Hash = (Get-FileHash $x64Out -Algorithm SHA256).Hash.ToLowerInvariant()
$x86Hash = (Get-FileHash $x86Out -Algorithm SHA256).Hash.ToLowerInvariant()

Write-Host "[3/4] Creating unsigned manifest..." -ForegroundColor Cyan
$manifestTemplatePath = Join-Path $root "addon.manifest.template.json"
$manifestOutPath = Join-Path $outDir "addon.manifest.json"

$manifest = Get-Content $manifestTemplatePath -Raw | ConvertFrom-Json
$manifest.version = $Version
$manifest.platformAssets."windows-x64".sha256 = $x64Hash
$manifest.platformAssets."windows-x86".sha256 = $x86Hash
$manifest.platformAssets."windows-x64".downloadUrl = "https://github.com/$RepoOwner/$RepoName/releases/download/v$Version/embegator-windows-x64.exe"
$manifest.platformAssets."windows-x86".downloadUrl = "https://github.com/$RepoOwner/$RepoName/releases/download/v$Version/embegator-windows-x86.exe"
$manifest.releaseNotesUrl = "https://github.com/$RepoOwner/$RepoName/releases/tag/v$Version"
$manifest.homepageUrl = "https://github.com/$RepoOwner/$RepoName"
$manifest.signature = "REPLACE_WITH_BASE64_SIGNATURE"

$manifest | ConvertTo-Json -Depth 10 | Set-Content $manifestOutPath -Encoding UTF8

Write-Host "[4/4] Done." -ForegroundColor Green
Write-Host "Artifacts: $outDir"
Write-Host "x64 sha256: $x64Hash"
Write-Host "x86 sha256: $x86Hash"
Write-Host "Next: sign addon.manifest.json and publish release assets."
