# package.ps1 — Build and zip a distributable freeplay-gametalk client package
# Run from the freeplay project root after cargo build --release

$ErrorActionPreference = "Stop"

$VERSION = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"' | ForEach-Object { $_.Matches[0].Groups[1].Value })
if (-not $VERSION) { $VERSION = "dev" }

$OUT_DIR = "dist\freeplay-gametalk-v$VERSION"
$ZIP_FILE = "dist\freeplay-gametalk-v$VERSION.zip"
$EXE_NAME = "freeplay.exe"

Write-Host "Packaging freeplay-gametalk v$VERSION..." -ForegroundColor Cyan
Write-Host ""

if (Test-Path $OUT_DIR) { Remove-Item -Recurse -Force $OUT_DIR }
New-Item -ItemType Directory -Force -Path $OUT_DIR | Out-Null

# ── Copy the main exe ─────────────────────────────────────────────────────────
$EXE_PATH = "target\release\$EXE_NAME"
if (-not (Test-Path $EXE_PATH)) {
    if (Test-Path "target\release\freeplay-gametalk.exe") {
        Write-Host "  Using freeplay-gametalk.exe, renaming to $EXE_NAME" -ForegroundColor Yellow
        Copy-Item "target\release\freeplay-gametalk.exe" "$OUT_DIR\$EXE_NAME"
    } else {
        Write-Host "  ❌ No exe found. Run: cargo build --release" -ForegroundColor Red
        exit 1
    }
} else {
    Write-Host "  ✓ Copying $EXE_NAME"
    Copy-Item $EXE_PATH $OUT_DIR
}

# ── Copy all DLLs from target\release ─────────────────────────────────────────
Write-Host "  ✓ Copying DLLs from target\release\"
Get-ChildItem "target\release\*.dll" -ErrorAction SilentlyContinue | ForEach-Object {
    Copy-Item $_.FullName -Destination $OUT_DIR
    Write-Host "      - $($_.Name)"
}

# ── Ensure FBNeo core ─────────────────────────────────────────────────────────
if (-not (Test-Path "$OUT_DIR\fbneo_libretro.dll")) {
    $fbneo_candidates = @(
        "fbneo_libretro.dll",
        "cores\fbneo_libretro.dll",
        "$env:USERPROFILE\AppData\Roaming\RetroArch\cores\fbneo_libretro.dll"
    )
    foreach ($path in $fbneo_candidates) {
        if (Test-Path $path) {
            Copy-Item $path "$OUT_DIR\fbneo_libretro.dll"
            Write-Host "  ✓ Copied fbneo_libretro.dll from $path"
            break
        }
    }
}

# ── Ensure SDL2 DLLs ──────────────────────────────────────────────────────────
$sdl_needed = @("SDL2.dll", "SDL2_ttf.dll")
$sdl_search_paths = @("C:\sdl2\lib\x64", "$env:USERPROFILE\.cargo\sdl2\lib", "lib")
foreach ($dll in $sdl_needed) {
    if (-not (Test-Path "$OUT_DIR\$dll")) {
        foreach ($search in $sdl_search_paths) {
            if (Test-Path "$search\$dll") {
                Copy-Item "$search\$dll" $OUT_DIR
                Write-Host "  ✓ Copied $dll from $search"
                break
            }
        }
    }
}

# ── Copy config ───────────────────────────────────────────────────────────────
if (Test-Path "config.toml") {
    Copy-Item "config.toml" $OUT_DIR
    Write-Host "  ✓ Copied config.toml"
    # Scrub Discord webhook URL from the distributed config
    $cfgPath = "$OUT_DIR\config.toml"
    if (Test-Path $cfgPath) {
        $cfg = Get-Content $cfgPath -Raw
        $cfg = $cfg -replace '(discord_webhook_url\s*=\s*")[^"]*(")', '${1}https://discord.com/api/webhooks/your-webhook-url-here${2}'
        Set-Content $cfgPath $cfg -NoNewline
        Write-Host "  ✓ Scrubbed webhook URL from config.toml"
    }
}
if (Test-Path ".env.example") {
    Copy-Item ".env.example" $OUT_DIR
    Write-Host "  ✓ Copied .env.example"
}

# ── Copy media folder (tracked runtime font only) ─────────────────────────────
$ttf_path = @("src\media\mk2.ttf", "media\mk2.ttf", "mk2.ttf") |
    Where-Object { Test-Path $_ } |
    Select-Object -First 1
if ($ttf_path) {
    New-Item -ItemType Directory -Force -Path "$OUT_DIR\media" | Out-Null
    Copy-Item $ttf_path "$OUT_DIR\media\mk2.ttf" -Force
    Write-Host "  ✓ Copied mk2.ttf → media\mk2.ttf"
} else {
    Write-Host "  ⚠ mk2.ttf not found — app will use bitmap fallback" -ForegroundColor Yellow
}

$regular_ttf_path = @("src\media\regular.ttf", "media\regular.ttf", "regular.ttf") |
    Where-Object { Test-Path $_ } |
    Select-Object -First 1
if ($regular_ttf_path) {
    New-Item -ItemType Directory -Force -Path "$OUT_DIR\media" | Out-Null
    Copy-Item $regular_ttf_path "$OUT_DIR\media\regular.ttf" -Force
    Write-Host "  ✓ Copied regular.ttf → media\regular.ttf"
}

$n27_ttf_path = @(
    "src\media\N27-Regular.ttf",
    "media\N27-Regular.ttf",
    "N27-Regular.ttf",
    "src\media\N27-Regular.otf",
    "media\N27-Regular.otf",
    "N27-Regular.otf"
) |
    Where-Object { Test-Path $_ } |
    Select-Object -First 1
if ($n27_ttf_path) {
    New-Item -ItemType Directory -Force -Path "$OUT_DIR\media" | Out-Null
    $n27_ext = [IO.Path]::GetExtension($n27_ttf_path)
    Copy-Item $n27_ttf_path "$OUT_DIR\media\N27-Regular$n27_ext" -Force
    Write-Host "  ✓ Copied N27-Regular$n27_ext → media\N27-Regular$n27_ext"
}

if (Test-Path "src\app_icon.bmp") {
    Copy-Item "src\app_icon.bmp" "$OUT_DIR\app_icon.bmp" -Force
    Write-Host "  ✓ Copied app_icon.bmp"
}

# ── Copy xband.reg ────────────────────────────────────────────────────────────
if (Test-Path "xband.reg") {
    Copy-Item "xband.reg" $OUT_DIR
    Write-Host "  ✓ Copied xband.reg"
}

# ── Create roms folder ────────────────────────────────────────────────────────
New-Item -ItemType Directory -Force -Path "$OUT_DIR\roms" | Out-Null
@"
Place your legally-obtained ROM zip here.
ROM files are not distributed with Freeplay.
"@ | Out-File "$OUT_DIR\roms\README.txt" -Encoding ASCII

# ── README ────────────────────────────────────────────────────────────────────
@"
freeplay-gametalk v$VERSION
===========================

INSTALL:
  1. Extract this zip anywhere (e.g. C:\Games\Freeplay)
  2. Put your legally-obtained ROM zip in the roms\ folder
  3. Copy .env.example to .env and fill in online service values
  4. Double-click freeplay.exe

MATCHMAKING:
  Click "Find Match" in the main menu. You'll log in with Discord once,
  then automatically pair with any other player also in the queue.
  Login is cached for 24 hours.

MANUAL NETPLAY:
  "Host Match" and "Join Match" still work for direct IP connections.

CONTROLS:
  Configure in the Controls menu. Xbox controllers auto-detect.
"@ | Out-File "$OUT_DIR\README.txt" -Encoding ASCII

# ── Zip it ────────────────────────────────────────────────────────────────────
Write-Host ""
Write-Host "Creating zip..."
if (Test-Path $ZIP_FILE) { Remove-Item $ZIP_FILE }
Compress-Archive -Path "$OUT_DIR\*" -DestinationPath $ZIP_FILE -CompressionLevel Optimal

$SIZE_MB = [math]::Round((Get-Item $ZIP_FILE).Length / 1MB, 2)
Write-Host ""
Write-Host "================================================================" -ForegroundColor Green
Write-Host "✅ Done: $ZIP_FILE ($SIZE_MB MB)" -ForegroundColor Green
Write-Host "================================================================" -ForegroundColor Green
Write-Host ""
Write-Host "Package contents:"
Get-ChildItem -Recurse $OUT_DIR | ForEach-Object {
    $rel = $_.FullName.Substring((Resolve-Path $OUT_DIR).Path.Length + 1)
    if ($_.PSIsContainer) {
        Write-Host "  [DIR]  $rel" -ForegroundColor Cyan
    } else {
        $sz = if ($_.Length -gt 1MB) { "{0:N1} MB" -f ($_.Length / 1MB) }
              elseif ($_.Length -gt 1KB) { "{0:N1} KB" -f ($_.Length / 1KB) }
              else { "$($_.Length) B" }
        Write-Host "         $rel  ($sz)"
    }
}
