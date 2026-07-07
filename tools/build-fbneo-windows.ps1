# Build the MK2-only (Midway T-Unit subset) FBNeo libretro core for Windows,
# pinned to a known commit, installed to .\cores\fbneo_mk2_libretro.dll.
#
# This replaces the old buildbot download: the buildbot nightly is (a) the
# full ~35MB every-driver core and (b) UNPINNED — whatever FBNeo master was
# that day. FBNeo savestates and simulation drift between commits, so an
# unpinned core is a cross-platform/cross-build-date desync vector that the
# ROM hash check cannot catch. Building from the same pinned ref as
# Linux/macOS closes it. All three build-fbneo-* scripts must pin the SAME
# ref; see build-fbneo-linux.sh for the full rationale.
#
# Requires MSYS2 with the mingw-w64 toolchain. GitHub's windows-latest
# runners ship MSYS2 at C:\msys64; locally, install from https://www.msys2.org.

$ErrorActionPreference = "Stop"

$FbneoRef  = if ($env:FBNEO_REF)  { $env:FBNEO_REF }  else { "cf53523f844b59d48748248a28f15b04d97f08d4" }
$FbneoRepo = if ($env:FBNEO_REPO) { $env:FBNEO_REPO } else { "https://github.com/libretro/FBNeo.git" }
$Msys      = if ($env:MSYS2_ROOT) { $env:MSYS2_ROOT } else { "C:\msys64" }

$Root      = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
$FbneoDir  = Join-Path $Root "vendor\FBNeo"
$CoresDir  = Join-Path $Root "cores"
$OutCore   = Join-Path $CoresDir "fbneo_mk2_libretro.dll"
$Bash      = Join-Path $Msys "usr\bin\bash.exe"

if (-not (Test-Path $Bash)) {
    throw "MSYS2 not found at $Msys (set MSYS2_ROOT). Install from https://www.msys2.org."
}
New-Item -ItemType Directory -Force -Path $CoresDir | Out-Null

# --- Fetch / pin --------------------------------------------------------------
if (-not (Test-Path (Join-Path $FbneoDir ".git"))) {
    Write-Host "Cloning libretro/FBNeo @ $FbneoRef"
    git init -q $FbneoDir
    git -C $FbneoDir remote add origin $FbneoRepo
}
$current = (git -C $FbneoDir rev-parse HEAD 2>$null); if (-not $current) { $current = "none" }
if ($current -ne $FbneoRef) {
    git -C $FbneoDir fetch --depth 1 origin $FbneoRef
    git -C $FbneoDir checkout -q --force $FbneoRef
}
Write-Host "FBNeo pinned at: $(git -C $FbneoDir rev-parse HEAD)"

Copy-Item (Join-Path $Root "tools\Makefile.mk2") (Join-Path $FbneoDir "src\burner\libretro\Makefile.mk2") -Force

# --- Build under MSYS2/MinGW64 -------------------------------------------------
$env:MSYSTEM = "MINGW64"
$env:CHERE_INVOKING = "1"

# Toolchain (no-op if already installed; runners cache pacman packages poorly,
# but this is only a few packages).
& $Bash -lc "pacman -S --noconfirm --needed mingw-w64-x86_64-toolchain make perl git" | Out-Host
if ($LASTEXITCODE -ne 0) { throw "pacman toolchain install failed" }

$fbneoUnix = (& $Bash -lc "cygpath -u '$FbneoDir'").Trim()
$buildCmd = @(
    "set -e",
    "cd '$fbneoUnix/src/burner/libretro'",
    "make SUBSET=mk2 platform=win64 generate-files",
    "make -j`$(nproc) SUBSET=mk2 platform=win64",
    "strip -s fbneo_mk2_libretro.dll"
) -join " && "
& $Bash -lc $buildCmd | Out-Host
if ($LASTEXITCODE -ne 0) { throw "FBNeo mk2 subset build failed" }

Copy-Item (Join-Path $FbneoDir "src\burner\libretro\fbneo_mk2_libretro.dll") $OutCore -Force
Write-Host "Installed $OutCore ($([math]::Round((Get-Item $OutCore).Length / 1MB, 1)) MB)"
