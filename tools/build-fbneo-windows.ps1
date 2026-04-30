# Build the FBNeo libretro core for Windows into .\cores\fbneo_libretro.dll.
# Requires git plus MSYS2/MinGW make tooling on PATH (`make` or `mingw32-make`).

$ErrorActionPreference = "Stop"

$Root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
$VendorDir = Join-Path $Root "vendor"
$FbneoDir = Join-Path $VendorDir "FBNeo"
$CoresDir = Join-Path $Root "cores"
$OutCore = Join-Path $CoresDir "fbneo_libretro.dll"

New-Item -ItemType Directory -Force -Path $VendorDir | Out-Null
New-Item -ItemType Directory -Force -Path $CoresDir | Out-Null

if (-not (Test-Path $FbneoDir)) {
    git clone https://github.com/finalburnneo/FBNeo.git $FbneoDir
} else {
    git -C $FbneoDir pull --ff-only
}

$make = (Get-Command mingw32-make -ErrorAction SilentlyContinue)
if (-not $make) {
    $make = Get-Command make -ErrorAction Stop
}

& $make.Source -C (Join-Path $FbneoDir "src\burner\libretro") "-j$([Environment]::ProcessorCount)"

$built = Get-ChildItem -Path (Join-Path $FbneoDir "src\burner\libretro") -Recurse -File |
    Where-Object { $_.Name -match 'fbneo.*libretro.*\.(dll|DLL)$' -or $_.Name -eq "fbneo_libretro.dll" } |
    Sort-Object LastWriteTime -Descending |
    Select-Object -First 1

if (-not $built) {
    throw "FBNeo build finished, but fbneo_libretro.dll was not found."
}

Copy-Item $built.FullName $OutCore -Force
Write-Host "Built $OutCore"
