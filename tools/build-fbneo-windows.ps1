# Ensure the FBNeo libretro core for Windows exists at .\cores\fbneo_libretro.dll.
# CI downloads the official libretro buildbot artifact because upstream FBNeo
# source layout/build requirements can drift. Local devs can still keep their
# own core in .\cores or next to the exe.

$ErrorActionPreference = "Stop"

$Root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
$CoresDir = Join-Path $Root "cores"
$OutCore = Join-Path $CoresDir "fbneo_libretro.dll"
$ZipPath = Join-Path $env:TEMP "fbneo_libretro.dll.zip"
$ExtractDir = Join-Path $env:TEMP "fbneo-libretro-core"
$BuildbotUrl = "https://buildbot.libretro.com/nightly/windows/x86_64/latest/fbneo_libretro.dll.zip"

New-Item -ItemType Directory -Force -Path $CoresDir | Out-Null

if (Test-Path $OutCore) {
    Write-Host "FBNeo core already present: $OutCore"
    exit 0
}

Write-Host "Downloading FBNeo libretro core from $BuildbotUrl"
Remove-Item $ZipPath -ErrorAction SilentlyContinue
Remove-Item $ExtractDir -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $ExtractDir | Out-Null

Invoke-WebRequest -Uri $BuildbotUrl -OutFile $ZipPath
Expand-Archive -Path $ZipPath -DestinationPath $ExtractDir -Force

$built = Get-ChildItem -Path $ExtractDir -Recurse -File |
    Where-Object { $_.Name -eq "fbneo_libretro.dll" } |
    Sort-Object LastWriteTime -Descending |
    Select-Object -First 1

if (-not $built) {
    throw "Downloaded FBNeo archive did not contain fbneo_libretro.dll."
}

Copy-Item $built.FullName $OutCore -Force
Write-Host "Installed $OutCore"
