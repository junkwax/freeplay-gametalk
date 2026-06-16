use std::fs::File;
use std::io::Write;

pub(crate) fn sync_completed_net_matches(
    net_match_count: &mut u32,
    session_p1_wins: u32,
    session_p2_wins: u32,
) -> bool {
    let completed = session_p1_wins.saturating_add(session_p2_wins);
    if completed > *net_match_count {
        *net_match_count = completed;
        true
    } else {
        false
    }
}

pub(crate) fn log_completed_net_match(
    net_match_count: u32,
    net_log: &mut Option<File>,
    match_limit: u32,
) -> bool {
    let line = format!("[net] Game {net_match_count}/{match_limit} complete.");
    println!("{line}");
    if let Some(f) = net_log.as_mut() {
        let _ = writeln!(f, "{line}");
    }
    net_match_count >= match_limit
}

pub(crate) fn mark_net_set_complete_pending(
    pending_since_frame: &mut Option<u32>,
    net_frame_counter: u32,
    net_log: &mut Option<File>,
    match_limit: u32,
) {
    if pending_since_frame.is_some() {
        return;
    }
    *pending_since_frame = Some(net_frame_counter);
    let line =
        format!("[net] Match limit reached ({match_limit} games); waiting for post-match sequence");
    println!("{line}");
    if let Some(f) = net_log.as_mut() {
        let _ = writeln!(f, "{line}");
    }
}

pub(crate) fn pending_net_set_expired(
    pending_since_frame: Option<u32>,
    net_frame_counter: u32,
    grace_frames: u32,
) -> bool {
    pending_since_frame
        .map(|since| net_frame_counter.saturating_sub(since) >= grace_frames)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::{pending_net_set_expired, sync_completed_net_matches};

    #[test]
    fn completed_net_matches_follow_score_event_totals() {
        let mut completed = 0;

        assert!(sync_completed_net_matches(&mut completed, 1, 0));
        assert_eq!(completed, 1);

        assert!(!sync_completed_net_matches(&mut completed, 1, 0));
        assert_eq!(completed, 1);

        assert!(sync_completed_net_matches(&mut completed, 1, 2));
        assert_eq!(completed, 3);
    }

    #[test]
    fn pending_net_set_waits_for_grace_frames() {
        assert!(!pending_net_set_expired(None, 100, 30));
        assert!(!pending_net_set_expired(Some(100), 129, 30));
        assert!(pending_net_set_expired(Some(100), 130, 30));
        assert!(!pending_net_set_expired(Some(200), 100, 30));
    }
}
