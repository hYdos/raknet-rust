use std::time::{Duration, Instant};

use bytes::Bytes;
use mcbe_raknet_rs::protocol::ack::{AckNackPayload, SequenceRange};
use mcbe_raknet_rs::protocol::reliability::Reliability;
use mcbe_raknet_rs::session::tunables::SessionTunables;
use mcbe_raknet_rs::session::{QueuePayloadResult, RakPriority, Session};

fn default_tick(session: &mut Session, now: Instant, max_new_datagrams: usize) -> usize {
    session
        .on_tick(now, max_new_datagrams, 64 * 1024, 0, 0)
        .len()
}

#[test]
fn reliable_send_is_throttled_by_congestion_window_until_ack() {
    let tunables = SessionTunables {
        initial_congestion_window: 1.0,
        min_congestion_window: 1.0,
        max_congestion_window: 1.0,
        ..SessionTunables::default()
    };

    let mut session = Session::with_tunables(200, tunables);
    let payload = Bytes::from(vec![0xAA; 150]);

    assert!(matches!(
        session.queue_payload(
            payload.clone(),
            Reliability::ReliableOrdered,
            0,
            RakPriority::High
        ),
        QueuePayloadResult::Enqueued { .. }
    ));
    assert!(matches!(
        session.queue_payload(payload, Reliability::ReliableOrdered, 0, RakPriority::High),
        QueuePayloadResult::Enqueued { .. }
    ));

    let now = Instant::now();
    let first = session.on_tick(now, 2, 64 * 1024, 0, 0);
    assert_eq!(
        first.len(),
        1,
        "cwnd=1 must allow only one reliable datagram"
    );
    let seq = first[0].header.sequence;

    let blocked = default_tick(&mut session, now + Duration::from_millis(1), 2);
    assert_eq!(blocked, 0, "second datagram must wait for ACK");

    session.handle_ack_payload(AckNackPayload {
        ranges: vec![SequenceRange {
            start: seq,
            end: seq,
        }],
    });
    let _ = session.process_incoming_receipts(now + Duration::from_millis(30));

    let after_ack = default_tick(&mut session, now + Duration::from_millis(31), 2);
    assert_eq!(
        after_ack, 1,
        "after ACK, queued reliable datagram must flow"
    );
}

#[test]
fn karn_rule_skips_srtt_update_for_retransmitted_datagram() {
    let mut session = Session::new(200);
    let now = Instant::now();

    assert!(matches!(
        session.queue_payload(
            Bytes::from(vec![0xBB; 150]),
            Reliability::ReliableOrdered,
            0,
            RakPriority::High
        ),
        QueuePayloadResult::Enqueued { .. }
    ));

    let sent = session.on_tick(now, 1, 64 * 1024, 0, 0);
    assert_eq!(sent.len(), 1);
    let seq = sent[0].header.sequence;

    let resent = session.collect_resendable(now + Duration::from_secs(2), 4, usize::MAX);
    assert_eq!(resent.len(), 1, "timeout should produce one resend");

    session.handle_ack_payload(AckNackPayload {
        ranges: vec![SequenceRange {
            start: seq,
            end: seq,
        }],
    });
    let _ =
        session.process_incoming_receipts(now + Duration::from_secs(2) + Duration::from_millis(20));

    let snapshot = session.metrics_snapshot();
    assert_eq!(
        snapshot.srtt_ms, 0.0,
        "Karn rule: retransmitted packet ACK must not sample RTT"
    );
}

#[test]
fn resend_rto_is_clamped_to_configured_max() {
    let tunables = SessionTunables {
        resend_rto: Duration::from_millis(100),
        min_resend_rto: Duration::from_millis(80),
        max_resend_rto: Duration::from_millis(500),
        ..SessionTunables::default()
    };

    let mut session = Session::with_tunables(200, tunables);
    let start = Instant::now();

    assert!(matches!(
        session.queue_payload(
            Bytes::from(vec![0xCC; 150]),
            Reliability::ReliableOrdered,
            0,
            RakPriority::High
        ),
        QueuePayloadResult::Enqueued { .. }
    ));
    let _ = session.on_tick(start, 1, 64 * 1024, 0, 0);

    for step in 1..=6 {
        let now = start + Duration::from_secs(step * 2);
        let resent = session.collect_resendable(now, 8, usize::MAX);
        assert_eq!(resent.len(), 1);
    }

    let snapshot = session.metrics_snapshot();
    assert!(
        snapshot.resend_rto_ms <= 500.0,
        "RTO must stay under configured max: {}",
        snapshot.resend_rto_ms
    );
}

#[test]
fn pending_outgoing_bytes_return_to_zero_after_flush() {
    let mut session = Session::new(200);
    let payload = Bytes::from(vec![0xDD; 150]);

    let _ = session.queue_payload(
        payload.clone(),
        Reliability::ReliableOrdered,
        0,
        RakPriority::High,
    );
    let _ = session.queue_payload(payload, Reliability::ReliableOrdered, 0, RakPriority::High);

    assert!(
        session.pending_outgoing_bytes() > 0,
        "queued bytes must increase after enqueue"
    );

    let sent = session.on_tick(Instant::now(), 8, 64 * 1024, 0, 0);
    assert!(!sent.is_empty());
    assert_eq!(session.pending_outgoing_frames(), 0);
    assert_eq!(session.pending_outgoing_bytes(), 0);
}

#[test]
fn soft_backpressure_defers_low_priority_reliable() {
    let tunables = SessionTunables {
        outgoing_queue_max_frames: 4,
        outgoing_queue_max_bytes: 8 * 1024,
        outgoing_queue_soft_ratio: 0.5,
        ..SessionTunables::default()
    };

    let mut session = Session::with_tunables(200, tunables);

    let _ = session.queue_payload(
        Bytes::from_static(b"a"),
        Reliability::Reliable,
        0,
        RakPriority::High,
    );
    let _ = session.queue_payload(
        Bytes::from_static(b"b"),
        Reliability::Reliable,
        0,
        RakPriority::High,
    );

    assert!(matches!(
        session.queue_payload(
            Bytes::from_static(b"c"),
            Reliability::Reliable,
            0,
            RakPriority::Low
        ),
        QueuePayloadResult::Deferred
    ));

    let snapshot = session.metrics_snapshot();
    assert_eq!(snapshot.outgoing_queue_defers, 1);
}

#[test]
fn nack_for_reliable_datagram_triggers_resend_before_timeout() {
    let mut session = Session::new(200);
    let now = Instant::now();

    assert!(matches!(
        session.queue_payload(
            Bytes::from(vec![0xEE; 150]),
            Reliability::ReliableOrdered,
            0,
            RakPriority::High
        ),
        QueuePayloadResult::Enqueued { .. }
    ));

    let sent = session.on_tick(now, 1, 64 * 1024, 0, 0);
    assert_eq!(sent.len(), 1);
    let seq = sent[0].header.sequence;

    session.handle_nack_payload(AckNackPayload {
        ranges: vec![SequenceRange {
            start: seq,
            end: seq,
        }],
    });
    let progress = session.process_incoming_receipts(now + Duration::from_millis(5));
    assert_eq!(progress.nacked, 1);

    let resent = session.on_tick(now + Duration::from_millis(5), 0, 0, 4, usize::MAX);
    assert!(
        resent.iter().any(|d| d.header.sequence == seq),
        "nack should schedule immediate resend of the same datagram sequence"
    );
}

#[test]
fn unreliable_with_ack_receipt_reports_receipt_completion_on_ack() {
    let mut session = Session::new(200);
    let now = Instant::now();

    assert!(matches!(
        session.queue_payload_with_receipt(
            Bytes::from_static(b"hello"),
            Reliability::UnreliableWithAckReceipt,
            0,
            RakPriority::Normal,
            Some(555)
        ),
        QueuePayloadResult::Enqueued { .. }
    ));

    let sent = session.on_tick(now, 1, 64 * 1024, 0, 0);
    assert_eq!(sent.len(), 1);
    let seq = sent[0].header.sequence;

    session.handle_ack_payload(AckNackPayload {
        ranges: vec![SequenceRange {
            start: seq,
            end: seq,
        }],
    });
    let progress = session.process_incoming_receipts(now + Duration::from_millis(15));
    assert_eq!(progress.acked, 1);
    assert_eq!(progress.acked_receipt_ids, vec![555]);
}
