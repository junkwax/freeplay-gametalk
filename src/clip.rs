//! Short gameplay clip capture for Discord sharing.
//!
//! Captures raw emulator frames and audio while the user records, then asks
//! ffmpeg to encode an MP4. If ffmpeg is missing, the raw capture is left on
//! disk so the clip is not lost.

use crate::retro::{
    FRAME_BUFFER, FRAME_HEIGHT, FRAME_PITCH, FRAME_WIDTH, PIXEL_FORMAT, RETRO_PIXEL_FORMAT_RGB565,
    RETRO_PIXEL_FORMAT_XRGB8888,
};
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const CLIP_DIR: &str = "clips";
const CLIP_FPS: u32 = 55;
const MAX_SECONDS: u32 = 20;

pub struct ClipRecorder {
    raw_dir: PathBuf,
    output_path: PathBuf,
    frame_count: u32,
    audio: Vec<i16>,
    sample_rate: u32,
}

pub struct ClipResult {
    pub message: String,
}

impl ClipRecorder {
    pub fn start(sample_rate: u32) -> Result<Self, String> {
        let ts = timestamp();
        let root = PathBuf::from(CLIP_DIR);
        let raw_dir = root.join(format!("raw_{ts}"));
        fs::create_dir_all(&raw_dir).map_err(|e| format!("create {}: {e}", raw_dir.display()))?;
        Ok(Self {
            raw_dir,
            output_path: root.join(format!("freeplay_{ts}.mp4")),
            frame_count: 0,
            audio: Vec::new(),
            sample_rate: sample_rate.max(1),
        })
    }

    pub fn elapsed_seconds(&self) -> u32 {
        self.frame_count / CLIP_FPS
    }

    pub fn is_at_limit(&self) -> bool {
        self.frame_count >= CLIP_FPS * MAX_SECONDS
    }

    pub fn record_audio(&mut self, samples: &[i16]) {
        self.audio.extend_from_slice(samples);
    }

    #[allow(static_mut_refs)]
    pub fn record_frame(&mut self) -> Result<(), String> {
        unsafe {
            if FRAME_WIDTH == 0 || FRAME_HEIGHT == 0 || FRAME_BUFFER.is_empty() {
                return Ok(());
            }
            let path = self
                .raw_dir
                .join(format!("frame_{:06}.ppm", self.frame_count));
            let mut file = BufWriter::new(
                File::create(&path).map_err(|e| format!("create {}: {e}", path.display()))?,
            );
            write!(file, "P6\n{} {}\n255\n", FRAME_WIDTH, FRAME_HEIGHT)
                .map_err(|e| e.to_string())?;
            for y in 0..FRAME_HEIGHT as usize {
                let row = y * FRAME_PITCH;
                for x in 0..FRAME_WIDTH as usize {
                    let rgb = pixel_rgb(row, x);
                    file.write_all(&rgb).map_err(|e| e.to_string())?;
                }
            }
            self.frame_count = self.frame_count.saturating_add(1);
        }
        Ok(())
    }

    pub fn finish(self) -> Result<ClipResult, String> {
        if self.frame_count == 0 {
            let _ = fs::remove_dir_all(&self.raw_dir);
            return Ok(ClipResult {
                message: "Clip discarded: no frames recorded".into(),
            });
        }

        let audio_path = self.raw_dir.join("audio.wav");
        write_wav(&audio_path, self.sample_rate, &self.audio)?;

        let Some(ffmpeg) = find_ffmpeg() else {
            return Ok(ClipResult {
                message: format!(
                    "ffmpeg not found; raw clip kept: {}",
                    self.raw_dir.display()
                ),
            });
        };

        let input_pattern = self.raw_dir.join("frame_%06d.ppm");
        let status = Command::new(&ffmpeg)
            .args([
                "-y",
                "-hide_banner",
                "-loglevel",
                "error",
                "-framerate",
                &CLIP_FPS.to_string(),
                "-i",
                &input_pattern.to_string_lossy(),
                "-i",
                &audio_path.to_string_lossy(),
                "-shortest",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
                "-b:a",
                "160k",
                &self.output_path.to_string_lossy(),
            ])
            .status();

        match status {
            Ok(s) if s.success() => {
                let _ = fs::remove_dir_all(&self.raw_dir);
                Ok(ClipResult {
                    message: format!("Clip saved: {}", self.output_path.display()),
                })
            }
            Ok(s) => Ok(ClipResult {
                message: format!(
                    "ffmpeg failed ({s}); raw clip kept: {}",
                    self.raw_dir.display()
                ),
            }),
            Err(e) => Ok(ClipResult {
                message: format!(
                    "ffmpeg failed to launch ({e}); raw clip kept: {}",
                    self.raw_dir.display()
                ),
            }),
        }
    }
}

pub(crate) fn find_ffmpeg() -> Option<PathBuf> {
    if let Some(path) = crate::config::env_value("FREEPLAY_FFMPEG").map(PathBuf::from) {
        if path.exists() {
            return Some(path);
        }
    }

    let mut candidates = vec![
        PathBuf::from("ffmpeg"),
        PathBuf::from("ffmpeg.exe"),
        PathBuf::from("tools/ffmpeg/ffmpeg.exe"),
        PathBuf::from("tools/ffmpeg/bin/ffmpeg.exe"),
    ];

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("ffmpeg.exe"));
            candidates.push(dir.join("ffmpeg").join("ffmpeg.exe"));
            candidates.push(dir.join("ffmpeg").join("bin").join("ffmpeg.exe"));
        }
    }

    candidates.into_iter().find(|path| {
        if path.components().count() > 1 || path.extension().is_some() {
            path.exists()
        } else {
            Command::new(path)
                .arg("-version")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        }
    })
}

#[allow(static_mut_refs)]
unsafe fn pixel_rgb(row: usize, x: usize) -> [u8; 3] {
    match PIXEL_FORMAT {
        RETRO_PIXEL_FORMAT_XRGB8888 => {
            let i = row + x * 4;
            let px = u32::from_ne_bytes([
                FRAME_BUFFER[i],
                FRAME_BUFFER[i + 1],
                FRAME_BUFFER[i + 2],
                FRAME_BUFFER[i + 3],
            ]);
            [
                ((px >> 16) & 0xff) as u8,
                ((px >> 8) & 0xff) as u8,
                (px & 0xff) as u8,
            ]
        }
        RETRO_PIXEL_FORMAT_RGB565 => {
            let i = row + x * 2;
            let px = u16::from_ne_bytes([FRAME_BUFFER[i], FRAME_BUFFER[i + 1]]);
            let r = ((px >> 11) & 0x1f) as u8;
            let g = ((px >> 5) & 0x3f) as u8;
            let b = (px & 0x1f) as u8;
            [
                (r << 3) | (r >> 2),
                (g << 2) | (g >> 4),
                (b << 3) | (b >> 2),
            ]
        }
        _ => {
            let i = row + x * 2;
            let px = u16::from_ne_bytes([FRAME_BUFFER[i], FRAME_BUFFER[i + 1]]);
            let r = ((px >> 10) & 0x1f) as u8;
            let g = ((px >> 5) & 0x1f) as u8;
            let b = (px & 0x1f) as u8;
            [
                (r << 3) | (r >> 2),
                (g << 3) | (g >> 2),
                (b << 3) | (b >> 2),
            ]
        }
    }
}

fn write_wav(path: &PathBuf, sample_rate: u32, samples: &[i16]) -> Result<(), String> {
    let mut file =
        BufWriter::new(File::create(path).map_err(|e| format!("create {}: {e}", path.display()))?);
    let data_bytes = (samples.len() * 2) as u32;
    let riff_size = 36u32.saturating_add(data_bytes);
    file.write_all(b"RIFF").map_err(|e| e.to_string())?;
    file.write_all(&riff_size.to_le_bytes())
        .map_err(|e| e.to_string())?;
    file.write_all(b"WAVEfmt ").map_err(|e| e.to_string())?;
    file.write_all(&16u32.to_le_bytes())
        .map_err(|e| e.to_string())?;
    file.write_all(&1u16.to_le_bytes())
        .map_err(|e| e.to_string())?;
    file.write_all(&2u16.to_le_bytes())
        .map_err(|e| e.to_string())?;
    file.write_all(&sample_rate.to_le_bytes())
        .map_err(|e| e.to_string())?;
    let byte_rate = sample_rate * 2 * 2;
    file.write_all(&byte_rate.to_le_bytes())
        .map_err(|e| e.to_string())?;
    file.write_all(&4u16.to_le_bytes())
        .map_err(|e| e.to_string())?;
    file.write_all(&16u16.to_le_bytes())
        .map_err(|e| e.to_string())?;
    file.write_all(b"data").map_err(|e| e.to_string())?;
    file.write_all(&data_bytes.to_le_bytes())
        .map_err(|e| e.to_string())?;
    for sample in samples {
        file.write_all(&sample.to_le_bytes())
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
