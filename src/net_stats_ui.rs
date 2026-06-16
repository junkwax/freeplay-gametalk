use crate::{mk2_perf, netcore, retro};

const MK2_PERF_SAMPLE_INTERVAL_FRAMES: u32 = 30;

fn net_stats_detail_rows(
    rollback_frames: u32,
    save_count: u32,
    load_count: u32,
    save_state_micros: u64,
    checksum_micros: u64,
    load_state_micros: u64,
    kbps_sent: Option<&str>,
    local_frames_behind: Option<&str>,
    remote_frames_behind: Option<&str>,
    ping_ms: Option<i32>,
    mk2_perf: Option<mk2_perf::Mk2PerfSample>,
) -> Vec<String> {
    let mut rows = Vec::new();
    if let Some(ms) = ping_ms {
        let quality = if ms <= 80 {
            "GOOD"
        } else if ms <= 140 {
            "OK"
        } else {
            "HIGH"
        };
        rows.push(format!("QUALITY {quality}"));
    }
    rows.push(format!("ROLL {}F", rollback_frames));
    if save_count > 0 {
        rows.push(format!("SAVES {save_count}"));
    }
    if load_count > 0 {
        rows.push(format!("LOADS {load_count}"));
    }
    if save_state_micros > 0 || checksum_micros > 0 || load_state_micros > 0 {
        rows.push(format!(
            "CPU S{} H{} L{} US",
            save_state_micros, checksum_micros, load_state_micros
        ));
    }
    if let (Some(local), Some(remote)) = (local_frames_behind, remote_frames_behind) {
        rows.push(format!("BEHIND L{local} R{remote}"));
    }
    if let Some(kbps) = kbps_sent {
        rows.push(format!("SEND {kbps} KB/S"));
    }
    if let Some(perf) = mk2_perf {
        rows.extend(perf.detail_rows());
    }
    rows
}

#[derive(Default)]
pub(crate) struct NetStatsUi {
    pub(crate) next_network_sample_frame: u32,
    mk2_perf_cache: Option<mk2_perf::Mk2PerfSample>,
    mk2_perf_next_sample_frame: u32,
    pub(crate) ping_ms: Option<i32>,
    pub(crate) kbps_sent: Option<String>,
    pub(crate) local_frames_behind: Option<String>,
    pub(crate) remote_frames_behind: Option<String>,
    pub(crate) rollback_frames: u32,
    pub(crate) save_count: u32,
    pub(crate) load_count: u32,
    pub(crate) save_state_micros: u64,
    pub(crate) checksum_micros: u64,
    pub(crate) load_state_micros: u64,
}

impl NetStatsUi {
    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }

    pub(crate) fn on_overlay_toggle(&mut self, visible: bool) {
        if visible {
            self.mk2_perf_next_sample_frame = 0;
        } else {
            self.mk2_perf_cache = None;
        }
    }

    pub(crate) fn record_step(&mut self, step_stats: netcore::NetStepStats) {
        self.rollback_frames = step_stats.advance_count.saturating_sub(1) as u32;
        self.save_count = step_stats.save_count as u32;
        self.load_count = step_stats.load_count as u32;
        self.save_state_micros = step_stats.save_state_micros;
        self.checksum_micros = step_stats.checksum_micros;
        self.load_state_micros = step_stats.load_state_micros;
    }

    pub(crate) fn sample_mk2_perf(
        &mut self,
        core: Option<&retro::Core>,
        net_frame_counter: u32,
    ) -> Option<mk2_perf::Mk2PerfSample> {
        let Some(core) = core else {
            return None;
        };
        if self.mk2_perf_cache.is_none() || net_frame_counter >= self.mk2_perf_next_sample_frame {
            self.mk2_perf_cache = mk2_perf::sample(core);
            self.mk2_perf_next_sample_frame =
                net_frame_counter.wrapping_add(MK2_PERF_SAMPLE_INTERVAL_FRAMES);
        }
        self.mk2_perf_cache
    }

    pub(crate) fn ping_label(&self) -> Option<String> {
        self.ping_ms.map(|ms| format!("{ms} ms"))
    }

    pub(crate) fn detail_rows(&self, mk2_perf: Option<mk2_perf::Mk2PerfSample>) -> Vec<String> {
        net_stats_detail_rows(
            self.rollback_frames,
            self.save_count,
            self.load_count,
            self.save_state_micros,
            self.checksum_micros,
            self.load_state_micros,
            self.kbps_sent.as_deref(),
            self.local_frames_behind.as_deref(),
            self.remote_frames_behind.as_deref(),
            self.ping_ms,
            mk2_perf,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::NetStatsUi;
    use crate::netcore::NetStepStats;

    #[test]
    fn net_stats_ui_records_step_and_formats_rows() {
        let mut stats = NetStatsUi {
            ping_ms: Some(72),
            kbps_sent: Some("12.5".into()),
            local_frames_behind: Some("1".into()),
            remote_frames_behind: Some("2".into()),
            ..NetStatsUi::default()
        };

        stats.record_step(NetStepStats {
            advance_count: 3,
            save_count: 2,
            load_count: 1,
            save_state_micros: 40,
            checksum_micros: 9,
            load_state_micros: 17,
            ..NetStepStats::default()
        });

        assert_eq!(stats.rollback_frames, 2);
        assert_eq!(stats.ping_label().as_deref(), Some("72 ms"));
        assert_eq!(
            stats.detail_rows(None),
            vec![
                "QUALITY GOOD",
                "ROLL 2F",
                "SAVES 2",
                "LOADS 1",
                "CPU S40 H9 L17 US",
                "BEHIND L1 R2",
                "SEND 12.5 KB/S",
            ]
        );
    }

    #[test]
    fn net_stats_ui_reset_clears_cached_overlay_state() {
        let mut stats = NetStatsUi {
            next_network_sample_frame: 275,
            mk2_perf_cache: Some(Default::default()),
            mk2_perf_next_sample_frame: 30,
            ping_ms: Some(140),
            rollback_frames: 4,
            save_count: 1,
            ..NetStatsUi::default()
        };

        stats.on_overlay_toggle(false);
        assert!(stats.mk2_perf_cache.is_none());
        assert_eq!(stats.mk2_perf_next_sample_frame, 30);

        stats.on_overlay_toggle(true);
        assert_eq!(stats.mk2_perf_next_sample_frame, 0);

        stats.reset();
        assert_eq!(stats.next_network_sample_frame, 0);
        assert!(stats.ping_ms.is_none());
        assert_eq!(stats.rollback_frames, 0);
        assert!(stats.mk2_perf_cache.is_none());
    }
}
