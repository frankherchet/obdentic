//! Deterministic, read-only summaries of recorded capture events.

use crate::{
    capture_events::{CaptureEvent, CaptureTimeUs, CaptureValue, SubscriptionFilterOutcome},
    jsonl_capture::{CaptureStatus, ParsedCapture},
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default)]
struct SignalSummary {
    interval_us: Option<CaptureTimeUs>,
    filter: Option<SubscriptionFilterOutcome>,
    samples: u64,
    failures: u64,
    skipped: u64,
    first_sample_us: Option<CaptureTimeUs>,
    last_sample_us: Option<CaptureTimeUs>,
    values: Vec<f64>,
    units: BTreeSet<String>,
    successful_times: Vec<(CaptureTimeUs, CaptureTimeUs, CaptureTimeUs)>,
}

#[derive(Default)]
struct Summary {
    signals: BTreeMap<String, SignalSummary>,
    events: u64,
    successes: u64,
    failures: u64,
    skipped_events: u64,
    skipped_slots: u64,
    lifecycle: u64,
    first_offset_us: Option<CaptureTimeUs>,
    last_offset_us: Option<CaptureTimeUs>,
    profile: Option<String>,
    wallclock_ms: Option<u64>,
    session_errors: Vec<String>,
    error_messages: BTreeMap<String, u64>,
    all_reads: Vec<(CaptureTimeUs, CaptureTimeUs, CaptureTimeUs)>,
}

fn summary(capture: &ParsedCapture) -> Summary {
    let mut summary = Summary::default();
    for event in &capture.events {
        summary.events += 1;
        match event {
            CaptureEvent::CaptureStarted {
                wallclock_ms,
                profile,
            } => {
                summary.lifecycle += 1;
                summary.wallclock_ms = *wallclock_ms;
                summary.profile = profile.clone();
            }
            CaptureEvent::SessionInitialized
            | CaptureEvent::SupportDiscovery { .. }
            | CaptureEvent::RuntimeStateChanged { .. }
            | CaptureEvent::ShutdownRequested => summary.lifecycle += 1,
            CaptureEvent::ResponsesObserved { .. } => {
                // Raw responder evidence remains in ParsedCapture for an
                // explicit offline re-decoder; summaries do not reinterpret it.
            }
            CaptureEvent::SessionStopped { offset_us } => {
                summary.lifecycle += 1;
                observe_offset(&mut summary, *offset_us);
            }
            CaptureEvent::SessionError { error } => {
                summary.lifecycle += 1;
                summary.session_errors.push(error.clone());
            }
            CaptureEvent::SubscriptionConfigured {
                semantic,
                requested_interval_us,
                filter,
            } => {
                let signal = summary.signals.entry(semantic.clone()).or_default();
                signal.interval_us = Some(*requested_interval_us);
                signal.filter = Some(*filter);
            }
            CaptureEvent::ReadSucceeded {
                semantic,
                requested_interval_us,
                due_us,
                started_us,
                finished_us,
                value,
                unit,
                ..
            } => {
                summary.successes += 1;
                observe_offset(&mut summary, *due_us);
                observe_offset(&mut summary, *finished_us);
                let signal = summary.signals.entry(semantic.clone()).or_default();
                signal.interval_us.get_or_insert(*requested_interval_us);
                signal.samples += 1;
                signal.first_sample_us = Some(
                    signal
                        .first_sample_us
                        .map_or(*finished_us, |old| old.min(*finished_us)),
                );
                signal.last_sample_us = Some(
                    signal
                        .last_sample_us
                        .map_or(*finished_us, |old| old.max(*finished_us)),
                );
                signal.units.insert(unit.clone());
                if let CaptureValue::Number(value) = value {
                    signal.values.push(*value);
                }
                signal
                    .successful_times
                    .push((*due_us, *started_us, *finished_us));
                summary.all_reads.push((*due_us, *started_us, *finished_us));
            }
            CaptureEvent::ReadFailed {
                semantic,
                requested_interval_us,
                timing,
                error,
                ..
            } => {
                summary.failures += 1;
                *summary.error_messages.entry(error.clone()).or_default() += 1;
                let signal = summary.signals.entry(semantic.clone()).or_default();
                signal.interval_us.get_or_insert(*requested_interval_us);
                signal.failures += 1;
                if let Some(timing) = timing {
                    observe_offset(&mut summary, timing.due_us);
                    observe_offset(&mut summary, timing.finished_us);
                    summary
                        .all_reads
                        .push((timing.due_us, timing.started_us, timing.finished_us));
                }
            }
            CaptureEvent::SlotsSkipped {
                semantic,
                count,
                first_due_us,
                last_due_us,
            } => {
                summary.skipped_events += 1;
                summary.skipped_slots += count;
                observe_offset(&mut summary, *first_due_us);
                observe_offset(&mut summary, *last_due_us);
                let signal = summary.signals.entry(semantic.clone()).or_default();
                signal.skipped += count;
            }
        }
    }
    summary
}

fn observe_offset(summary: &mut Summary, offset: CaptureTimeUs) {
    summary.first_offset_us = Some(
        summary
            .first_offset_us
            .map_or(offset, |old| old.min(offset)),
    );
    summary.last_offset_us = Some(summary.last_offset_us.map_or(offset, |old| old.max(offset)));
}

pub fn render_inspection(path: &str, capture: &ParsedCapture) -> String {
    let summary = summary(capture);
    let mut output = format!(
        "Capture: {path}\nFormat: JSONL {}\nStatus: {}\nProfile: {}\nStarted: {}\nDuration: {}\nEvents: {}\nReads: {} succeeded, {} failed\nSkipped: {} events, {} slots\nLifecycle events: {}\n\nSignals\n",
        crate::jsonl_capture::VERSION,
        status(capture.status),
        summary.profile.as_deref().unwrap_or("unavailable"),
        summary.wallclock_ms.map_or_else(|| "unavailable".into(), |value| value.to_string()),
        duration(summary.first_offset_us, summary.last_offset_us),
        summary.events,
        summary.successes,
        summary.failures,
        summary.skipped_events,
        summary.skipped_slots,
        summary.lifecycle,
    );
    for (semantic, signal) in &summary.signals {
        let units = join_or_unavailable(&signal.units);
        let range = if signal.units.len() > 1 {
            "incompatible units".into()
        } else {
            numeric_range(&signal.values)
        };
        output.push_str(&format!(
            "  {semantic}\n    requested: {} ({})\n    samples: {}; failures: {}; skipped: {}\n    first/last sample: {} / {}\n    units: {units}; numeric range: {range}\n",
            signal.interval_us.map_or_else(|| "unavailable".into(), format_interval),
            filter_name(signal.filter),
            signal.samples,
            signal.failures,
            signal.skipped,
            offset(signal.first_sample_us),
            offset(signal.last_sample_us),
        ));
    }
    if !summary.session_errors.is_empty() {
        output.push_str("\nSession errors\n");
        for error in summary.session_errors {
            output.push_str(&format!("  {error}\n"));
        }
    }
    if !summary.error_messages.is_empty() {
        output.push_str("\nRecorded read errors\n");
        for (error, count) in summary.error_messages {
            output.push_str(&format!("  {count} × {error}\n"));
        }
    }
    output
}

pub fn render_capability(path: &str, capture: &ParsedCapture) -> String {
    let summary = summary(capture);
    let duration_us = summary
        .last_offset_us
        .zip(summary.first_offset_us)
        .map(|(last, first)| last - first);
    let success_rate = rate(summary.successes, duration_us);
    let attempted = summary.successes + summary.failures;
    let attempted_rate = rate(attempted, duration_us);
    let latencies = summary
        .all_reads
        .iter()
        .map(|(_, start, finish)| finish - start)
        .collect::<Vec<_>>();
    let lateness = summary
        .all_reads
        .iter()
        .map(|(due, start, _)| start - due)
        .collect::<Vec<_>>();
    let mut output = format!(
        "Capture: {path}\nStatus: {}\nDuration: {}\n\nOverall\n  successful reads: {}\n  failed reads: {} ({})\n  observed successful throughput: {} reads/s\n  observed attempted throughput: {} reads/s\n  read latency p50/p95/max: {}\n  scheduler lateness p50/p95/max: {}\n  skipped slots: {} across {} events\n  session errors: {}\n\nSignals\n",
        status(capture.status),
        duration(summary.first_offset_us, summary.last_offset_us),
        summary.successes,
        summary.failures,
        percentage(summary.failures, attempted),
        success_rate,
        attempted_rate,
        metrics(&latencies),
        metrics(&lateness),
        summary.skipped_slots,
        summary.skipped_events,
        summary.session_errors.len(),
    );
    for (semantic, signal) in &summary.signals {
        let timings = &signal.successful_times;
        let intervals = timings
            .windows(2)
            .map(|pair| pair[1].2 - pair[0].2)
            .collect::<Vec<_>>();
        let signal_span = timings
            .first()
            .zip(timings.last())
            .map(|(first, last)| last.2 - first.2);
        output.push_str(&format!(
            "  {semantic}\n    requested: {}\n    successful samples: {}; failures: {}; skipped slots: {}\n    achieved: {} samples/s\n    sample interval p50/p95/max: {}\n    longest sample gap: {}\n",
            signal.interval_us.map_or_else(|| "unavailable".into(), format_interval),
            signal.samples,
            signal.failures,
            signal.skipped,
            rate(signal.samples, signal_span),
            metrics(&intervals),
            intervals.iter().max().map_or_else(|| "unavailable".into(), |value| format_us(*value)),
        ));
    }
    if !summary.error_messages.is_empty() {
        output.push_str("\nRecorded errors\n");
        for (error, count) in summary.error_messages {
            output.push_str(&format!("  {count} × {error}\n"));
        }
    }
    output
}

fn status(status: CaptureStatus) -> &'static str {
    match status {
        CaptureStatus::Complete => "complete",
        CaptureStatus::Partial => "partial",
    }
}

fn offset(value: Option<CaptureTimeUs>) -> String {
    value.map_or_else(|| "unavailable".into(), format_us)
}

fn duration(first: Option<CaptureTimeUs>, last: Option<CaptureTimeUs>) -> String {
    first.zip(last).map_or_else(
        || "unavailable".into(),
        |(first, last)| format_us(last - first),
    )
}

fn format_interval(value: CaptureTimeUs) -> String {
    format_us(value)
}

fn format_us(value: CaptureTimeUs) -> String {
    if value.is_multiple_of(1_000_000) {
        format!("{} s", value / 1_000_000)
    } else if value.is_multiple_of(1_000) {
        format!("{} ms", value / 1_000)
    } else {
        format!("{value} us")
    }
}

fn filter_name(value: Option<SubscriptionFilterOutcome>) -> &'static str {
    match value {
        Some(SubscriptionFilterOutcome::Scheduled) => "scheduled",
        Some(SubscriptionFilterOutcome::Unsupported) => "unsupported",
        Some(SubscriptionFilterOutcome::Unknown) => "unknown",
        None => "unavailable",
    }
}

fn join_or_unavailable(values: &BTreeSet<String>) -> String {
    if values.is_empty() {
        "unavailable".into()
    } else {
        values.iter().cloned().collect::<Vec<_>>().join(", ")
    }
}

fn numeric_range(values: &[f64]) -> String {
    if values.is_empty() {
        return "unavailable".into();
    }
    let minimum = values.iter().copied().fold(f64::INFINITY, f64::min);
    let maximum = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    format!("{minimum} .. {maximum}")
}

/// Nearest-rank percentile: p50/p95 pick element ceil(p*n)-1 from sorted data.
fn percentile(values: &[CaptureTimeUs], numerator: u64, denominator: u64) -> Option<CaptureTimeUs> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let rank = u64::try_from(sorted.len())
        .ok()?
        .saturating_mul(numerator)
        .div_ceil(denominator);
    sorted
        .get(usize::try_from(rank.saturating_sub(1)).ok()?)
        .copied()
}

fn metrics(values: &[CaptureTimeUs]) -> String {
    match (
        percentile(values, 50, 100),
        percentile(values, 95, 100),
        values.iter().max(),
    ) {
        (Some(p50), Some(p95), Some(maximum)) => format!(
            "{} / {} / {}",
            format_us(p50),
            format_us(p95),
            format_us(*maximum)
        ),
        _ => "unavailable".into(),
    }
}

fn rate(count: u64, duration_us: Option<CaptureTimeUs>) -> String {
    match duration_us {
        Some(0) | None => "unavailable".into(),
        Some(duration) => format!("{:.2}", (count as f64) * 1_000_000.0 / duration as f64),
    }
}

fn percentage(part: u64, total: u64) -> String {
    if total == 0 {
        "unavailable".into()
    } else {
        format!("{:.2}%", (part as f64) * 100.0 / total as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture_events::{CaptureEvent, CaptureValue};

    fn capture(events: Vec<CaptureEvent>) -> ParsedCapture {
        ParsedCapture {
            events,
            status: CaptureStatus::Complete,
        }
    }

    fn success(
        semantic: &str,
        due_us: u64,
        started_us: u64,
        finished_us: u64,
        value: f64,
        unit: &str,
    ) -> CaptureEvent {
        CaptureEvent::ReadSucceeded {
            semantic: semantic.into(),
            requested_interval_us: 100_000,
            due_us,
            started_us,
            finished_us,
            request_payload: vec![1, 12],
            response_payload: vec![65, 12, 0, 0],
            value: CaptureValue::Number(value),
            unit: unit.into(),
            source: "user".into(),
            profile: "generic-obd2".into(),
            decoder: "x".into(),
            provenance: "x".into(),
        }
    }

    #[test]
    fn inspection_keeps_failures_and_reports_numeric_ranges() {
        let rendered = render_inspection(
            "sample.jsonl",
            &capture(vec![
                CaptureEvent::capture_started(Some(1), Some("engine-baseline".into())),
                CaptureEvent::subscription_configured(
                    "engine.rpm",
                    100_000,
                    SubscriptionFilterOutcome::Scheduled,
                ),
                success("engine.rpm", 0, 10, 20, 800.0, "rpm"),
                success("engine.rpm", 100_000, 110_000, 120_000, 900.0, "rpm"),
                CaptureEvent::read_failed(
                    "engine.rpm",
                    100_000,
                    None,
                    Some(vec![1, 12]),
                    "ambiguous responders",
                ),
                CaptureEvent::SessionStopped { offset_us: 130_000 },
            ]),
        );
        assert!(rendered.contains("Reads: 2 succeeded, 1 failed"));
        assert!(rendered.contains("numeric range: 800 .. 900"));
        assert!(rendered.contains("ambiguous responders"));
    }

    #[test]
    fn capability_uses_monotonic_timing_and_nearest_rank_percentiles() {
        let rendered = render_capability(
            "sample.jsonl",
            &capture(vec![
                success("engine.rpm", 0, 10, 20, 1.0, "rpm"),
                success("engine.rpm", 100, 120, 150, 2.0, "rpm"),
                success("engine.rpm", 200, 230, 270, 3.0, "rpm"),
                CaptureEvent::SlotsSkipped {
                    semantic: "engine.rpm".into(),
                    count: 2,
                    first_due_us: 300,
                    last_due_us: 400,
                },
                CaptureEvent::SessionStopped { offset_us: 500 },
            ]),
        );
        assert!(rendered.contains("read latency p50/p95/max: 30 us / 40 us / 40 us"));
        assert!(rendered.contains("scheduler lateness p50/p95/max: 20 us / 30 us / 30 us"));
        assert!(rendered.contains("skipped slots: 2 across 1 events"));
    }

    #[test]
    fn units_are_never_merged_silently() {
        let rendered = render_inspection(
            "sample.jsonl",
            &capture(vec![
                success("engine.rpm", 0, 0, 1, 1.0, "rpm"),
                success("engine.rpm", 2, 2, 3, 1.0, "%"),
            ]),
        );
        assert!(rendered.contains("incompatible units"));
    }
}
