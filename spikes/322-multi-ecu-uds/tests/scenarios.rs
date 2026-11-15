//! Simulation scenarios for #322. Each test names the ADR rule it demonstrates.
//! Synthetic ids, DIDs and data only.

use spike_322_multi_ecu_uds::controller::*;
use spike_322_multi_ecu_uds::sim::*;
use std::collections::BTreeMap;

const A: Target = Target {
    name: "ecu_a",
    req: "7E0",
    resp: "7E8",
};
const B: Target = Target {
    name: "ecu_b",
    req: "7E2",
    resp: "7EA",
};
const C: Target = Target {
    name: "ecu_c",
    req: "7E3",
    resp: "7EB",
};
const IA: usize = 0;
const IB: usize = 1;
const IC: usize = 2;

fn ecu(t: Target) -> Ecu {
    let mut e = Ecu::new(t.req, t.resp);
    match t.name {
        "ecu_a" => {
            e.dids = vec![
                Did {
                    id: 0xA001,
                    data: vec![1, 2],
                    session_only: false,
                },
                Did {
                    id: 0xA002,
                    data: vec![3, 4],
                    session_only: true,
                },
            ];
            e.pids = vec![(0x0C, vec![0x1A, 0xF8]), (0x0D, vec![0x30])];
        }
        "ecu_b" => {
            e.dids = vec![
                Did {
                    id: 0xB001,
                    data: vec![5],
                    session_only: true,
                },
                Did {
                    id: 0xB002,
                    data: (0..10).collect(),
                    session_only: true,
                },
            ];
        }
        _ => {
            e.dids = vec![Did {
                id: 0xC001,
                data: vec![9],
                session_only: true,
            }]
        }
    }
    e
}

fn fleet(targets: &[Target]) -> Controller {
    let sim = Sim::new(targets.iter().map(|t| ecu(*t)).collect());
    Controller::new(sim, targets.to_vec(), Config::default())
}

fn want(ti: usize, item: Item, prio: u8, session_only: bool, period: Ms, remaining: u32) -> Want {
    Want::new(ti, item, prio, session_only, period, remaining)
}

/// Requests of one wire string, as (time, selected request id), from the adapter log.
fn sent(c: &Controller, wire: &str) -> Vec<(Ms, String)> {
    let mut sh = String::new();
    let mut out = Vec::new();
    for (t, cmd) in &c.sim.log {
        if let Some(h) = cmd.strip_prefix("ATSH ") {
            sh = h.to_string();
        } else if cmd == wire {
            out.push((*t, sh.clone()));
        }
    }
    out
}

fn max_gap(times: &[Ms]) -> Ms {
    times.windows(2).map(|w| w[1] - w[0]).max().unwrap_or(0)
}

fn maintenance_gaps(c: &Controller) -> BTreeMap<String, Ms> {
    let mut by: BTreeMap<String, Vec<Ms>> = BTreeMap::new();
    for (t, h) in sent(c, "3E80") {
        by.entry(h).or_default().push(t);
    }
    by.into_iter().map(|(h, v)| (h, max_gap(&v))).collect()
}

fn count_samples(c: &Controller, ti: usize, item: Item) -> usize {
    c.samples
        .iter()
        .filter(|s| s.ti == ti && s.item == item)
        .count()
}

// ---- interleaving, ownership, keepalive ------------------------------------------------

#[test]
fn two_sessions_interleaved_with_mode01_keep_s3_alive_without_relying_on_reads() {
    let mut c = fleet(&[A, B]);
    c.sim.ecus[IB].any_request_resets_s3 = false; // only 3E 80 keeps B alive
    c.sim.ecus[IA].s3_ms = 3000;
    c.wants = vec![
        want(IA, Item::Did(0xA002), 1, true, 400, 100_000),
        want(IB, Item::Did(0xB001), 1, true, 400, 100_000),
        want(IA, Item::Pid(0x0C), 2, false, 300, 100_000),
    ];
    c.run_until(60_000);
    assert!(c.stopped.is_none(), "{:?}", c.stopped);
    assert!(c.sim.ecus[IA].expiries.is_empty() && c.sim.ecus[IB].expiries.is_empty());
    assert_eq!(c.count_events(|e| matches!(e, Ev::Activated(_))), 2);
    let gaps = maintenance_gaps(&c);
    assert!(gaps["7E0"] <= 3000 && gaps["7E2"] <= 3000, "{gaps:?}");
    assert!(
        count_samples(&c, IA, Item::Did(0xA002)) > 20
            && count_samples(&c, IB, Item::Did(0xB001)) > 20
    );
    // every sample carries the app's view of its target's session state
    assert!(c
        .samples
        .iter()
        .filter(|s| s.item == Item::Did(0xB001))
        .all(|s| matches!(s.assumed, Assumed::Extended { .. })));
}

#[test]
fn header_switches_are_tracked_and_never_redundant() {
    let mut c = fleet(&[A, B]);
    c.wants = vec![
        want(IA, Item::Did(0xA002), 1, true, 500, 20),
        want(IB, Item::Did(0xB001), 1, true, 500, 20),
    ];
    c.run_until(20_000);
    // consecutive requests to the same header never re-send ATSH/ATCRA (except resync after silence)
    let mut last = String::new();
    for (_, cmd) in &c.sim.log {
        if let Some(h) = cmd.strip_prefix("ATSH ") {
            assert_ne!(h, last, "redundant header setup");
            last = h.to_string();
        }
    }
    // every ATSH is followed by the matching ATCRA before any request
    let log: Vec<&str> = c.sim.log.iter().map(|(_, s)| s.as_str()).collect();
    for (i, cmd) in log.iter().enumerate() {
        if cmd.starts_with("ATSH ") {
            assert!(log[i + 1].starts_with("ATCRA "));
        }
    }
    // each value is attributed to exactly the target whose filter was active
    for s in &c.samples {
        assert_eq!(
            s.data,
            if s.item == Item::Did(0xA002) {
                vec![3, 4]
            } else {
                vec![5]
            }
        );
    }
}

#[test]
fn multi_frame_response_is_assembled_for_the_requesting_target_only() {
    let mut c = fleet(&[A, B]);
    c.wants = vec![want(IB, Item::Did(0xB002), 1, true, 500, 3)];
    c.run_until(10_000);
    assert!(c.stopped.is_none(), "{:?}", c.stopped);
    let s: Vec<_> = c
        .samples
        .iter()
        .filter(|s| s.item == Item::Did(0xB002))
        .collect();
    assert_eq!(s.len(), 3);
    assert_eq!(s[0].data, (0..10).collect::<Vec<u8>>());
}

// ---- shared-transport classification --------------------------------------------------

#[test]
fn foreign_responder_through_the_filter_stops_everything_without_further_traffic() {
    let mut c = fleet(&[A, B]);
    c.wants = vec![
        want(IA, Item::Did(0xA002), 1, true, 300, 1000),
        want(IB, Item::Did(0xB001), 1, true, 300, 1000),
    ];
    c.sim.rules.push(Rule::nth(
        "7E0",
        "22A002",
        5,
        Action::Extra("7EA 04 62 B0 01 05".into()),
    ));
    c.run_until(30_000);
    assert!(
        c.stopped.as_deref().unwrap().contains("unattributed frame"),
        "{:?}",
        c.stopped
    );
    let n = c.sim.log.len();
    c.run_until(40_000);
    assert_eq!(c.sim.log.len(), n, "no traffic after stop-all");
    assert!(c
        .sessions
        .iter()
        .all(|s| matches!(s.status, Status::Isolated(_))));
    assert!(c
        .sessions
        .iter()
        .all(|s| !matches!(s.assumed, Assumed::Extended { .. })));
    assert!(
        sent(&c, "1001").is_empty(),
        "no close attempts over an uncertain transport"
    );
}

#[test]
fn stray_frame_without_an_unanswered_request_stops_everything() {
    let mut c = fleet(&[A, B]);
    c.wants = vec![want(IA, Item::Pid(0x0C), 1, false, 300, 1000)];
    c.sim.rules.push(Rule::nth(
        "7E0",
        "010C",
        3,
        Action::Extra("7E8 03 62 A0 01".into()),
    ));
    c.run_until(10_000);
    assert!(c.stopped.is_some());
}

#[test]
fn late_frame_matching_exactly_one_unanswered_request_is_discarded_without_coverage() {
    let mut c = fleet(&[A, B]);
    c.wants = vec![want(IA, Item::Did(0xA001), 1, false, 500, 1000)];
    c.sim
        .rules
        .push(Rule::nth("7E0", "22A001", 2, Action::Late));
    c.run_until(10_000);
    assert!(c.stopped.is_none(), "{:?}", c.stopped);
    assert_eq!(c.late_discarded, 1);
    assert_eq!(c.count_events(|e| matches!(e, Ev::Unanswered { .. })), 1);
    // the late answer is never counted: samples come only from requests that were answered in time
    let total_requests = sent(&c, "22A001").len();
    assert_eq!(count_samples(&c, IA, Item::Did(0xA001)), total_requests - 1);
}

#[test]
fn identical_request_is_not_repeated_while_its_late_answer_could_still_arrive() {
    let mut c = fleet(&[A]);
    c.wants = vec![want(IA, Item::Did(0xA001), 1, false, 100, 1000)];
    c.sim
        .rules
        .push(Rule::nth("7E0", "22A001", 1, Action::Lost));
    c.run_until(5_000);
    let t = sent(&c, "22A001");
    assert!(t[1].0 - t[0].0 > Config::default().late_window_ms, "{t:?}");
}

#[test]
fn truncated_multi_frame_response_stops_everything() {
    let mut c = fleet(&[A, B]);
    c.wants = vec![want(IB, Item::Did(0xB002), 1, true, 500, 100)];
    c.sim
        .rules
        .push(Rule::nth("7E2", "22B002", 2, Action::Truncated));
    c.run_until(20_000);
    assert!(
        c.stopped.as_deref().unwrap().contains("truncated"),
        "{:?}",
        c.stopped
    );
}

#[test]
fn missing_prompt_is_a_transport_failure_and_stops_everything() {
    let mut c = fleet(&[A, B]);
    c.wants = vec![
        want(IA, Item::Did(0xA002), 1, true, 300, 1000),
        want(IB, Item::Did(0xB001), 1, true, 300, 1000),
    ];
    c.sim
        .rules
        .push(Rule::nth("7E2", "22B001", 3, Action::NoPrompt));
    c.run_until(30_000);
    assert!(c.stopped.as_deref().unwrap().starts_with("transport"));
    assert!(c
        .sessions
        .iter()
        .all(|s| s.assumed == Assumed::Default || s.assumed == Assumed::Unconfirmed));
}

#[test]
fn disconnect_marks_every_target_unconfirmed_and_sends_nothing() {
    let mut c = fleet(&[A, B]);
    c.wants = vec![
        want(IA, Item::Did(0xA002), 1, true, 300, 1000),
        want(IB, Item::Did(0xB001), 1, true, 300, 1000),
    ];
    c.run_until(5_000);
    let n = c.sim.log.len();
    c.disconnect();
    c.run_until(10_000);
    c.end_run();
    assert_eq!(c.sim.log.len(), n);
    assert!(c.sessions.iter().all(|s| s.assumed == Assumed::Unconfirmed));
}

// ---- blocking and overdue maintenance --------------------------------------------------

#[test]
fn blocked_read_makes_overdue_session_unconfirmed_and_it_is_reverified_not_assumed() {
    let mut c = fleet(&[A, B]);
    c.sim.ecus[IB].s3_ms = 3000;
    c.wants = vec![
        want(IB, Item::Did(0xB001), 1, true, 400, 100_000),
        want(IA, Item::Pid(0x0C), 2, false, 300, 100_000),
    ];
    c.sim
        .rules
        .push(Rule::nth("7E0", "010C", 20, Action::Block(4500)));
    c.run_until(30_000);
    assert!(c.stopped.is_none(), "{:?}", c.stopped);
    assert_eq!(
        c.sim.ecus[IB].expiries.len(),
        1,
        "the simulated ECU really lost its session"
    );
    let overdue = c
        .events
        .iter()
        .position(|e| *e == Ev::Overdue(IB))
        .expect("overdue");
    let again = c
        .events
        .iter()
        .rposition(|e| *e == Ev::Activated(IB))
        .unwrap();
    assert!(again > overdue, "re-verified by a new 10 03");
    assert_eq!(c.sessions[IB].status, Status::Active);
    // a session-only sample is only ever taken while the session is assumed; never while unconfirmed
    assert!(c
        .samples
        .iter()
        .filter(|s| s.ti == IB)
        .all(|s| matches!(s.assumed, Assumed::Extended { .. })));
}

// ---- NRC table ------------------------------------------------------------------------

#[test]
fn nrc_table() {
    use ActEffect::*;
    use KeepEffect as K;
    use ReadEffect::*;
    for nrc in [0x11, 0x12, 0x13, 0x7E, 0x7F, 0x33, 0x10, 0x99] {
        assert_eq!(activation_effect(nrc), Definitive, "{nrc:02X}");
    }
    for nrc in [0x22, 0x37, 0x92, 0x93] {
        assert_eq!(activation_effect(nrc), Transient, "{nrc:02X}");
    }
    // busyRepeatRequest is not a final refusal: the ECU may have entered the session
    assert_eq!(activation_effect(0x21), Unconfirmed);
    assert_eq!(read_effect(0x31, true), Unsupported);
    assert_eq!(read_effect(0x33, true), Restricted);
    for nrc in [0x21, 0x22, 0x78, 0x92, 0x93] {
        assert_eq!(read_effect(nrc, true), RetryLater);
    }
    assert_eq!(read_effect(0x7F, true), SessionLost);
    assert_eq!(read_effect(0x7E, true), SessionLost);
    // the same codes on a default-session read say nothing about any session
    assert_eq!(read_effect(0x7F, false), Rejected);
    for nrc in [0x10, 0x13, 0x14, 0x24, 0x25, 0x26, 0x99] {
        assert_eq!(read_effect(nrc, true), Rejected);
    }
    for nrc in [0x11, 0x12, 0x13] {
        assert_eq!(maintenance_effect(nrc), K::Refused);
    }
    for nrc in [0x7E, 0x7F, 0x22, 0x99] {
        assert_eq!(maintenance_effect(nrc), K::SessionLost);
    }
}

#[test]
fn ordinary_negative_response_to_one_read_marks_only_that_signal() {
    let mut c = fleet(&[A, B]);
    c.wants = vec![
        want(IB, Item::Did(0xB001), 1, true, 400, 100_000),
        want(IB, Item::Did(0xB002), 1, true, 400, 100_000),
    ];
    c.sim
        .rules
        .push(Rule::always("7E2", "22B002", Action::Nrc(0x22)));
    c.run_until(20_000);
    assert!(c.stopped.is_none());
    assert_eq!(c.sessions[IB].status, Status::Active);
    assert_eq!(c.count_events(|e| matches!(e, Ev::SessionLost(_))), 0);
    assert!(count_samples(&c, IB, Item::Did(0xB001)) > 10);
    assert_eq!(count_samples(&c, IB, Item::Did(0xB002)), 0);
}

#[test]
fn nrc_7f_on_a_session_only_did_is_session_loss_evidence_and_the_session_is_reestablished() {
    let mut c = fleet(&[A, B]);
    c.wants = vec![want(IB, Item::Did(0xB001), 1, true, 400, 100_000)];
    c.sim
        .rules
        .push(Rule::nth("7E2", "22B001", 5, Action::Nrc(0x7F)));
    c.run_until(20_000);
    assert!(c.stopped.is_none());
    assert_eq!(c.count_events(|e| matches!(e, Ev::SessionLost(_))), 1);
    assert_eq!(c.count_events(|e| matches!(e, Ev::Activated(_))), 2);
    assert!(matches!(c.sessions[IB].observed, Observed::Extended(_)));
}

#[test]
fn nrc_31_after_a_spontaneous_session_end_is_decided_by_the_canary_read() {
    let mut c = fleet(&[A, B]);
    c.wants = vec![
        want(IB, Item::Did(0xB001), 1, true, 400, 100_000),
        want(IB, Item::Did(0xB002), 2, true, 800, 100_000),
    ];
    c.run_until(5_000);
    assert_eq!(c.sessions[IB].canary.is_some(), true);
    c.sim.ecus[IB].extended = false; // ECU leaves the session by itself (reset, gateway, ...)
    c.run_until(15_000);
    assert!(c.stopped.is_none());
    assert_eq!(c.count_events(|e| matches!(e, Ev::SessionLost(_))), 1);
    assert_eq!(c.sessions[IB].status, Status::Active);
}

#[test]
fn nrc_31_on_one_did_of_a_live_session_does_not_end_the_session() {
    let mut c = fleet(&[A, B]);
    c.sim.ecus[IB].dids.push(Did {
        id: 0xB003,
        data: vec![1],
        session_only: true,
    });
    c.wants = vec![want(IB, Item::Did(0xB001), 1, true, 400, 100_000)];
    c.run_until(3_000);
    // a DID the ECU does not know: 0x31, but the canary still answers
    c.wants
        .push(want(IB, Item::Did(0xBEEF), 1, true, 400, 100_000));
    c.run_until(15_000);
    assert_eq!(c.count_events(|e| matches!(e, Ev::SessionLost(_))), 0);
    assert_eq!(c.sessions[IB].status, Status::Active);
}

// ---- activation, retry, refusal, unconfirmed return -----------------------------------

#[test]
fn definitive_refusal_is_not_retried_and_other_targets_continue() {
    let mut c = fleet(&[A, B]);
    c.sim
        .rules
        .push(Rule::always("7E2", "1003", Action::Nrc(0x12)));
    c.wants = vec![
        want(IB, Item::Did(0xB001), 1, true, 400, 100),
        want(IA, Item::Did(0xA002), 1, true, 400, 100_000),
    ];
    c.run_until(120_000);
    assert!(c.stopped.is_none());
    assert_eq!(c.sessions[IB].status, Status::Refused(0x12));
    assert_eq!(c.count_events(|e| matches!(e, Ev::Isolated(_, _))), 0);
    assert_eq!(
        sent(&c, "1003").iter().filter(|(_, h)| h == "7E2").count(),
        1
    );
    assert!(count_samples(&c, IA, Item::Did(0xA002)) > 100);
    assert_eq!(count_samples(&c, IB, Item::Did(0xB001)), 0);
}

#[test]
fn transient_refusal_backs_off_2_4_s_then_cools_down_60_s_and_never_gives_up() {
    let mut c = fleet(&[A, B]);
    c.sim
        .rules
        .push(Rule::always("7E2", "1003", Action::Nrc(0x22)));
    c.wants = vec![want(IB, Item::Did(0xB001), 1, true, 400, 100)];
    c.run_until(140_000);
    let t: Vec<Ms> = sent(&c, "1003").into_iter().map(|(t, _)| t).collect();
    assert!(t.len() >= 6, "{t:?}");
    assert!(t[1] - t[0] >= 2000 && t[2] - t[1] >= 4000, "{t:?}");
    assert!(
        t[3] - t[2] >= 60_000,
        "cool-down before the next episode: {t:?}"
    );
    assert!(t[4] - t[3] >= 2000 && t[5] - t[4] >= 4000, "{t:?}");
    assert!(
        t[1] - t[0] < 3000 && t[2] - t[1] < 5000,
        "backoff is bounded: {t:?}"
    );
    assert_eq!(
        sent(&c, "1001").len(),
        0,
        "a final negative answer needs no return to default"
    );
}

// ---- bounded retry after activation success followed by immediate session loss ---------
// Approved criterion (maintainer, #322, 2026-11-15): activation succeeds but the session is
// lost again before any qualifying session read succeeded -> a failed attempt: 2 s, 4 s, then
// 60 s cool-down, repeating per episode. A qualifying session read resets the sequence.

/// Send times of `10 03` to one target.
fn activation_times(c: &Controller, req: &str) -> Vec<Ms> {
    sent(c, "1003")
        .into_iter()
        .filter(|(_, h)| h == req)
        .map(|(t, _)| t)
        .collect()
}

/// Instants at which the controller scheduled the next activation of a target, in order:
/// `Retry.next_at` after a failed attempt, `CoolDown.until` after the third.
fn scheduled_instants(c: &Controller, target: usize) -> Vec<Ms> {
    c.events
        .iter()
        .filter_map(|e| match e {
            Ev::Retry { ti, next_at, .. } if *ti == target => Some(*next_at),
            Ev::CoolDown { ti, until } if *ti == target => Some(*until),
            _ => None,
        })
        .collect()
}

/// Gap i between consecutive activations must be at least 2 s, 4 s, 60 s, 2 s, 4 s, 60 s ...
/// (three failed attempts per episode) and not wastefully longer.
fn assert_backoff_rhythm(t: &[Ms]) {
    println!("10 03 send times (virtual ms): {t:?}");
    const MIN: [Ms; 3] = [2000, 4000, 60_000];
    for (i, w) in t.windows(2).enumerate() {
        let (gap, min) = (w[1] - w[0], MIN[i % 3]);
        assert!(
            gap >= min,
            "premature activation {} after {i}: gap {gap} ms < {min} ms: {t:?}",
            i + 1
        );
        assert!(
            gap < min + 1000,
            "gap {gap} ms is much longer than {min} ms: {t:?}"
        );
    }
}

#[test]
fn repeated_activation_then_immediate_session_loss_is_bounded_by_backoff_and_cooldown() {
    let mut c = fleet(&[A, B]);
    // B accepts every `10 03` but answers every session read with 7F (session lost) before
    // a single session read ever succeeds. A is a healthy bystander.
    c.sim
        .rules
        .push(Rule::always("7E2", "22B001", Action::Nrc(0x7F)));
    c.wants = vec![
        want(IB, Item::Did(0xB001), 1, true, 400, 100_000),
        want(IA, Item::Did(0xA002), 1, true, 400, 100_000),
    ];
    c.run_until(140_000);
    assert!(c.stopped.is_none(), "{:?}", c.stopped);

    let t = activation_times(&c, "7E2");
    assert!(t.len() >= 7, "two full episodes and more expected: {t:?}");
    assert_eq!(c.count_events(|e| *e == Ev::Activated(IB)), t.len());
    assert_backoff_rhythm(&t);

    // every activation was scheduled by the controller at its backoff or cool-down instant
    let due = scheduled_instants(&c, IB);
    println!("scheduled instants (virtual ms): {due:?}");
    for (i, next) in t.iter().skip(1).enumerate() {
        assert!(
            *next >= due[i],
            "activation {} before its scheduled instant: {next} < {}",
            i + 1,
            due[i]
        );
        assert!(
            *next - due[i] < 500,
            "activation {} late against the schedule: {t:?} vs {due:?}",
            i + 1
        );
    }
    let attempts: Vec<u32> = c
        .events
        .iter()
        .filter_map(|e| match e {
            Ev::Retry { ti, attempt, .. } if *ti == IB => Some(*attempt),
            _ => None,
        })
        .collect();
    assert_eq!(&attempts[..4], &[1, 2, 1, 2], "{attempts:?}");
    assert_eq!(
        c.events
            .iter()
            .filter(|e| matches!(e, Ev::CoolDown { ti, .. } if *ti == IB))
            .count(),
        2
    );

    // nothing of this was a qualifying read; no vehicle traffic outside the closed vocabulary
    assert_eq!(count_samples(&c, IB, Item::Did(0xB001)), 0);
    assert!(!c.sessions[IB].qualified);
    assert_eq!(
        sent(&c, "1001").len(),
        0,
        "a lost session needs no return to default"
    );

    // target isolation: the bystander is neither re-activated nor delayed
    assert_eq!(c.count_events(|e| *e == Ev::Activated(IA)), 1);
    assert_eq!(c.count_events(|e| *e == Ev::SessionLost(IA)), 0);
    assert_eq!(c.count_events(|e| matches!(e, Ev::Isolated(_, _))), 0);
    assert!(count_samples(&c, IA, Item::Did(0xA002)) > 100);
    assert!(maintenance_gaps(&c)["7E0"] <= 3000);
    assert!(c.sim.ecus[IA].expiries.is_empty());
}

#[test]
fn successful_default_session_reads_do_not_qualify_and_do_not_reset_the_sequence() {
    let mut c = fleet(&[A, B]);
    c.sim.ecus[IB].dids.push(Did {
        id: 0xB0D0,
        data: vec![7],
        session_only: false,
    });
    c.sim
        .rules
        .push(Rule::always("7E2", "22B001", Action::Nrc(0x7F)));
    c.wants = vec![
        want(IB, Item::Did(0xB0D0), 0, false, 400, 100_000),
        want(IB, Item::Did(0xB001), 1, true, 400, 100_000),
    ];
    c.run_until(80_000);
    assert!(c.stopped.is_none(), "{:?}", c.stopped);
    let t = activation_times(&c, "7E2");
    assert!(t.len() >= 4, "{t:?}");
    assert_backoff_rhythm(&t);
    // the default-session DID answers all along, yet it is not a qualifying session read
    assert!(count_samples(&c, IB, Item::Did(0xB0D0)) > 100);
    assert_eq!(count_samples(&c, IB, Item::Did(0xB001)), 0);
}

#[test]
fn a_qualifying_session_read_resets_the_failure_sequence() {
    let mut c = fleet(&[A, B]);
    // Reads of 22 B001 to B, overall request numbers: 1-2 fail, 3-4 succeed (qualifying),
    // 5 fails after a qualifying read (free, immediate re-activation), 6 fails again before
    // any qualifying read (first failed attempt of a new sequence), 7 onwards succeed.
    // The second rule counts only requests the first one did not consume, hence n = 3.
    c.sim
        .rules
        .push(Rule::nth_times("7E2", "22B001", 1, 2, Action::Nrc(0x7F)));
    c.sim
        .rules
        .push(Rule::nth_times("7E2", "22B001", 3, 2, Action::Nrc(0x7F)));
    c.wants = vec![want(IB, Item::Did(0xB001), 1, true, 400, 100_000)];
    c.run_until(40_000);
    assert!(c.stopped.is_none(), "{:?}", c.stopped);

    let t = activation_times(&c, "7E2");
    assert_eq!(t.len(), 5, "{t:?}");
    println!("10 03 send times (virtual ms): {t:?}");
    assert_eq!(c.count_events(|e| *e == Ev::SessionLost(IB)), 4);
    let attempts: Vec<u32> = c
        .events
        .iter()
        .filter_map(|e| match e {
            Ev::Retry { ti, attempt, .. } if *ti == IB => Some(*attempt),
            _ => None,
        })
        .collect();
    assert_eq!(
        attempts,
        vec![1, 2, 1],
        "sequence restarted after the qualifying reads"
    );
    assert_eq!(c.count_events(|e| matches!(e, Ev::CoolDown { .. })), 0);

    assert!(t[1] - t[0] >= 2000 && t[2] - t[1] >= 4000, "{t:?}");
    assert!(
        t[3] - t[2] < 2000,
        "loss after a qualifying read re-activates at once: {t:?}"
    );
    assert!(
        t[4] - t[3] >= 2000 && t[4] - t[3] < 3000,
        "first failure of the new sequence backs off 2 s, not 4 s or a cool-down: {t:?}"
    );
    let due = scheduled_instants(&c, IB);
    println!("scheduled instants (virtual ms): {due:?}");
    assert!(t[4] >= due[2] && t[4] - due[2] < 500, "{t:?} vs {due:?}");

    assert_eq!(c.sessions[IB].status, Status::Active);
    assert!(c.sessions[IB].qualified);
    assert!(count_samples(&c, IB, Item::Did(0xB001)) > 20);
}

/// Commands that reached one target, in order (session and read vocabulary only).
fn commands_to(c: &Controller, req: &str) -> Vec<String> {
    let mut sh = String::new();
    let mut v = Vec::new();
    for (_, cmd) in &c.sim.log {
        if let Some(h) = cmd.strip_prefix("ATSH ") {
            sh = h.to_string();
        } else if sh == req
            && (cmd == "1003" || cmd == "1001" || cmd == "3E80" || cmd.starts_with("22"))
        {
            v.push(cmd.clone());
        }
    }
    v
}

#[test]
fn busy_repeat_request_on_activation_is_unconfirmed_cleaned_up_and_only_then_backed_off() {
    // NRC 21 is not a final refusal (ADR 0003 fail-closed rule, `has_expected_negative_session_
    // response`): the ECU may still have entered the session. Return to default first (bounded
    // `10 01`); back off only after a positive default return.
    let mut c = fleet(&[A, B]);
    c.sim
        .rules
        .push(Rule::always("7E2", "1003", Action::Nrc(0x21)));
    c.wants = vec![
        want(IB, Item::Did(0xB001), 1, true, 400, 100),
        want(IA, Item::Did(0xA002), 1, true, 400, 100_000),
    ];
    c.run_until(9_000);
    assert!(c.stopped.is_none(), "{:?}", c.stopped);
    let to_b = commands_to(&c, "7E2");
    assert_eq!(
        to_b,
        vec!["1003", "1001", "1003", "1001", "1003", "1001"],
        "each NRC 21 is followed by the cleanup; no read of a session DID is sent"
    );
    // backoff only after the positive return: 2 s, then 4 s, measured from the activation
    let t: Vec<Ms> = sent(&c, "1003")
        .into_iter()
        .filter(|(_, h)| h == "7E2")
        .map(|(t, _)| t)
        .collect();
    assert!(
        t[1] - t[0] >= 2000 && t[1] - t[0] < 3000,
        "first backoff 2 s: {t:?}"
    );
    assert!(
        t[2] - t[1] >= 4000 && t[2] - t[1] < 5000,
        "second backoff 4 s: {t:?}"
    );
    // never recorded as a refusal, never a session; the closed return is observed
    assert_eq!(c.count_events(|e| matches!(e, Ev::Refused(_, _))), 0);
    assert!(matches!(c.sessions[IB].observed, Observed::Default(_)));
    assert_eq!(c.sessions[IB].assumed, Assumed::Default);
    assert!(matches!(c.sessions[IB].status, Status::CoolDown { .. }));
    assert!(count_samples(&c, IA, Item::Did(0xA002)) > 15);
    // the episode keeps its bound: three attempts, then the 60 s cool-down, then a new episode
    c.run_until(100_000);
    assert!(c.stopped.is_none());
    let to_b = commands_to(&c, "7E2");
    let count = |wire: &str| to_b.iter().filter(|x| *x == wire).count();
    assert_eq!(count("1003"), 6, "two episodes of three attempts");
    assert_eq!(count("1001"), 6, "one cleanup per attempt");
}

#[test]
fn busy_repeat_request_on_activation_without_a_positive_return_isolates_only_that_target() {
    let mut c = fleet(&[A, B]);
    c.sim
        .rules
        .push(Rule::always("7E2", "1003", Action::Nrc(0x21)));
    c.sim.rules.push(Rule::always("7E2", "1001", Action::Lost));
    c.wants = vec![
        want(IB, Item::Did(0xB001), 1, true, 400, 100),
        want(IA, Item::Did(0xA002), 1, true, 400, 100_000),
    ];
    c.run_until(60_000);
    assert!(c.stopped.is_none(), "{:?}", c.stopped);
    assert!(matches!(c.sessions[IB].status, Status::Isolated(_)));
    assert_eq!(c.sessions[IB].assumed, Assumed::Unconfirmed);
    assert_eq!(
        commands_to(&c, "7E2"),
        vec!["1003", "1001", "1001"],
        "at most two 10 01, no retry of 10 03, no reads"
    );
    assert_eq!(c.count_events(|e| matches!(e, Ev::Refused(_, _))), 0);
    assert!(count_samples(&c, IA, Item::Did(0xA002)) > 100);
}

#[test]
fn unanswered_activation_is_cleaned_up_and_an_unconfirmed_return_isolates_only_that_target() {
    let mut c = fleet(&[A, B]);
    c.sim
        .rules
        .push(Rule::always("7E2", "1003", Action::ReplyLost));
    c.sim.rules.push(Rule::always("7E2", "1001", Action::Lost));
    c.wants = vec![
        want(IB, Item::Did(0xB001), 1, true, 400, 100),
        want(IA, Item::Did(0xA002), 1, true, 400, 100_000),
    ];
    c.run_until(60_000);
    assert!(c.stopped.is_none(), "{:?}", c.stopped);
    assert!(matches!(c.sessions[IB].status, Status::Isolated(_)));
    assert_eq!(c.sessions[IB].assumed, Assumed::Unconfirmed);
    // bounded cleanup: one 10 03, then exactly two 10 01, then nothing more to that target
    let to_b: Vec<String> = {
        let mut sh = String::new();
        let mut v = Vec::new();
        for (_, cmd) in &c.sim.log {
            if let Some(h) = cmd.strip_prefix("ATSH ") {
                sh = h.to_string();
            } else if sh == "7E2"
                && (cmd == "1003" || cmd == "1001" || cmd == "3E80" || cmd.starts_with("22"))
            {
                v.push(cmd.clone());
            }
        }
        v
    };
    assert_eq!(to_b, vec!["1003", "1001", "1001"]);
    assert!(
        count_samples(&c, IA, Item::Did(0xA002)) > 100,
        "target A keeps measuring"
    );
}

#[test]
fn unconfirmed_return_over_a_failing_transport_stops_everything_instead() {
    let mut c = fleet(&[A, B]);
    c.sim
        .rules
        .push(Rule::always("7E2", "1003", Action::ReplyLost));
    c.sim
        .rules
        .push(Rule::always("7E2", "1001", Action::NoPrompt));
    c.wants = vec![
        want(IB, Item::Did(0xB001), 1, true, 400, 100),
        want(IA, Item::Did(0xA002), 1, true, 400, 100_000),
    ];
    c.run_until(60_000);
    assert!(c.stopped.as_deref().unwrap().starts_with("transport"));
}

#[test]
fn keepalive_refusal_releases_the_ecu_and_marks_it_refused() {
    let mut c = fleet(&[A, B]);
    c.sim
        .rules
        .push(Rule::nth("7E2", "3E80", 2, Action::Nrc(0x12)));
    c.wants = vec![want(IB, Item::Did(0xB001), 1, true, 400, 100_000)];
    c.run_until(20_000);
    assert_eq!(c.sessions[IB].status, Status::Refused(0x12));
    assert_eq!(c.count_events(|e| matches!(e, Ev::Isolated(_, _))), 0);
    assert_eq!(sent(&c, "1001").len(), 1);
}

#[test]
fn two_unanswered_session_reads_in_a_row_trigger_reverification_by_10_03() {
    let mut c = fleet(&[A, B]);
    c.wants = vec![want(IB, Item::Did(0xB001), 1, true, 400, 100_000)];
    c.sim
        .rules
        .push(Rule::nth_times("7E2", "22B001", 4, 2, Action::Lost));
    c.run_until(25_000);
    assert!(c.stopped.is_none(), "{:?}", c.stopped);
    assert_eq!(c.count_events(|e| matches!(e, Ev::SessionLost(_))), 1);
    assert_eq!(c.count_events(|e| matches!(e, Ev::Activated(_))), 2);
}

// ---- fairness, priority, capacity, lifecycle ------------------------------------------

#[test]
fn dpf_raw_request_keeps_priority_while_two_sessions_are_maintained_and_nothing_starves() {
    let mut c = fleet(&[A, B]);
    c.wants = vec![
        want(IA, Item::Did(0xA001), 0, false, 1000, 100_000), // DPF raw request, highest priority
        want(IB, Item::Did(0xB001), 2, true, 500, 100_000),
        want(IA, Item::Pid(0x0D), 3, false, 500, 100_000),
        want(IA, Item::Did(0xA002), 2, true, 500, 100_000),
    ];
    c.run_until(60_000);
    assert!(c.stopped.is_none(), "{:?}", c.stopped);
    let dpf: Vec<Ms> = c
        .samples
        .iter()
        .filter(|s| s.item == Item::Did(0xA001))
        .map(|s| s.at)
        .collect();
    assert!(dpf.len() >= 40, "{}", dpf.len());
    assert!(max_gap(&dpf) <= 2500, "DPF gap {}", max_gap(&dpf));
    for (ti, item) in [
        (IB, Item::Did(0xB001)),
        (IA, Item::Pid(0x0D)),
        (IA, Item::Did(0xA002)),
    ] {
        assert!(count_samples(&c, ti, item) >= 20, "{item:?} starved");
    }
    for (h, g) in maintenance_gaps(&c) {
        assert!(g <= 3500, "{h} gap {g}");
    }
}

#[test]
fn third_session_is_refused_when_measured_maintenance_cost_exceeds_the_budget() {
    let mut c = fleet(&[A, B, C]);
    c.sim.elm_timeout_ms = 400; // slow adapter: a suppressed 3E 80 costs 400 ms
    c.wants = vec![
        want(IA, Item::Did(0xA002), 1, true, 500, 100_000),
        want(IB, Item::Did(0xB001), 1, true, 500, 100_000),
        want(IC, Item::Did(0xC001), 1, true, 500, 100_000),
    ];
    c.run_until(40_000);
    assert!(c.stopped.is_none());
    assert_eq!(c.count_events(|e| matches!(e, Ev::CapacityRefused(IC))), 1);
    assert_eq!(c.sessions[IC].status, Status::Idle);
    assert_eq!(c.active_sessions(), 2);
    assert!(c.sim.ecus.iter().all(|e| e.expiries.is_empty()));
}

#[test]
fn third_session_is_admitted_when_the_measured_cost_is_low() {
    let mut c = fleet(&[A, B, C]);
    c.wants = vec![
        want(IA, Item::Did(0xA002), 1, true, 500, 100_000),
        want(IB, Item::Did(0xB001), 1, true, 500, 100_000),
        want(IC, Item::Did(0xC001), 1, true, 500, 100_000),
    ];
    c.run_until(40_000);
    assert!(c.stopped.is_none());
    assert_eq!(c.active_sessions(), 3);
    assert!(c.sim.ecus.iter().all(|e| e.expiries.is_empty()));
}

#[test]
fn finished_goals_close_only_their_ecu_and_end_of_run_closes_the_rest() {
    let mut c = fleet(&[A, B]);
    c.wants = vec![
        want(IA, Item::Did(0xA002), 1, true, 400, 5),
        want(IB, Item::Did(0xB001), 1, true, 400, 100_000),
    ];
    c.run_until(20_000);
    assert_eq!(c.sessions[IA].status, Status::Idle);
    assert!(matches!(c.sessions[IA].observed, Observed::Default(_)));
    assert_eq!(c.sessions[IB].status, Status::Active);
    assert_eq!(
        sent(&c, "1001").iter().filter(|(_, h)| h == "7E0").count(),
        1
    );
    assert_eq!(
        sent(&c, "1001").iter().filter(|(_, h)| h == "7E2").count(),
        0
    );
    c.end_run();
    assert_eq!(
        sent(&c, "1001").iter().filter(|(_, h)| h == "7E2").count(),
        1
    );
    assert!(c.sessions.iter().all(|s| s.assumed == Assumed::Default));
    // a later need re-establishes it: no one-activation limit
    c.wants.push(want(IA, Item::Did(0xA002), 1, true, 400, 3));
    let now = c.sim.clock;
    c.run_until(now + 10_000);
    assert_eq!(
        sent(&c, "1003").iter().filter(|(_, h)| h == "7E0").count(),
        2
    );
}

// ---- tester present only for due targets, earliest deadline first ---------------------

struct Audit {
    /// (send time, time since the target's previous maintenance send)
    gaps: Vec<(Ms, Ms)>,
    /// another maintained target had an earlier deadline than the one served
    order_violations: usize,
    /// `3E 80` sent while another target was due as well (the order mattered)
    contended: usize,
}

/// Replays the adapter log: for every `3E 80`, the time since the target's previous
/// maintenance send (`10 03` or `3E 80`), and whether another maintained target had an
/// earlier deadline at that moment.
fn tester_present_audit(c: &Controller, horizon: Ms) -> Audit {
    let mut sh = String::new();
    let mut last: BTreeMap<String, Ms> = BTreeMap::new();
    let mut gaps = Vec::new();
    let mut order_violations = 0;
    let mut contended = 0;
    for (t, cmd) in &c.sim.log {
        if let Some(h) = cmd.strip_prefix("ATSH ") {
            sh = h.to_string();
        } else if cmd == "1003" {
            last.insert(sh.clone(), *t);
        } else if cmd == "1001" {
            last.remove(&sh);
        } else if cmd == "3E80" {
            let prev = last.get(&sh).copied().expect("3E 80 without an activation");
            gaps.push((*t, *t - prev));
            // only targets still inside the assumption horizon are maintained at all
            let earlier = last
                .iter()
                .filter(|(h, p)| **h != sh && **p < prev && *t - **p <= horizon)
                .count();
            order_violations += earlier;
            contended += last
                .iter()
                .filter(|(h, p)| **h != sh && *t >= **p + 2000 && *t - **p <= horizon)
                .count();
            last.insert(sh.clone(), *t);
        }
    }
    Audit {
        gaps,
        order_violations,
        contended,
    }
}

#[test]
fn tester_present_is_sent_only_when_a_target_is_due_never_continuously() {
    let mut c = fleet(&[A, B, C]);
    c.wants = vec![
        want(IA, Item::Did(0xA001), 0, false, 100, 100_000),
        want(IA, Item::Pid(0x0C), 2, false, 100, 100_000),
        want(IA, Item::Did(0xA002), 1, true, 100, 100_000),
        want(IB, Item::Did(0xB001), 1, true, 100, 100_000),
        want(IC, Item::Did(0xC001), 1, true, 100, 100_000),
    ];
    c.run_until(60_000);
    assert!(c.stopped.is_none(), "{:?}", c.stopped);
    let gaps = tester_present_audit(&c, 4000).gaps;
    assert!(gaps.len() >= 60, "{} keepalives", gaps.len());
    // REQ-027.2: no more often than every 2 s per target, however busy the reads are
    assert!(
        gaps.iter().all(|(_, g)| *g >= 2000),
        "early tester present: {:?}",
        gaps.iter().filter(|(_, g)| *g < 2000).collect::<Vec<_>>()
    );
    // and not more than one per period and target overall (3 targets, 60 s)
    assert!(
        sent(&c, "3E80").len() <= 3 * 30,
        "{}",
        sent(&c, "3E80").len()
    );
}

#[test]
fn tester_present_goes_to_the_earliest_deadline_first_when_several_are_due() {
    let mut c = fleet(&[A, B, C]);
    // a blocked Mode 01 read makes several targets due at once
    c.sim
        .rules
        .push(Rule::nth("7E0", "010C", 20, Action::Block(2500)));
    c.sim
        .rules
        .push(Rule::nth("7E0", "010C", 40, Action::Block(2500)));
    c.wants = vec![
        want(IA, Item::Pid(0x0C), 0, false, 100, 100_000),
        want(IA, Item::Did(0xA002), 1, true, 100, 100_000),
        want(IB, Item::Did(0xB001), 1, true, 100, 100_000),
        want(IC, Item::Did(0xC001), 1, true, 100, 100_000),
    ];
    c.run_until(60_000);
    assert!(c.stopped.is_none(), "{:?}", c.stopped);
    let audit = tester_present_audit(&c, 4000);
    assert!(audit.gaps.len() >= 60, "{} keepalives", audit.gaps.len());
    assert!(
        audit.contended >= 2,
        "the scenario must make several targets due at once ({})",
        audit.contended
    );
    assert_eq!(
        audit.order_violations, 0,
        "a target with an earlier deadline was served later"
    );
    assert!(audit.gaps.iter().all(|(_, g)| *g >= 2000));
}

#[test]
fn idle_scheduler_waits_for_the_next_deadline_instead_of_polling_the_keepalive() {
    let mut c = fleet(&[A, B]);
    // one slow read per target: between the reads nothing is due except maintenance
    c.wants = vec![
        want(IA, Item::Did(0xA002), 1, true, 10_000, 100_000),
        want(IB, Item::Did(0xB001), 1, true, 10_000, 100_000),
    ];
    c.run_until(40_000);
    assert!(c.stopped.is_none(), "{:?}", c.stopped);
    let audit = tester_present_audit(&c, 4000);
    let gaps = audit.gaps;
    assert_eq!(audit.order_violations, 0);
    assert!(
        gaps.iter().all(|(_, g)| *g >= 2000 && *g <= 2200),
        "{gaps:?}"
    );
    // 2 targets, 40 s, one `3E 80` per 2 s each
    assert!(
        (30..=40).contains(&sent(&c, "3E80").len()),
        "{}",
        sent(&c, "3E80").len()
    );
}

// ---- closed vocabulary ----------------------------------------------------------------

#[test]
fn only_the_closed_vocabulary_ever_reaches_the_adapter() {
    let mut c = fleet(&[A, B, C]);
    c.sim
        .rules
        .push(Rule::nth("7E2", "1003", 1, Action::Nrc(0x22)));
    c.sim
        .rules
        .push(Rule::nth("7E3", "3E80", 2, Action::Nrc(0x7F)));
    c.wants = vec![
        want(IA, Item::Did(0xA001), 0, false, 1000, 100_000),
        want(IA, Item::Pid(0x0C), 2, false, 500, 100_000),
        want(IA, Item::Did(0xA002), 2, true, 500, 100_000),
        want(IB, Item::Did(0xB001), 2, true, 500, 100_000),
        want(IC, Item::Did(0xC001), 2, true, 500, 100_000),
    ];
    c.run_until(60_000);
    c.end_run();
    let known_dids = ["A001", "A002", "B001", "C001"];
    for (_, cmd) in &c.sim.log {
        let ok = matches!(cmd.as_str(), "1003" | "1001" | "3E80" | "010C")
            || cmd
                .strip_prefix("ATSH 7E")
                .is_some_and(|r| matches!(r, "0" | "2" | "3"))
            || cmd
                .strip_prefix("ATCRA 7E")
                .is_some_and(|r| matches!(r, "8" | "A" | "B"))
            || cmd
                .strip_prefix("22")
                .is_some_and(|d| known_dids.contains(&d));
        assert!(ok, "outside the closed vocabulary: {cmd}");
    }
    assert!(c.sim.log.iter().all(|(_, cmd)| !cmd.starts_with("27")
        && !cmd.starts_with("31")
        && !cmd.starts_with("2E")
        && !cmd.starts_with("14")));
}
