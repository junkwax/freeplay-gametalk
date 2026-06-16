fn apply_audio_recovery_ramp(samples: &mut [i16], previous_tail: Option<(i16, i16)>) {
    const RAMP_STEREO_FRAMES: usize = 64;
    let Some((prev_l, prev_r)) = previous_tail else {
        return;
    };
    let frames = (samples.len() / 2).min(RAMP_STEREO_FRAMES);
    if frames == 0 {
        return;
    }
    let denom = frames as i32;
    for frame in 0..frames {
        let idx = frame * 2;
        let weight = (frame + 1) as i32;
        let l = samples[idx] as i32;
        let r = samples[idx + 1] as i32;
        samples[idx] = (prev_l as i32 + ((l - prev_l as i32) * weight) / denom)
            .clamp(i16::MIN as i32, i16::MAX as i32) as i16;
        samples[idx + 1] = (prev_r as i32 + ((r - prev_r as i32) * weight) / denom)
            .clamp(i16::MIN as i32, i16::MAX as i32) as i16;
    }
}

pub(crate) fn prepare_game_audio(
    samples: &mut [i16],
    rollback_recovery: bool,
    audio_tail_sample: &mut Option<(i16, i16)>,
) {
    if rollback_recovery {
        apply_audio_recovery_ramp(samples, *audio_tail_sample);
    }
    if samples.len() >= 2 {
        let last = samples.len() - 2;
        *audio_tail_sample = Some((samples[last], samples[last + 1]));
    }
}

#[cfg(test)]
mod tests {
    use super::{apply_audio_recovery_ramp, prepare_game_audio};

    #[test]
    fn rollback_audio_ramp_interpolates_from_previous_tail() {
        let mut samples = vec![1000, -1000, 1000, -1000, 1000, -1000, 1000, -1000];

        apply_audio_recovery_ramp(&mut samples, Some((0, 0)));

        assert_eq!(samples, vec![250, -250, 500, -500, 750, -750, 1000, -1000]);
    }

    #[test]
    fn prepare_game_audio_only_ramps_rollback_recovery_frames() {
        let mut tail = Some((1000, -1000));
        let mut normal = vec![10, 20, 30, 40];

        prepare_game_audio(&mut normal, false, &mut tail);

        assert_eq!(normal, vec![10, 20, 30, 40]);
        assert_eq!(tail, Some((30, 40)));

        let mut rollback = vec![300, -300, 300, -300];
        prepare_game_audio(&mut rollback, true, &mut tail);

        assert_eq!(rollback, vec![165, -130, 300, -300]);
        assert_eq!(tail, Some((300, -300)));
    }
}
