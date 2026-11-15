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
    for nrc in [0x21, 0x22, 0x37, 0x92, 0x93] {
        assert_eq!(activation_effect(nrc), Transient, "{nrc:02X}");
    }
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
