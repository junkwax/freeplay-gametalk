# package.ps1 — Build and zip a distributable freeplay-gametalk client package
# Run from the freeplay project root after cargo build --release

$ErrorActionPreference = "Stop"

# Prefer the latest git tag (stripped of a leading "v") so the package matches
# the released version without editing Cargo.toml — same source of truth the
# binary uses via build.rs. Fall back to Cargo.toml, then "dev".
$VERSION = (git describe --tags --abbrev=0 2>$null)
# A tagless checkout (e.g. CI on a branch push, default shallow fetch) makes
# `git describe` exit non-zero. We intentionally fall back below, but the leaked
# $LASTEXITCODE would otherwise fail the whole pwsh step at its end — reset it.
$global:LASTEXITCODE = 0
if ($VERSION) { $VERSION = $VERSION -replace '^v', '' }
if (-not $VERSION) {
    $VERSION = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"' | ForEach-Object { $_.Matches[0].Groups[1].Value })
}
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
# Prefer the trimmed mk2-subset core (built via tools\build-fbneo-windows.ps1,
# same preference order as render.rs::platform_core_names) and fall back to a
# stock full-driver core so older dev setups still package. Whichever name is
# found is kept as-is in $OUT_DIR — the client's own fallback lookup at
# runtime relies on the real filename being present.
$fbneo_found = (Test-Path "$OUT_DIR\fbneo_mk2_libretro.dll") -or (Test-Path "$OUT_DIR\fbneo_libretro.dll")
if (-not $fbneo_found) {
    $fbneo_candidates = @(
        @{ Path = "cores\fbneo_mk2_libretro.dll"; Name = "fbneo_mk2_libretro.dll" },
        @{ Path = "src\fbneo_mk2_libretro.dll"; Name = "fbneo_mk2_libretro.dll" },
        @{ Path = "fbneo_mk2_libretro.dll"; Name = "fbneo_mk2_libretro.dll" },
        @{ Path = "cores\fbneo_libretro.dll"; Name = "fbneo_libretro.dll" },
        @{ Path = "src\fbneo_libretro.dll"; Name = "fbneo_libretro.dll" },
        @{ Path = "fbneo_libretro.dll"; Name = "fbneo_libretro.dll" },
        @{ Path = "$env:USERPROFILE\AppData\Roaming\RetroArch\cores\fbneo_libretro.dll"; Name = "fbneo_libretro.dll" }
    )
    foreach ($candidate in $fbneo_candidates) {
        if (Test-Path $candidate.Path) {
            Copy-Item $candidate.Path "$OUT_DIR\$($candidate.Name)"
            Write-Host "  ✓ Copied $($candidate.Name) from $($candidate.Path)"
            $fbneo_found = $true
            break
        }
    }
}
if (-not $fbneo_found) {
    Write-Host "  ❌ No FBNeo core found (looked for fbneo_mk2_libretro.dll, then fbneo_libretro.dll). Run: .\tools\build-fbneo-windows.ps1" -ForegroundColor Red
    exit 1
}

# The mk2 subset core is a MinGW/MSYS2 build that dynamically depends on
# libwinpthread-1.dll (build-fbneo-windows.ps1 stages a copy in cores\
# alongside the core for exactly this reason). Without it next to
# freeplay.exe, loading the core silently fails on a machine without MSYS2
# on PATH. A stock MSVC-built fbneo_libretro.dll doesn't need this.
if (-not (Test-Path "$OUT_DIR\libwinpthread-1.dll")) {
    $winpthread_candidates = @("cores\libwinpthread-1.dll", "src\libwinpthread-1.dll", "libwinpthread-1.dll")
    foreach ($path in $winpthread_candidates) {
        if (Test-Path $path) {
            Copy-Item $path "$OUT_DIR\libwinpthread-1.dll"
            Write-Host "  ✓ Copied libwinpthread-1.dll from $path"
            break
        }
    }
}
if ((Test-Path "$OUT_DIR\fbneo_mk2_libretro.dll") -and -not (Test-Path "$OUT_DIR\libwinpthread-1.dll")) {
    Write-Host "  ⚠ libwinpthread-1.dll not found; the mk2 core will fail to load without it. Re-run .\tools\build-fbneo-windows.ps1 to stage it." -ForegroundColor Yellow
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

# Source of values for the bundled .env: prefer local .env (dev/self-host
# overrides) → fall back to .env.public (tracked, public defaults baked in
# so a fresh download just works). CI builds always hit the .env.public
# branch since they have no .env.
$envSource = $null
if (Test-Path ".env") {
    $envSource = ".env"
    Write-Host "  ✓ Using .env for config injection"
} elseif (Test-Path ".env.public") {
    $envSource = ".env.public"
    Write-Host "  ✓ Using .env.public for config injection (no local .env)"
} elseif (Test-Path ".env.example") {
    $envSource = ".env.example"
    Write-Host "  ✓ Using .env.example for config injection (no local .env or .env.public)"
}

if (Test-Path "config.toml") {
    Copy-Item "config.toml" $OUT_DIR
    Write-Host "  ✓ Copied config.toml"
    $cfgPath = "$OUT_DIR\config.toml"
    if (Test-Path $cfgPath) {
        $cfg = Get-Content $cfgPath -Raw

        # Scrub webhook
        $cfg = $cfg -replace '(discord_webhook_url\s*=\s*")[^"]*(")', '${1}https://discord.com/api/webhooks/your-webhook-url-here${2}'

        if ($envSource) {
            $envLines = Get-Content $envSource | Where-Object { $_ -match '^\s*\w+\s*=' -and -not $_.Trim().StartsWith('#') }
            foreach ($line in $envLines) {
                if ($line -match '^\s*FREEPLAY_SIGNALING_URL\s*=\s*(.+)$') {
                    $url = $Matches[1].Trim().Trim('"').Trim("'")
                    if ($cfg -notmatch 'signaling_url\s*=') {
                        $cfg += "`r`nsignaling_url = `"$url`""
                    } else {
                        $cfg = $cfg -replace '(signaling_url\s*=\s*")[^"]*(")', "`${1}$url`${2}"
                    }
                }
                if ($line -match '^\s*FREEPLAY_DISCORD_CLIENT_ID\s*=\s*(.+)$') {
                    $id = $Matches[1].Trim().Trim('"').Trim("'")
                    if ($cfg -notmatch 'discord_client_id\s*=') {
                        $cfg += "`r`ndiscord_client_id = `"$id`""
                    } else {
                        $cfg = $cfg -replace '(discord_client_id\s*=\s*")[^"]*(")', "`${1}$id`${2}"
                    }
                }
            }
            if ($cfg -match 'signaling_url' -or $cfg -match 'discord_client_id') {
                Write-Host "  ✓ Injected env values into config.toml from $envSource"
            }
        }

        Set-Content $cfgPath $cfg -NoNewline
        Write-Host "  ✓ Scrubbed webhook URL from config.toml"
    }
}

# Bundle a `.env` next to freeplay.exe so the binary can read URLs and
# client ID at runtime. Built from .env.public if there's no local .env,
# so a clean download is functional out of the box. Webhook URL is
# always blank in this bundled file — users who want personal Discord
# notifications fill it in themselves.
if ($envSource) {
    $bundled = Get-Content $envSource | Where-Object {
        # Strip webhook URLs and any secret/token/private-key style values.
        # We only ship public runtime defaults.
        $line = $_.Trim()
        if ($line -match '^\s*FREEPLAY_DISCORD_WEBHOOK_URL\s*=' -or
            $line -match '^\s*[A-Z0-9_]*(SECRET|TOKEN|PRIVATE_KEY)\s*=') {
            return $false
        }
        return $true
    }
    # Always end with a blank webhook line so users see where to put one.
    $bundled += "`n# Optional — your own Discord channel webhook for match notifications."
    $bundled += "FREEPLAY_DISCORD_WEBHOOK_URL="
    Set-Content "$OUT_DIR\.env" -Value ($bundled -join "`n") -NoNewline
    Write-Host "  ✓ Bundled .env (public defaults, no secrets)"
}

if (Test-Path ".env.example") {
    Copy-Item ".env.example" $OUT_DIR
    Write-Host "  ✓ Copied .env.example"
}
foreach ($doc in @("LICENSE", "NOTICE.md")) {
    if (Test-Path $doc) {
        Copy-Item $doc $OUT_DIR
        Write-Host "  ✓ Copied $doc"
    }
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

# ── Copy fp_ui assets (fonts + wordmark) ─────────────────────────────────────
# The new UI (config new_ui = true) resolves these next to the exe and loads
# them lazily at its first frame — a package without assets\fonts\ used to
# exit on that first frame, i.e. "doesn't launch". The OFL-*.txt license
# files must ship alongside the TTFs (SIL Open Font License requirement).
if (Test-Path "assets\fonts") {
    New-Item -ItemType Directory -Force -Path "$OUT_DIR\assets\fonts" | Out-Null
    Copy-Item "assets\fonts\*" "$OUT_DIR\assets\fonts\" -Force
    $fontCount = (Get-ChildItem "$OUT_DIR\assets\fonts").Count
    Write-Host "  ✓ Copied assets\fonts ($fontCount files)"
} else {
    Write-Host "  ❌ assets\fonts not found; the new UI (new_ui = true) cannot render without it" -ForegroundColor Red
    exit 1
}
if (Test-Path "assets\logo\wordmark.png") {
    New-Item -ItemType Directory -Force -Path "$OUT_DIR\assets\logo" | Out-Null
    Copy-Item "assets\logo\wordmark.png" "$OUT_DIR\assets\logo\" -Force
    Write-Host "  ✓ Copied assets\logo\wordmark.png"
} else {
    Write-Host "  ⚠ assets\logo\wordmark.png not found — header falls back to text" -ForegroundColor Yellow
}

if (-not (Test-Path "appicon.png")) {
    Write-Host "  ❌ appicon.png not found; packaged builds require a transparent PNG app icon" -ForegroundColor Red
    exit 1
}
Copy-Item "appicon.png" "$OUT_DIR\appicon.png" -Force
Write-Host "  ✓ Copied appicon.png"

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
  3. Run freeplay.exe --doctor-report doctor.txt to verify setup
  4. Double-click freeplay.exe

MATCHMAKING:
  Click "Find Match" in the main menu. You'll log in with Discord once,
  then automatically pair with any other player also in the queue.
  Login is cached for 24 hours.
  Matchmaking works out of the box with the bundled public .env.

MANUAL / LAN NETPLAY:
  Public matches use Find Match. Direct-IP launch is still available for
  testers from the command line with --player/--local/--peer.

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
