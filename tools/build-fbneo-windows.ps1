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
# runners ship MSYS2 at C:\msys64; locally, install from https://www.msys2.org
# (or: winget install MSYS2.MSYS2).
#
# Note: written to be Windows PowerShell 5.1-safe. git writes progress and
# some status text to stderr, which PS 5.1 converts to terminating errors
# under $ErrorActionPreference = "Stop"; every native call here is routed
# through Invoke-Native (cmd /c with 2>&1) and checked via exit code.

$ErrorActionPreference = "Stop"

function Invoke-Native {
    param(
        [Parameter(Mandatory)] [string] $Command,
        [switch] $AllowFailure
    )
    # cmd /c keeps native stderr out of PowerShell's error stream entirely.
    $output = & cmd /c "$Command 2>&1"
    if (-not $AllowFailure -and $LASTEXITCODE -ne 0) {
        $output | Out-Host
        throw "Command failed (exit $LASTEXITCODE): $Command"
    }
    return $output
}

$FbneoRef  = if ($env:FBNEO_REF)  { $env:FBNEO_REF }  else { "cf53523f844b59d48748248a28f15b04d97f08d4" }
$FbneoRepo = if ($env:FBNEO_REPO) { $env:FBNEO_REPO } else { "https://github.com/libretro/FBNeo.git" }
$Msys      = if ($env:MSYS2_ROOT) { $env:MSYS2_ROOT } else { "C:\msys64" }

$Root      = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
$FbneoDir  = Join-Path $Root "vendor\FBNeo"
$CoresDir  = Join-Path $Root "cores"
$OutCore   = Join-Path $CoresDir "fbneo_mk2_libretro.dll"
$Bash      = Join-Path $Msys "usr\bin\bash.exe"

if (-not (Test-Path $Bash)) {
    throw "MSYS2 not found at $Msys (set MSYS2_ROOT). Install from https://www.msys2.org or: winget install MSYS2.MSYS2"
}
New-Item -ItemType Directory -Force -Path $CoresDir | Out-Null

# --- Fetch / pin --------------------------------------------------------------
if (-not (Test-Path (Join-Path $FbneoDir ".git"))) {
    Write-Host "Initializing vendor\FBNeo"
    New-Item -ItemType Directory -Force -Path $FbneoDir | Out-Null
    Invoke-Native "git init -q `"$FbneoDir`"" | Out-Null
    Invoke-Native "git -C `"$FbneoDir`" remote add origin $FbneoRepo" | Out-Null
}

# Empty repo (fresh init) has no HEAD; --verify --quiet fails silently.
$current = Invoke-Native "git -C `"$FbneoDir`" rev-parse --verify --quiet HEAD" -AllowFailure
if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace("$current")) { $current = "none" }

if ("$current".Trim() -ne $FbneoRef) {
    Write-Host "Fetching libretro/FBNeo @ $FbneoRef (shallow, ~100MB)..."
    Invoke-Native "git -C `"$FbneoDir`" fetch --depth 1 origin $FbneoRef" | Out-Host
    Invoke-Native "git -C `"$FbneoDir`" checkout -q --force $FbneoRef" | Out-Null
}
$pinned = ([string](Invoke-Native "git -C `"$FbneoDir`" rev-parse HEAD")).Trim()
Write-Host "FBNeo pinned at: $pinned"

Copy-Item (Join-Path $Root "tools\Makefile.mk2") (Join-Path $FbneoDir "src\burner\libretro\Makefile.mk2") -Force

# --- Build under MSYS2/MinGW64 -------------------------------------------------
$env:MSYSTEM = "MINGW64"
$env:CHERE_INVOKING = "1"

Write-Host "Ensuring mingw-w64 toolchain (pacman, no-op if installed)..."
& $Bash -lc "pacman -S --noconfirm --needed mingw-w64-x86_64-toolchain make perl git" | Out-Host
if ($LASTEXITCODE -ne 0) { throw "pacman toolchain install failed (exit $LASTEXITCODE)" }

$fbneoUnix = (& $Bash -lc "cygpath -u '$FbneoDir'")
if ($LASTEXITCODE -ne 0) { throw "cygpath failed" }
$fbneoUnix = "$fbneoUnix".Trim()

$buildCmd = @(
    "set -e",
    "cd '$fbneoUnix/src/burner/libretro'",
    "make SUBSET=mk2 platform=win64 generate-files",
    "make -j`$(nproc) SUBSET=mk2 platform=win64",
    "strip -s fbneo_mk2_libretro.dll"
) -join " && "
Write-Host "Building mk2 subset core (first build takes a few minutes)..."
& $Bash -lc $buildCmd | Out-Host
if ($LASTEXITCODE -ne 0) { throw "FBNeo mk2 subset build failed (exit $LASTEXITCODE)" }

Copy-Item (Join-Path $FbneoDir "src\burner\libretro\fbneo_mk2_libretro.dll") $OutCore -Force
Write-Host "Installed $OutCore ($([math]::Round((Get-Item $OutCore).Length / 1MB, 1)) MB)"

# The mk2 subset build only static-links libgcc/libstdc++ (see the Windows
# LDFLAGS in FBNeo's own libretro Makefile); libwinpthread-1.dll stays a
# dynamic dependency. MSYS2 has it on PATH inside this build shell, but a
# plain launch of freeplay.exe has neither MSYS2 nor that DLL on PATH, so
# the core fails to load with no visible error (LoadLibrary just fails).
# Stage it next to the core so package.ps1 can bundle it.
$WinPthreadSrc = Join-Path $Msys "mingw64\bin\libwinpthread-1.dll"
if (Test-Path $WinPthreadSrc) {
    Copy-Item $WinPthreadSrc (Join-Path $CoresDir "libwinpthread-1.dll") -Force
    Write-Host "Installed $(Join-Path $CoresDir 'libwinpthread-1.dll') (mk2 core runtime dependency)"
} else {
    Write-Warning "libwinpthread-1.dll not found at $WinPthreadSrc; the built core may fail to load at runtime without it."
}