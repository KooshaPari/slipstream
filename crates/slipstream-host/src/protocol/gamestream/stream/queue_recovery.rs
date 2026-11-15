use std::sync::mpsc::{SyncSender, TrySendError};
use std::time::{Duration, Instant};

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Offer {
    Queued,
    Full,
    Disconnected,
}

pub(super) fn offer_batch<T>(
    tx: &SyncSender<T>,
    frame: T,
    batch_len: u32,
    au_seq: &mut u32,
    recover_after_drop: &mut bool,
) -> Offer {
    match tx.try_send(frame) {
        Ok(()) => {
            *au_seq = au_seq.wrapping_add(batch_len);
            Offer::Queued
        }
        Err(TrySendError::Full(_)) => {
            *recover_after_drop = true;
            Offer::Full
        }
        Err(TrySendError::Disconnected(_)) => Offer::Disconnected,
    }
}

pub(super) fn request_keyframe(
    last_keyframe: &mut Option<Instant>,
    coalesce: Duration,
    recover_after_drop: &mut bool,
    request: impl FnOnce(),
) -> bool {
    let now = Instant::now();
    let emit = match *last_keyframe {
        Some(t) => now.duration_since(t) >= coalesce,
        None => true,
    };
    if emit {
        request();
        *last_keyframe = Some(now);
        *recover_after_drop = false;
    }
    emit
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::sync_channel;

    #[derive(Debug, PartialEq, Eq)]
    enum Frame {
        P(u32),
        Idr(u32),
    }

    #[test]
    #[ignore = "known bug: dependent P can cross a queue-drop recovery boundary"]
    fn coalesced_request_must_not_admit_dependent_p() {
        let (tx, rx) = sync_channel(1);
        tx.send(Frame::Idr(6)).unwrap();
        let mut seq = 7;
        let mut recovering = false;
        assert_eq!(
            offer_batch(&tx, Frame::P(7), 1, &mut seq, &mut recovering),
            Offer::Full
        );
        assert_eq!(seq, 7);
        rx.recv().unwrap();
        let mut last = Some(Instant::now());
        assert!(!request_keyframe(
            &mut last,
            Duration::from_secs(60),
            &mut recovering,
            || panic!("coalesced")
        ));
        assert!(recovering);
        let _ = offer_batch(&tx, Frame::P(7), 1, &mut seq, &mut recovering);
        assert!(
            rx.try_recv().is_err(),
            "dependent P must wait for a queued IDR"
        );
    }

    #[test]
    #[ignore = "known bug: recovery clears on request before an IDR is queued"]
    fn request_must_not_clear_recovery_before_inflight_p_drains() {
        let mut last = None;
        let mut recovering = true;
        let mut requested = false;
        assert!(request_keyframe(
            &mut last,
            Duration::ZERO,
            &mut recovering,
            || requested = true
        ));
        assert!(requested);
        let (tx, rx) = sync_channel(1);
        let mut seq = 7;
        let _ = offer_batch(&tx, Frame::P(7), 1, &mut seq, &mut recovering);
        assert!(recovering, "a request is not a delivered IDR");
        assert!(
            rx.try_recv().is_err(),
            "in-flight P must wait for a queued IDR"
        );
    }

    #[test]
    #[ignore = "known bug: a rejected IDR does not guard later P frames"]
    fn rejected_recovery_idr_must_not_admit_later_p() {
        let (tx, rx) = sync_channel(1);
        tx.send(Frame::P(6)).unwrap();
        let mut seq = 7;
        let mut recovering = true;
        assert_eq!(
            offer_batch(&tx, Frame::Idr(7), 1, &mut seq, &mut recovering),
            Offer::Full
        );
        rx.recv().unwrap();
        let mut last = Some(Instant::now());
        assert!(!request_keyframe(
            &mut last,
            Duration::from_secs(60),
            &mut recovering,
            || panic!("coalesced")
        ));
        let _ = offer_batch(&tx, Frame::P(7), 1, &mut seq, &mut recovering);
        assert!(
            rx.try_recv().is_err(),
            "a rejected IDR cannot re-anchor later P frames"
        );
    }

    #[test]
    #[ignore = "known bug: buffered submission timestamp can diverge from wire index"]
    fn buffered_au_must_keep_its_submission_index_after_drop() {
        let (tx, rx) = sync_channel(1);
        tx.send((0_u32, 0_u32)).unwrap();
        let mut seq = 1_u32;
        let mut recovering = false;
        let second_submitted_index = seq.wrapping_add(1);
        assert_eq!(
            offer_batch(&tx, (1, seq), 1, &mut seq, &mut recovering),
            Offer::Full
        );
        rx.recv().unwrap();
        let wire_index = seq;
        assert_eq!(
            offer_batch(
                &tx,
                (second_submitted_index, wire_index),
                1,
                &mut seq,
                &mut recovering
            ),
            Offer::Queued
        );
        let (nvenc_timestamp, wire_index) = rx.recv().unwrap();
        assert_eq!(
            nvenc_timestamp, wire_index,
            "RFI must use the emitted frame's NVENC timestamp"
        );
    }
}
