use std::path::Path;

pub fn run() -> i32 {
    let report = build_report();
    for line in &report.lines {
        println!("{line}");
    }
    if report.ok {
        0
    } else {
        1
    }
}

pub fn run_report(path: &Path) -> i32 {
    let report = build_report();
    let body = report.lines.join("\r\n");
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        if let Err(e) = std::fs::create_dir_all(parent) {
            println!(
                "doctor: failed to create report directory {}: {e}",
                parent.display()
            );
            return 1;
        }
    }
    match std::fs::write(path, format!("{body}\r\n")) {
        Ok(()) => println!("doctor: wrote report {}", path.display()),
        Err(e) => {
            println!("doctor: failed to write report {}: {e}", path.display());
            return 1;
        }
    }
    if report.ok {
        0
    } else {
        1
    }
}

struct DoctorReport {
    lines: Vec<String>,
    ok: bool,
}

struct DoctorInputs {
    version: String,
    config_toml: bool,
    env_file: bool,
    env_example: bool,
    signaling: bool,
    stats: bool,
    discord_client_id: bool,
    rom: Option<std::path::PathBuf>,
    core: Option<std::path::PathBuf>,
    scoreboard_font: Option<std::path::PathBuf>,
    ffmpeg: Option<std::path::PathBuf>,
}

impl DoctorInputs {
    fn probe() -> Self {
        let signaling = crate::config::signaling_url()
            .or_else(|| crate::config::env_value("FREEPLAY_SIGNALING_URL"));
        Self {
            version: crate::version::footer_string(),
            config_toml: Path::new("config.toml").exists(),
            env_file: Path::new(".env").exists(),
            env_example: Path::new(".env.example").exists(),
            signaling: signaling.is_some(),
            stats: crate::config::env_value("FREEPLAY_STATS_URL").is_some(),
            discord_client_id: crate::config::env_value("FREEPLAY_DISCORD_CLIENT_ID").is_some(),
            rom: crate::rom::find_rom_zip(),
            core: find_core(),
            scoreboard_font: first_existing(&["media/mk2.ttf", "src/media/mk2.ttf", "mk2.ttf"]),
            ffmpeg: crate::clip::find_ffmpeg(),
        }
    }
}

fn build_report() -> DoctorReport {
    build_report_from(DoctorInputs::probe())
}

fn build_report_from(input: DoctorInputs) -> DoctorReport {
    let mut lines = Vec::new();
    let mut ok = true;

    lines.push("freeplay-gametalk doctor".into());
    lines.push(format!("version: {}", input.version));
    lines.push(String::new());

    check(
        &mut lines,
        "config.toml",
        input.config_toml,
        "optional local config",
        &mut ok,
        false,
    );
    check(
        &mut lines,
        ".env",
        input.env_file,
        "local private env file",
        &mut ok,
        false,
    );
    check(
        &mut lines,
        ".env.example",
        input.env_example,
        "sample env file",
        &mut ok,
        true,
    );

    check(
        &mut lines,
        "FREEPLAY_SIGNALING_URL",
        input.signaling,
        "required for online matchmaking",
        &mut ok,
        true,
    );
    check(
        &mut lines,
        "FREEPLAY_STATS_URL",
        input.stats,
        "required for profile/ghost stats",
        &mut ok,
        false,
    );
    check(
        &mut lines,
        "FREEPLAY_DISCORD_CLIENT_ID",
        input.discord_client_id,
        "optional for Discord Rich Presence",
        &mut ok,
        false,
    );

    check_path(
        &mut lines,
        "ROM zip",
        input.rom.as_deref(),
        "required to launch gameplay",
        &mut ok,
        true,
    );

    check_path(
        &mut lines,
        "FBNeo core",
        input.core.as_deref(),
        "required to launch gameplay",
        &mut ok,
        true,
    );

    check_path(
        &mut lines,
        "mk2 scoreboard font",
        input.scoreboard_font.as_deref(),
        "optional; falls back if missing",
        &mut ok,
        false,
    );

    check_path(
        &mut lines,
        "ffmpeg clip encoder",
        input.ffmpeg.as_deref(),
        "optional; required for Ctrl+R MP4 clips",
        &mut ok,
        false,
    );

    lines.push(String::new());
    if ok {
        lines.push("doctor: OK".into());
    } else {
        lines.push("doctor: missing required items".into());
    }
    DoctorReport { lines, ok }
}

fn check(
    lines: &mut Vec<String>,
    name: &str,
    present: bool,
    note: &str,
    ok: &mut bool,
    required: bool,
) {
    let status = if present {
        "OK"
    } else if required {
        "MISSING"
    } else {
        "SKIP"
    };
    lines.push(format!("{status:7} {name} - {note}"));
    if required && !present {
        *ok = false;
    }
}

fn check_path(
    lines: &mut Vec<String>,
    name: &str,
    path: Option<&Path>,
    note: &str,
    ok: &mut bool,
    required: bool,
) {
    match path {
        Some(p) => lines.push(format!("OK      {name} - {}", p.display())),
        None => check(lines, name, false, note, ok, required),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn full_inputs() -> DoctorInputs {
        DoctorInputs {
            version: "test-version".into(),
            config_toml: true,
            env_file: true,
            env_example: true,
            signaling: true,
            stats: true,
            discord_client_id: true,
            rom: Some("mk2.zip".into()),
            core: Some(platform_core_name().into()),
            scoreboard_font: Some("media/mk2.ttf".into()),
            ffmpeg: Some("ffmpeg".into()),
        }
    }

    fn has_line(report: &DoctorReport, needle: &str) -> bool {
        report.lines.iter().any(|line| line.contains(needle))
    }

    #[test]
    fn doctor_report_is_ok_when_required_items_present() {
        let report = build_report_from(full_inputs());

        assert!(report.ok);
        assert!(has_line(&report, "version: test-version"));
        assert!(has_line(&report, "OK      ROM zip - mk2.zip"));
        assert!(has_line(
            &report,
            &format!("OK      FBNeo core - {}", platform_core_name())
        ));
        assert!(has_line(&report, "doctor: OK"));
    }

    #[test]
    fn doctor_report_marks_missing_required_items() {
        let mut input = full_inputs();
        input.env_example = false;
        input.signaling = false;
        input.rom = None;
        input.core = None;

        let report = build_report_from(input);

        assert!(!report.ok);
        assert!(has_line(&report, "MISSING .env.example"));
        assert!(has_line(&report, "MISSING FREEPLAY_SIGNALING_URL"));
        assert!(has_line(&report, "MISSING ROM zip"));
        assert!(has_line(&report, "MISSING FBNeo core"));
        assert!(has_line(&report, "doctor: missing required items"));
    }

    #[test]
    fn doctor_report_allows_missing_optional_items() {
        let mut input = full_inputs();
        input.config_toml = false;
        input.env_file = false;
        input.stats = false;
        input.discord_client_id = false;
        input.scoreboard_font = None;
        input.ffmpeg = None;

        let report = build_report_from(input);

        assert!(report.ok);
        assert!(has_line(&report, "SKIP    config.toml"));
        assert!(has_line(&report, "SKIP    .env"));
        assert!(has_line(&report, "SKIP    FREEPLAY_STATS_URL"));
        assert!(has_line(&report, "SKIP    FREEPLAY_DISCORD_CLIENT_ID"));
        assert!(has_line(&report, "SKIP    mk2 scoreboard font"));
        assert!(has_line(&report, "SKIP    ffmpeg clip encoder"));
        assert!(has_line(&report, "doctor: OK"));
    }
}
