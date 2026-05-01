use std::path::Path;

pub fn run() -> i32 {
    let mut ok = true;

    println!("freeplay-gametalk doctor");
    println!("version: {}", crate::version::footer_string());
    println!();

    check(
        "config.toml",
        Path::new("config.toml").exists(),
        "optional local config",
        &mut ok,
        false,
    );
    check(
        ".env",
        Path::new(".env").exists(),
        "local private env file",
        &mut ok,
        false,
    );
    check(
        ".env.example",
        Path::new(".env.example").exists(),
        "sample env file",
        &mut ok,
        true,
    );

    let signaling = crate::config::signaling_url()
        .or_else(|| crate::config::env_value("FREEPLAY_SIGNALING_URL"));
    let stats = crate::config::env_value("FREEPLAY_STATS_URL");
    let discord = crate::config::env_value("FREEPLAY_DISCORD_CLIENT_ID");
    check(
        "FREEPLAY_SIGNALING_URL",
        signaling.is_some(),
        "required for online matchmaking",
        &mut ok,
        true,
    );
    check(
        "FREEPLAY_STATS_URL",
        stats.is_some(),
        "required for profile/ghost stats",
        &mut ok,
        false,
    );
    check(
        "FREEPLAY_DISCORD_CLIENT_ID",
        discord.is_some(),
        "required for Discord presence/login",
        &mut ok,
        true,
    );

    let rom = crate::rom::find_rom_zip();
    check_path(
        "ROM zip",
        rom.as_deref(),
        "required to launch gameplay",
        &mut ok,
        true,
    );

    let core = find_core();
    check_path(
        "FBNeo core",
        core.as_deref(),
        "required to launch gameplay",
        &mut ok,
        true,
    );

    let scoreboard_font = first_existing(&["media/mk2.ttf", "src/media/mk2.ttf", "mk2.ttf"]);
    check_path(
        "mk2 scoreboard font",
        scoreboard_font.as_deref(),
        "optional; falls back if missing",
        &mut ok,
        false,
    );

    println!();
    if ok {
        println!("doctor: OK");
        0
    } else {
        println!("doctor: missing required items");
        1
    }
}

fn check(name: &str, present: bool, note: &str, ok: &mut bool, required: bool) {
    let status = if present {
        "OK"
    } else if required {
        "MISSING"
    } else {
        "SKIP"
    };
    println!("{status:7} {name} - {note}");
    if required && !present {
        *ok = false;
    }
}

fn check_path(name: &str, path: Option<&Path>, note: &str, ok: &mut bool, required: bool) {
    match path {
        Some(p) => println!("OK      {name} - {}", p.display()),
        None => check(name, false, note, ok, required),
    }
}

fn find_core() -> Option<std::path::PathBuf> {
    let core = platform_core_name();
    let in_cores = format!("cores/{core}");
    let mut candidates = vec![core.to_string(), in_cores];
    if let Some(exe_dir) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
    {
        candidates.push(exe_dir.join(core).to_string_lossy().into_owned());
        candidates.push(
            exe_dir
                .join("cores")
                .join(core)
                .to_string_lossy()
                .into_owned(),
        );
    }
    let refs: Vec<&str> = candidates.iter().map(String::as_str).collect();
    first_existing(&refs)
}

fn first_existing(paths: &[&str]) -> Option<std::path::PathBuf> {
    paths.iter().find_map(|p| path_if_exists(p))
}

fn path_if_exists(path: &str) -> Option<std::path::PathBuf> {
    let path = Path::new(path);
    if path.exists() {
        Some(path.to_path_buf())
    } else {
        None
    }
}

fn platform_core_name() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "fbneo_libretro.dll"
    }
    #[cfg(target_os = "linux")]
    {
        "fbneo_libretro.so"
    }
    #[cfg(target_os = "macos")]
    {
        "fbneo_libretro.dylib"
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    {
        "fbneo_libretro"
    }
}
