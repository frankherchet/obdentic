use obdentic::{
    capture_events::{CaptureEvent, CaptureValue, DtcObservationFact, DtcTransportOutcome, SubscriptionFilterOutcome},
    hex,
    jsonl_capture::{self, CaptureStatus, ParsedCapture},
    tui::{self, DashboardLayout, Panel, View},
};
use ratatui::{
    backend::CrosstermBackend,
    crossterm::{
        event::{self, Event, KeyCode},
        execute,
        terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    },
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    text::Line,
    widgets::{Axis, Block, Borders, Chart, Dataset, GraphType, List, ListItem, Paragraph, Sparkline, Wrap},
    Frame, Terminal,
};
use std::{
    collections::BTreeMap,
    env,
    io::{self},
    path::Path,
};

#[cfg(test)]
use ratatui::backend::TestBackend;

const DEFAULT_WINDOW_US: u64 = 60_000_000;
const MIN_WINDOW_US: u64 = 1_000_000;

#[derive(Clone, Debug, PartialEq)]
struct OfflineSample {
    timestamp_us: u64,
    value: CaptureValue,
    unit: String,
    request: Vec<u8>,
    response: Vec<u8>,
    source: String,
    profile: String,
    decoder: String,
    provenance: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TimelineEvent {
    timestamp_us: u64,
    kind: &'static str,
    semantic: Option<String>,
    detail: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Availability {
    Captured,
    Unsupported,
    RequestedNoSample,
    NotCaptured,
}

#[derive(Clone, Debug, PartialEq)]
struct OfflineTimeline {
    status: CaptureStatus,
    profile: Option<String>,
    duration_us: u64,
    samples: BTreeMap<String, Vec<OfflineSample>>,
    subscriptions: BTreeMap<String, SubscriptionFilterOutcome>,
    events: Vec<TimelineEvent>,
}

impl OfflineTimeline {
    fn from_capture(capture: &ParsedCapture) -> Self {
        let mut timeline = Self {
            status: capture.status,
            profile: None,
            duration_us: 0,
            samples: BTreeMap::new(),
            subscriptions: BTreeMap::new(),
            events: Vec::new(),
        };
        let mut last_clock_us = 0_u64;
        let mut pending_response_event = None;

        for event in &capture.events {
            match event {
                CaptureEvent::CaptureStarted { profile, .. } => {
                    timeline.profile = profile.clone();
                    timeline.push_event(0, "capture_started", None, format!("profile={}", profile.as_deref().unwrap_or("unknown")));
                }
                CaptureEvent::SessionInitialized => {
                    timeline.push_event(last_clock_us, "session_initialized", None, String::new());
                }
                CaptureEvent::SubscriptionConfigured { semantic, filter, requested_interval_us } => {
                    timeline.subscriptions.insert(semantic.clone(), *filter);
                    timeline.push_event(
                        last_clock_us,
                        "subscription",
                        Some(semantic.clone()),
                        format!("interval={}ms status={}", requested_interval_us / 1_000, filter_name(*filter)),
                    );
                }
                CaptureEvent::SupportDiscovery { request_payload, responder, response_payload } => {
                    timeline.push_event(
                        last_clock_us,
                        "support",
                        None,
                        format!("{} TX {} RX {}", responder.as_deref().unwrap_or("unknown"), hex(request_payload), hex(response_payload)),
                    );
                }
                CaptureEvent::ProtocolNegotiationObserved { request_payload, responder, response_payload } => {
                    timeline.push_event(
                        last_clock_us,
                        "protocol",
                        None,
                        format!("{} TX {} RX {}", responder.as_deref().unwrap_or("unknown"), hex(request_payload), hex(response_payload)),
                    );
                }
                CaptureEvent::ResponsesObserved { semantic, request_payload, responses, selected_responder, selection_error } => {
                    let responders = responses
                        .iter()
                        .map(|response| format!("{}:{}", response.responder.as_deref().unwrap_or("unknown"), hex(&response.payload)))
                        .collect::<Vec<_>>()
                        .join(" | ");
                    timeline.push_event(
                        last_clock_us,
                        "responses",
                        Some(semantic.clone()),
                        format!(
                            "TX {} responders [{}] selected={}{}",
                            hex(request_payload),
                            responders,
                            selected_responder.as_deref().unwrap_or("none"),
                            selection_error.as_ref().map(|error| format!(" error={error}")).unwrap_or_default(),
                        ),
                    );
                    pending_response_event = Some(timeline.events.len() - 1);
                }
                CaptureEvent::ReadSucceeded {
                    semantic,
                    finished_us,
                    request_payload,
                    response_payload,
                    value,
                    unit,
                    source,
                    profile,
                    decoder,
                    provenance,
                    ..
                } => {
                    last_clock_us = last_clock_us.max(*finished_us);
                    timeline.duration_us = timeline.duration_us.max(*finished_us);
                    if let Some(index) = pending_response_event.take() {
                        timeline.events[index].timestamp_us = *finished_us;
                    }
                    timeline.samples.entry(semantic.clone()).or_default().push(OfflineSample {
                        timestamp_us: *finished_us,
                        value: value.clone(),
                        unit: unit.clone(),
                        request: request_payload.clone(),
                        response: response_payload.clone(),
                        source: source.clone(),
                        profile: profile.clone(),
                        decoder: decoder.clone(),
                        provenance: provenance.clone(),
                    });
                    timeline.push_event(
                        *finished_us,
                        "read_ok",
                        Some(semantic.clone()),
                        format!("TX {} RX {} -> {} {}", hex(request_payload), hex(response_payload), value_text(value), unit),
                    );
                }
                CaptureEvent::ReadFailed { semantic, timing, request_payload, error, .. } => {
                    let timestamp_us = timing.map(|timing| timing.finished_us).unwrap_or(last_clock_us);
                    last_clock_us = last_clock_us.max(timestamp_us);
                    timeline.duration_us = timeline.duration_us.max(timestamp_us);
                    if let Some(index) = pending_response_event.take() {
                        timeline.events[index].timestamp_us = timestamp_us;
                    }
                    timeline.push_event(
                        timestamp_us,
                        "read_error",
                        Some(semantic.clone()),
                        format!("TX {} error={error}", request_payload.as_deref().map(hex).unwrap_or_else(|| "none".into())),
                    );
                }
                CaptureEvent::SlotsSkipped { semantic, count, first_due_us, last_due_us } => {
                    last_clock_us = last_clock_us.max(*last_due_us);
                    timeline.duration_us = timeline.duration_us.max(*last_due_us);
                    timeline.push_event(
                        *last_due_us,
                        "slots_skipped",
                        Some(semantic.clone()),
                        format!("count={count} due={}..{}ms", first_due_us / 1_000, last_due_us / 1_000),
                    );
                }
                CaptureEvent::SessionError { error } => {
                    timeline.push_event(last_clock_us, "session_error", None, error.clone());
                }
                CaptureEvent::RuntimeStateChanged { from, to, event, .. } => {
                    timeline.push_event(last_clock_us, "runtime", None, format!("{} -> {} ({event:?})", from.serialize(), to.serialize()));
                }
                CaptureEvent::ShutdownRequested => {
                    timeline.push_event(last_clock_us, "shutdown_requested", None, String::new());
                }
                CaptureEvent::SessionStopped { offset_us } => {
                    last_clock_us = last_clock_us.max(*offset_us);
                    timeline.duration_us = timeline.duration_us.max(*offset_us);
                    timeline.push_event(*offset_us, "session_stopped", None, String::new());
                }
                CaptureEvent::DiagnosticJobStarted { job_id, step_count, .. } => {
                    timeline.push_event(last_clock_us, "job_started", None, format!("{job_id} steps={step_count}"));
                }
                CaptureEvent::DiagnosticJobStep { job_id, step_sequence, mode, source, status, error } => {
                    timeline.push_event(last_clock_us, "job_step", None, format!("{job_id} step={step_sequence} mode={mode:02X} source={} status={}{}", source.as_deref().unwrap_or("unknown"), status.as_str(), error.as_ref().map(|error| format!(" error={error}")).unwrap_or_default()));
                }
                CaptureEvent::DiagnosticJobCompleted { job_id, status } => {
                    timeline.push_event(last_clock_us, "job_completed", None, format!("{job_id} status={status:?}"));
                }
                CaptureEvent::DiagnosticJobFailed { job_id, error } => {
                    timeline.push_event(last_clock_us, "job_failed", None, format!("{job_id} error={error}"));
                }
                CaptureEvent::DiagnosticJobCancelled { job_id } => {
                    timeline.push_event(last_clock_us, "job_cancelled", None, job_id.clone());
                }
                CaptureEvent::DtcTransportObserved { job_id, step_sequence, responder, outcome } => {
                    timeline.push_event(last_clock_us, "dtc_transport", None, format!("{job_id} step={step_sequence} responder={} {}", responder.as_deref().unwrap_or("unknown"), dtc_transport_text(outcome)));
                }
                CaptureEvent::DtcObservation { job_id, responder, fact, .. } => {
                    timeline.push_event(last_clock_us, "dtc_fact", None, format!("{job_id} responder={} {}", responder.as_deref().unwrap_or("unknown"), dtc_fact_text(fact)));
                }
            }
        }

        for samples in timeline.samples.values_mut() {
            samples.sort_by_key(|sample| sample.timestamp_us);
        }
        timeline.events.sort_by_key(|event| event.timestamp_us);
        timeline
    }

    fn push_event(&mut self, timestamp_us: u64, kind: &'static str, semantic: Option<String>, detail: String) {
        self.events.push(TimelineEvent { timestamp_us, kind, semantic, detail });
    }

    fn availability(&self, semantic: &str) -> Availability {
        if self.samples.get(semantic).is_some_and(|samples| !samples.is_empty()) {
            Availability::Captured
        } else {
            match self.subscriptions.get(semantic) {
                Some(SubscriptionFilterOutcome::Unsupported | SubscriptionFilterOutcome::Unknown) => Availability::Unsupported,
                Some(SubscriptionFilterOutcome::Scheduled) => Availability::RequestedNoSample,
                None => Availability::NotCaptured,
            }
        }
    }

    fn current_at(&self, semantic: &str, cursor_us: u64) -> Option<&OfflineSample> {
        let samples = self.samples.get(semantic)?;
        let index = samples.partition_point(|sample| sample.timestamp_us <= cursor_us);
        index.checked_sub(1).and_then(|index| samples.get(index))
    }

    fn samples_in(&self, semantic: &str, start_us: u64, end_us: u64) -> &[OfflineSample] {
        let Some(samples) = self.samples.get(semantic) else {
            return &[];
        };
        let start = samples.partition_point(|sample| sample.timestamp_us < start_us);
        let end = samples.partition_point(|sample| sample.timestamp_us <= end_us);
        &samples[start..end]
    }

    fn events_in(&self, start_us: u64, end_us: u64) -> &[TimelineEvent] {
        let start = self.events.partition_point(|event| event.timestamp_us < start_us);
        let end = self.events.partition_point(|event| event.timestamp_us <= end_us);
        &self.events[start..end]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Navigation {
    cursor_us: u64,
    window_us: u64,
}

impl Navigation {
    fn new(duration_us: u64) -> Self {
        Self {
            cursor_us: duration_us,
            window_us: DEFAULT_WINDOW_US.min(duration_us.max(MIN_WINDOW_US)),
        }
    }

    fn bounds(self, duration_us: u64) -> (u64, u64) {
        if duration_us <= self.window_us {
            return (0, duration_us.max(1));
        }
        let half = self.window_us / 2;
        let mut start = self.cursor_us.saturating_sub(half);
        let mut end = start.saturating_add(self.window_us);
        if end > duration_us {
            end = duration_us;
            start = end.saturating_sub(self.window_us);
        }
        (start, end.max(start + 1))
    }

    fn apply(&mut self, key: KeyCode, duration_us: u64) {
        let step = (self.window_us / 10).max(100_000);
        match key {
            KeyCode::Left => self.cursor_us = self.cursor_us.saturating_sub(step),
            KeyCode::Right => self.cursor_us = self.cursor_us.saturating_add(step).min(duration_us),
            KeyCode::PageUp => self.cursor_us = self.cursor_us.saturating_sub(self.window_us),
            KeyCode::PageDown => self.cursor_us = self.cursor_us.saturating_add(self.window_us).min(duration_us),
            KeyCode::Home | KeyCode::Char('g') => self.cursor_us = 0,
            KeyCode::End | KeyCode::Char('G') => self.cursor_us = duration_us,
            KeyCode::Char('+') | KeyCode::Char('=') => {
                self.window_us = (self.window_us / 2).max(MIN_WINDOW_US).min(duration_us.max(MIN_WINDOW_US));
            }
            KeyCode::Char('-') => {
                self.window_us = self.window_us.saturating_mul(2).min(duration_us.max(MIN_WINDOW_US));
            }
            _ => {}
        }
    }
}

fn filter_name(filter: SubscriptionFilterOutcome) -> &'static str {
    match filter {
        SubscriptionFilterOutcome::Scheduled => "scheduled",
        SubscriptionFilterOutcome::Unsupported => "unsupported",
        SubscriptionFilterOutcome::Unknown => "unknown",
    }
}

fn value_text(value: &CaptureValue) -> String {
    match value {
        CaptureValue::Number(value) => format!("{value:.3}"),
        CaptureValue::Boolean(value) => value.to_string(),
        CaptureValue::Enum(value) | CaptureValue::Text(value) => value.clone(),
        CaptureValue::Unavailable { reason } => format!("unavailable({reason})"),
    }
}

fn dtc_transport_text(outcome: &DtcTransportOutcome) -> String {
    match outcome {
        DtcTransportOutcome::Response => "response".into(),
        DtcTransportOutcome::NoResponse => "no_response".into(),
        DtcTransportOutcome::Malformed => "malformed".into(),
        DtcTransportOutcome::Error(error) => format!("error={error}"),
    }
}

fn dtc_fact_text(fact: &DtcObservationFact) -> String {
    match fact {
        DtcObservationFact::DtcCode(code) => format!("dtc={code}"),
        DtcObservationFact::NoDtcs => "no_dtcs".into(),
        DtcObservationFact::DecodeError(error) => format!("decode_error={error}"),
    }
}

fn run_capture_tui(path: &Path, layout: &DashboardLayout) -> Result<(), String> {
    let capture = jsonl_capture::read(path)?;
    let timeline = OfflineTimeline::from_capture(&capture);
    enable_raw_mode().map_err(|error| error.to_string())?;
    let mut stdout = io::stdout();
    if let Err(error) = execute!(stdout, EnterAlternateScreen) {
        let _ = disable_raw_mode();
        return Err(error.to_string());
    }
    let mut terminal = match Terminal::new(CrosstermBackend::new(stdout)) {
        Ok(terminal) => terminal,
        Err(error) => {
            let _ = disable_raw_mode();
            return Err(error.to_string());
        }
    };
    let mut navigation = Navigation::new(timeline.duration_us);
    let result = loop {
        terminal
            .draw(|frame| render_offline(frame, layout, &timeline, navigation, &path.display().to_string()))
            .map_err(|error| error.to_string())?;
        match event::read().map_err(|error| error.to_string())? {
            Event::Key(key) if matches!(key.code, KeyCode::Char('q') | KeyCode::Esc) => break Ok(()),
            Event::Key(key) => navigation.apply(key.code, timeline.duration_us),
            _ => {}
        }
    };
    let _ = disable_raw_mode();
    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen);
    let _ = terminal.show_cursor();
    result
}

fn render_offline(
    frame: &mut Frame,
    layout: &DashboardLayout,
    timeline: &OfflineTimeline,
    navigation: Navigation,
    path: &str,
) {
    let areas = Layout::vertical([
        Constraint::Length(4),
        Constraint::Min(12),
        Constraint::Length(9),
    ])
    .split(frame.area());
    let (start_us, end_us) = navigation.bounds(timeline.duration_us);
    let status = match timeline.status {
        CaptureStatus::Complete => "COMPLETE",
        CaptureStatus::Partial => "PARTIAL",
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(format!(
                " OBDentic | capture {} | {} | profile {}",
                path,
                status,
                timeline.profile.as_deref().unwrap_or("unknown")
            )),
            Line::from(format!(
                " cursor {:.3}s / {:.3}s | window {:.3}s..{:.3}s | ←/→ PgUp/PgDn Home/End +/- | q/Esc",
                navigation.cursor_us as f64 / 1_000_000.0,
                timeline.duration_us as f64 / 1_000_000.0,
                start_us as f64 / 1_000_000.0,
                end_us as f64 / 1_000_000.0,
            )),
        ])
        .block(Block::default().borders(Borders::ALL).title("Offline capture")),
        areas[0],
    );

    render_panels(frame, areas[1], layout, timeline, navigation, start_us, end_us);
    let bottom = Layout::horizontal([Constraint::Percentage(68), Constraint::Percentage(32)]).split(areas[2]);
    render_evidence(frame, bottom[0], timeline, start_us, end_us);
    render_event_status(frame, bottom[1], timeline, start_us, end_us);
}

fn render_panels(
    frame: &mut Frame,
    area: Rect,
    layout: &DashboardLayout,
    timeline: &OfflineTimeline,
    navigation: Navigation,
    start_us: u64,
    end_us: u64,
) {
    let rows = layout.panels.len().div_ceil(2);
    if rows == 0 {
        return;
    }
    let rows = Layout::vertical(vec![Constraint::Percentage(100 / rows as u16); rows]).split(area);
    for (panel, area) in layout.panels.iter().zip(rows.iter().flat_map(|row| {
        let columns = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).split(*row);
        [columns[0], columns[1]]
    })) {
        render_panel(frame, *area, panel, timeline, navigation.cursor_us, start_us, end_us);
    }
}

fn render_panel(
    frame: &mut Frame,
    area: Rect,
    panel: &Panel,
    timeline: &OfflineTimeline,
    cursor_us: u64,
    start_us: u64,
    end_us: u64,
) {
    match panel.view {
        View::Value => render_value(frame, area, panel, timeline, cursor_us),
        View::Sparkline => render_sparkline(frame, area, panel, timeline, start_us, end_us),
        View::TimeSeries => render_time_series(frame, area, panel, timeline, start_us, end_us),
        View::Compare => render_compare(frame, area, panel, timeline, start_us, end_us),
    }
}

fn one_signal(panel: &Panel) -> Option<&str> {
    match panel.signals.as_slice() {
        [signal] => Some(signal),
        _ => None,
    }
}

fn unavailable(frame: &mut Frame, area: Rect, panel: &Panel, timeline: &OfflineTimeline, semantic: &str) {
    let status = match timeline.availability(semantic) {
        Availability::Captured => "no sample in selected window",
        Availability::Unsupported => "unsupported / unavailable in capture",
        Availability::RequestedNoSample => "requested but not sampled",
        Availability::NotCaptured => "not captured",
    };
    frame.render_widget(
        Paragraph::new(format!("{status}: {semantic}"))
            .block(Block::default().borders(Borders::ALL).title(panel.title.as_str())),
        area,
    );
}

fn render_value(frame: &mut Frame, area: Rect, panel: &Panel, timeline: &OfflineTimeline, cursor_us: u64) {
    let Some(semantic) = one_signal(panel) else {
        return unavailable(frame, area, panel, timeline, "invalid value panel");
    };
    let Some(sample) = timeline.current_at(semantic, cursor_us) else {
        return unavailable(frame, area, panel, timeline, semantic);
    };
    let age_ms = cursor_us.saturating_sub(sample.timestamp_us) / 1_000;
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(format!("{} {}", value_text(&sample.value), sample.unit)).style(Style::default().fg(Color::Cyan)),
            Line::from(format!("{semantic} | age {age_ms} ms")),
            Line::from(format!("source {} | {}", sample.source, sample.profile)),
        ])
        .wrap(Wrap { trim: true })
        .block(Block::default().borders(Borders::ALL).title(panel.title.as_str())),
        area,
    );
}

fn render_sparkline(frame: &mut Frame, area: Rect, panel: &Panel, timeline: &OfflineTimeline, start_us: u64, end_us: u64) {
    let Some(semantic) = one_signal(panel) else {
        return unavailable(frame, area, panel, timeline, "invalid sparkline panel");
    };
    let samples = timeline.samples_in(semantic, start_us, end_us);
    let values = sparkline_values(samples);
    if values.is_empty() {
        return unavailable(frame, area, panel, timeline, semantic);
    }
    frame.render_widget(
        Sparkline::default()
            .data(&values)
            .style(Style::default().fg(Color::Green))
            .block(Block::default().borders(Borders::ALL).title(panel.title.as_str())),
        area,
    );
}

fn sparkline_values(samples: &[OfflineSample]) -> Vec<u64> {
    let numeric = samples
        .iter()
        .filter_map(|sample| match sample.value { CaptureValue::Number(value) => Some(value), _ => None })
        .collect::<Vec<_>>();
    if numeric.is_empty() {
        return Vec::new();
    }
    let minimum = numeric.iter().copied().fold(f64::INFINITY, f64::min);
    let maximum = numeric.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let span = maximum - minimum;
    numeric
        .into_iter()
        .map(|value| if span == 0.0 { 1 } else { ((value - minimum) * 100.0 / span) as u64 })
        .collect()
}

fn numeric_points(samples: &[OfflineSample]) -> Vec<(f64, f64)> {
    samples
        .iter()
        .filter_map(|sample| match sample.value {
            CaptureValue::Number(value) => Some((sample.timestamp_us as f64 / 1_000_000.0, value)),
            _ => None,
        })
        .collect()
}

fn render_time_series(frame: &mut Frame, area: Rect, panel: &Panel, timeline: &OfflineTimeline, start_us: u64, end_us: u64) {
    let Some(semantic) = one_signal(panel) else {
        return unavailable(frame, area, panel, timeline, "invalid time-series panel");
    };
    let samples = timeline.samples_in(semantic, start_us, end_us);
    let points = numeric_points(samples);
    if points.is_empty() {
        return unavailable(frame, area, panel, timeline, semantic);
    }
    let unit = samples.iter().find_map(|sample| matches!(sample.value, CaptureValue::Number(_)).then_some(sample.unit.as_str())).unwrap_or("");
    let (_, _, y_min, y_max) = chart_bounds(&points, &[]);
    let chart = Chart::new(vec![Dataset::default()
        .name(semantic)
        .graph_type(GraphType::Line)
        .style(Style::default().fg(Color::Cyan))
        .data(&points)])
        .block(Block::default().borders(Borders::ALL).title(panel.title.as_str()))
        .x_axis(Axis::default().title("capture seconds").bounds([start_us as f64 / 1_000_000.0, end_us as f64 / 1_000_000.0]))
        .y_axis(Axis::default().title(unit).bounds([y_min, y_max]));
    frame.render_widget(chart, area);
}

fn render_compare(frame: &mut Frame, area: Rect, panel: &Panel, timeline: &OfflineTimeline, start_us: u64, end_us: u64) {
    let [left, right] = panel.signals.as_slice() else {
        return unavailable(frame, area, panel, timeline, "invalid compare panel");
    };
    let left_samples = timeline.samples_in(left, start_us, end_us);
    let right_samples = timeline.samples_in(right, start_us, end_us);
    let left_points = numeric_points(left_samples);
    let right_points = numeric_points(right_samples);
    if left_points.is_empty() || right_points.is_empty() {
        return unavailable(frame, area, panel, timeline, &format!("{left} / {right}"));
    }
    let left_unit = left_samples.iter().find_map(|sample| matches!(sample.value, CaptureValue::Number(_)).then_some(sample.unit.as_str())).unwrap_or("");
    let right_unit = right_samples.iter().find_map(|sample| matches!(sample.value, CaptureValue::Number(_)).then_some(sample.unit.as_str())).unwrap_or("");
    if left_unit != right_unit {
        frame.render_widget(
            Paragraph::new(format!("incompatible units: {left_unit} vs {right_unit}"))
                .block(Block::default().borders(Borders::ALL).title(panel.title.as_str())),
            area,
        );
        return;
    }
    let (_, _, y_min, y_max) = chart_bounds(&left_points, &right_points);
    let chart = Chart::new(vec![
        Dataset::default().name(left.as_str()).graph_type(GraphType::Line).style(Style::default().fg(Color::Cyan)).data(&left_points),
        Dataset::default().name(right.as_str()).graph_type(GraphType::Line).style(Style::default().fg(Color::Yellow)).data(&right_points),
    ])
    .block(Block::default().borders(Borders::ALL).title(panel.title.as_str()))
    .x_axis(Axis::default().title("capture seconds").bounds([start_us as f64 / 1_000_000.0, end_us as f64 / 1_000_000.0]))
    .y_axis(Axis::default().title(left_unit).bounds([y_min, y_max]));
    frame.render_widget(chart, area);
}

fn chart_bounds(left: &[(f64, f64)], right: &[(f64, f64)]) -> (f64, f64, f64, f64) {
    let mut values = left.iter().chain(right.iter());
    let Some((first_x, first_y)) = values.next().copied() else {
        return (0.0, 1.0, 0.0, 1.0);
    };
    let (mut x_min, mut x_max, mut y_min, mut y_max) = (first_x, first_x, first_y, first_y);
    for (x, y) in values {
        x_min = x_min.min(*x);
        x_max = x_max.max(*x);
        y_min = y_min.min(*y);
        y_max = y_max.max(*y);
    }
    if x_min == x_max { x_max += 1.0; }
    if y_min == y_max { y_max += 1.0; }
    (x_min, x_max, y_min, y_max)
}

fn render_evidence(frame: &mut Frame, area: Rect, timeline: &OfflineTimeline, start_us: u64, end_us: u64) {
    let events = timeline.events_in(start_us, end_us);
    let items = events
        .iter()
        .rev()
        .filter(|event| matches!(event.kind, "read_ok" | "read_error" | "responses" | "support" | "protocol" | "dtc_fact" | "dtc_transport"))
        .take(6)
        .rev()
        .map(|event| ListItem::new(format!("{:>8.3}s {:<12} {} {}", event.timestamp_us as f64 / 1_000_000.0, event.kind, event.semantic.as_deref().unwrap_or(""), event.detail)));
    frame.render_widget(
        List::new(items).block(Block::default().borders(Borders::ALL).title("Evidence / TX / RX")),
        area,
    );
}

fn render_event_status(frame: &mut Frame, area: Rect, timeline: &OfflineTimeline, start_us: u64, end_us: u64) {
    let items = timeline
        .events_in(start_us, end_us)
        .iter()
        .rev()
        .filter(|event| matches!(event.kind, "read_error" | "slots_skipped" | "session_error" | "job_failed" | "runtime" | "session_stopped"))
        .take(6)
        .rev()
        .map(|event| ListItem::new(format!("{:>7.2}s {} {}", event.timestamp_us as f64 / 1_000_000.0, event.kind, event.detail)));
    frame.render_widget(
        List::new(items).block(Block::default().borders(Borders::ALL).title("Errors / lifecycle")),
        area,
    );
}

fn usage() -> &'static str {
    "usage: obdentic-capture-tui <capture.jsonl> [--layout <layout.tsv>]"
}

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    let (capture, layout) = match args.as_slice() {
        [capture] => (capture.as_str(), None),
        [capture, flag, layout] if flag == "--layout" => (capture.as_str(), Some(layout.as_str())),
        _ => return Err(usage().into()),
    };
    let layout = layout.map_or_else(|| Ok(tui::engine_overview()), |path| tui::load_layout(Path::new(path)))?;
    run_capture_tui(Path::new(capture), &layout)
}

#[cfg(test)]
mod tests {
    use super::*;
    use obdentic::capture_events::{ReadTiming, ResponderEvidence};

    fn sample_event(semantic: &str, timestamp_us: u64, value: f64, unit: &str) -> CaptureEvent {
        CaptureEvent::ReadSucceeded {
            semantic: semantic.into(),
            requested_interval_us: 1_000_000,
            due_us: timestamp_us.saturating_sub(100_000),
            started_us: timestamp_us.saturating_sub(50_000),
            finished_us: timestamp_us,
            request_payload: vec![0x01, 0x0c],
            response_payload: vec![0x41, 0x0c, 0x00, 0x00],
            value: CaptureValue::Number(value),
            unit: unit.into(),
            source: "7E8".into(),
            profile: "obd2-v1".into(),
            decoder: "test".into(),
            provenance: "synthetic".into(),
        }
    }

    fn capture(status: CaptureStatus) -> ParsedCapture {
        let mut events = vec![
            CaptureEvent::capture_started(None, Some("engine-drive".into())),
            CaptureEvent::subscription_configured("engine.rpm", 1_000_000, SubscriptionFilterOutcome::Scheduled),
            CaptureEvent::subscription_configured("future.signal", 1_000_000, SubscriptionFilterOutcome::Unsupported),
            CaptureEvent::responses_observed(
                "engine.rpm",
                vec![0x01, 0x0c],
                vec![ResponderEvidence::new(Some("7E8".into()), vec![0x41, 0x0c, 0x0c, 0x80]).unwrap()],
                Some("7E8".into()),
                None,
            ).unwrap(),
            sample_event("engine.rpm", 1_000_000, 800.0, "rpm"),
            sample_event("engine.rpm", 2_500_000, 1_200.0, "rpm"),
            CaptureEvent::ReadFailed {
                semantic: "engine.rpm".into(),
                requested_interval_us: 1_000_000,
                timing: Some(ReadTiming::new(3_000_000, 3_050_000, 3_200_000)),
                request_payload: Some(vec![0x01, 0x0c]),
                error: "conflicting responders".into(),
            },
            CaptureEvent::SlotsSkipped {
                semantic: "engine.rpm".into(),
                count: 1,
                first_due_us: 4_000_000,
                last_due_us: 4_000_000,
            },
        ];
        if status == CaptureStatus::Complete {
            events.push(CaptureEvent::SessionStopped { offset_us: 5_000_000 });
        }
        ParsedCapture { events, status }
    }

    fn buffer_text(terminal: &Terminal<TestBackend>) -> String {
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn timeline_indexes_irregular_samples_and_prior_value_without_transport() {
        let timeline = OfflineTimeline::from_capture(&capture(CaptureStatus::Complete));
        assert_eq!(timeline.duration_us, 5_000_000);
        assert_eq!(timeline.current_at("engine.rpm", 2_000_000).unwrap().timestamp_us, 1_000_000);
        assert_eq!(timeline.current_at("engine.rpm", 3_000_000).unwrap().timestamp_us, 2_500_000);
        assert_eq!(timeline.availability("future.signal"), Availability::Unsupported);
        assert_eq!(timeline.availability("unknown.signal"), Availability::NotCaptured);
        let points = numeric_points(timeline.samples_in("engine.rpm", 0, 5_000_000));
        assert_eq!(points, [(1.0, 800.0), (2.5, 1_200.0)]);
    }

    #[test]
    fn navigation_uses_capture_time_and_zoom_is_bounded() {
        let mut navigation = Navigation::new(120_000_000);
        assert_eq!(navigation.cursor_us, 120_000_000);
        navigation.apply(KeyCode::Home, 120_000_000);
        assert_eq!(navigation.cursor_us, 0);
        navigation.apply(KeyCode::Right, 120_000_000);
        assert!(navigation.cursor_us > 0);
        let previous = navigation.window_us;
        navigation.apply(KeyCode::Char('+'), 120_000_000);
        assert!(navigation.window_us < previous);
        for _ in 0..20 {
            navigation.apply(KeyCode::Char('+'), 120_000_000);
        }
        assert_eq!(navigation.window_us, MIN_WINDOW_US);
        navigation.apply(KeyCode::End, 120_000_000);
        assert_eq!(navigation.cursor_us, 120_000_000);
    }

    #[test]
    fn renders_partial_status_missing_signal_and_raw_evidence() {
        let timeline = OfflineTimeline::from_capture(&capture(CaptureStatus::Partial));
        let layout = DashboardLayout {
            name: "offline-test".into(),
            panels: vec![
                Panel { title: "RPM".into(), view: View::TimeSeries, signals: vec!["engine.rpm".into()] },
                Panel { title: "Missing".into(), view: View::Value, signals: vec!["future.signal".into()] },
            ],
        };
        let mut terminal = Terminal::new(TestBackend::new(140, 35)).unwrap();
        terminal
            .draw(|frame| render_offline(frame, &layout, &timeline, Navigation::new(timeline.duration_us), "fixture.jsonl"))
            .unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("PARTIAL"));
        assert!(text.contains("unsupported / unavailable in capture"));
        assert!(text.contains("TX 01 0C"));
        assert!(text.contains("conflicting responders"));
        assert!(text.contains("slots_skipped"));
    }

    #[test]
    fn compare_rejects_incompatible_units_without_interpolation() {
        let mut parsed = capture(CaptureStatus::Complete);
        parsed.events.push(sample_event("vehicle.speed", 2_000_000, 50.0, "km/h"));
        let timeline = OfflineTimeline::from_capture(&parsed);
        let panel = Panel {
            title: "bad compare".into(),
            view: View::Compare,
            signals: vec!["engine.rpm".into(), "vehicle.speed".into()],
        };
        let mut terminal = Terminal::new(TestBackend::new(80, 12)).unwrap();
        terminal.draw(|frame| render_compare(frame, frame.area(), &panel, &timeline, 0, 5_000_000)).unwrap();
        assert!(buffer_text(&terminal).contains("incompatible units: rpm vs km/h"));
    }
}
