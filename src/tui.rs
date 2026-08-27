use crate::{
    audit::{AuditEntry, AuditState},
    hex,
    telemetry::{Sample, TelemetryState},
    Transaction,
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
    widgets::{
        Axis, Block, Borders, Chart, Dataset, GraphType, List, ListItem, Paragraph, Sparkline, Wrap,
    },
    Frame, Terminal,
};
use std::{
    collections::VecDeque,
    fs,
    io::{self, Write},
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

#[cfg(test)]
use ratatui::backend::TestBackend;

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Value,
    Sparkline,
    TimeSeries,
    Compare,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Panel {
    pub title: String,
    pub view: View,
    pub signals: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DashboardLayout {
    pub name: String,
    pub panels: Vec<Panel>,
}

pub fn engine_overview() -> DashboardLayout {
    DashboardLayout {
        name: "engine-overview".into(),
        panels: vec![
            panel("Engine RPM", View::Value, &["engine.rpm"]),
            panel("RPM history", View::Sparkline, &["engine.rpm"]),
            panel(
                "Coolant temperature",
                View::Value,
                &["engine.coolant_temperature"],
            ),
            panel("Air flow", View::TimeSeries, &["engine.maf"]),
            panel("Road speed", View::Value, &["vehicle.speed"]),
            panel("RPM / MAF", View::Compare, &["engine.rpm", "engine.maf"]),
        ],
    }
}

fn panel(title: &str, view: View, signals: &[&str]) -> Panel {
    Panel {
        title: title.into(),
        view,
        signals: signals.iter().map(|signal| (*signal).into()).collect(),
    }
}

pub fn save_layout(path: &Path, layout: &DashboardLayout) -> Result<(), String> {
    validate_layout(layout)?;
    let mut contents = format!("OBDENTIC-LAYOUT\t1\nname\t{}\n", layout.name);
    for panel in &layout.panels {
        contents.push_str(&format!(
            "panel\t{}\t{}\t{}\n",
            panel.title,
            view_name(panel.view),
            panel.signals.join(",")
        ));
    }
    write_private_new(path, &contents)
}

pub fn load_layout(path: &Path) -> Result<DashboardLayout, String> {
    let contents = fs::read_to_string(path).map_err(|error| error.to_string())?;
    let mut lines = contents.lines();
    if lines.next() != Some("OBDENTIC-LAYOUT\t1") {
        return Err("unsupported layout format".into());
    }
    let name = layout_field(lines.next(), "name")?.into();
    let mut panels = Vec::new();
    for line in lines {
        let fields: Vec<_> = line.split('\t').collect();
        let ["panel", title, view, signals] = fields.as_slice() else {
            return Err("malformed layout panel".into());
        };
        panels.push(Panel {
            title: (*title).into(),
            view: parse_view(view)?,
            signals: signals.split(',').map(str::to_owned).collect(),
        });
    }
    let layout = DashboardLayout { name, panels };
    validate_layout(&layout)?;
    Ok(layout)
}

fn layout_field<'a>(line: Option<&'a str>, name: &str) -> Result<&'a str, String> {
    let Some(line) = line else {
        return Err(format!("layout is missing {name}"));
    };
    let Some((actual, value)) = line.split_once('\t') else {
        return Err(format!("malformed layout {name}"));
    };
    (actual == name)
        .then_some(value)
        .ok_or_else(|| format!("layout is missing {name}"))
}

fn view_name(view: View) -> &'static str {
    match view {
        View::Value => "value",
        View::Sparkline => "sparkline",
        View::TimeSeries => "time-series",
        View::Compare => "compare",
    }
}

fn parse_view(value: &str) -> Result<View, String> {
    match value {
        "value" => Ok(View::Value),
        "sparkline" => Ok(View::Sparkline),
        "time-series" => Ok(View::TimeSeries),
        "compare" => Ok(View::Compare),
        _ => Err(format!("unsupported layout view {value}")),
    }
}

fn validate_layout(layout: &DashboardLayout) -> Result<(), String> {
    layout_text("name", &layout.name)?;
    if layout.panels.is_empty() {
        return Err("layout has no panels".into());
    }
    for panel in &layout.panels {
        layout_text("panel title", &panel.title)?;
        let expected_signals = match panel.view {
            View::Compare => 2,
            View::Value | View::Sparkline | View::TimeSeries => 1,
        };
        if panel.signals.len() != expected_signals {
            return Err(format!(
                "{} requires {expected_signals} semantic signal(s)",
                panel.title
            ));
        }
        for signal in &panel.signals {
            if signal.is_empty()
                || !signal.contains('.')
                || !signal
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
            {
                return Err(format!(
                    "{} contains an invalid semantic signal",
                    panel.title
                ));
            }
        }
    }
    Ok(())
}

fn layout_text(field: &str, value: &str) -> Result<(), String> {
    if value.is_empty()
        || value
            .bytes()
            .any(|byte| matches!(byte, b'\t' | b'\r' | b'\n'))
    {
        return Err(format!(
            "layout {field} is empty or contains a tab or newline"
        ));
    }
    Ok(())
}

fn write_private_new(path: &Path, contents: &str) -> Result<(), String> {
    let temporary = path.with_extension("tmp");
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(&temporary)
        .map_err(|error| error.to_string())?;
    if let Err(error) = file
        .write_all(contents.as_bytes())
        .and_then(|_| file.flush())
        .and_then(|_| file.sync_all())
    {
        let _ = fs::remove_file(&temporary);
        return Err(error.to_string());
    }
    drop(file);
    let result = fs::hard_link(&temporary, path).and_then(|_| fs::remove_file(&temporary));
    if let Err(error) = result {
        let _ = fs::remove_file(&temporary);
        return Err(error.to_string());
    }
    Ok(())
}

pub fn run(
    layout: &DashboardLayout,
    state: &TelemetryState,
    transactions: &[Transaction],
) -> Result<(), String> {
    let audit = transactions
        .iter()
        .map(|transaction| AuditEntry {
            timestamp_ms: transaction.timestamp_ms(),
            source: transaction.source().into(),
            semantic: transaction.semantic(),
            request: transaction.request().to_vec(),
            response: transaction.response().to_vec(),
        })
        .collect::<Vec<_>>();
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
    let result = run_terminal(&mut terminal, layout, state, &audit);
    let _ = disable_raw_mode();
    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen);
    let _ = terminal.show_cursor();
    result
}

pub fn run_live(
    layout: &DashboardLayout,
    telemetry: Arc<Mutex<TelemetryState>>,
    audit: Arc<Mutex<AuditState>>,
) -> Result<(), String> {
    enable_raw_mode().map_err(|error| error.to_string())?;
    let mut stdout = io::stdout();
    if let Err(error) = execute!(stdout, EnterAlternateScreen) {
        let _ = disable_raw_mode();
        return Err(error.to_string());
    }
    let mut terminal =
        Terminal::new(CrosstermBackend::new(stdout)).map_err(|error| error.to_string())?;
    let result = loop {
        let state = telemetry
            .lock()
            .map_err(|_| "telemetry state lock poisoned")?
            .clone();
        let audit = audit
            .lock()
            .map_err(|_| "audit state lock poisoned")?
            .snapshot();
        terminal
            .draw(|frame| render(frame, layout, &state, &audit))
            .map_err(|error| error.to_string())?;
        if event::poll(Duration::from_millis(100)).map_err(|error| error.to_string())?
            && matches!(event::read().map_err(|error| error.to_string())?, Event::Key(key) if matches!(key.code, KeyCode::Char('q') | KeyCode::Esc))
        {
            break Ok(());
        }
    };
    let _ = disable_raw_mode();
    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen);
    let _ = terminal.show_cursor();
    result
}

fn run_terminal(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    layout: &DashboardLayout,
    state: &TelemetryState,
    audit: &[AuditEntry],
) -> Result<(), String> {
    loop {
        terminal
            .draw(|frame| render(frame, layout, state, audit))
            .map_err(|error| error.to_string())?;
        match event::read().map_err(|error| error.to_string())? {
            Event::Key(key) if matches!(key.code, KeyCode::Char('q') | KeyCode::Esc) => {
                return Ok(())
            }
            _ => {}
        }
    }
}

fn render(
    frame: &mut Frame,
    layout: &DashboardLayout,
    state: &TelemetryState,
    audit: &[AuditEntry],
) {
    let areas = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(15),
        Constraint::Length(6),
    ])
    .split(frame.area());
    let source = audit
        .first()
        .map(|entry| entry.source.as_str())
        .unwrap_or("none");
    frame.render_widget(
        Paragraph::new(format!(
            " OBDentic  |  {}  |  offline {source}  |  q / Esc closes",
            layout.name
        ))
        .block(Block::default().borders(Borders::ALL).title("Connection")),
        areas[0],
    );

    render_panels(frame, areas[1], layout, state);
    let bottom = Layout::horizontal([Constraint::Percentage(65), Constraint::Percentage(35)])
        .split(areas[2]);
    let raw = audit.iter().map(|entry| {
        ListItem::new(format!(
            "{}  TX {}  RX {}",
            entry.semantic,
            hex(&entry.request),
            hex(&entry.response)
        ))
    });
    frame.render_widget(
        List::new(raw).block(Block::default().borders(Borders::ALL).title("Raw TX/RX")),
        bottom[0],
    );
    let activity = audit
        .iter()
        .map(|entry| ListItem::new(format!("{} -> read {}", entry.source, entry.semantic)));
    frame.render_widget(
        List::new(activity).block(Block::default().borders(Borders::ALL).title("Activity")),
        bottom[1],
    );
}

fn render_panels(frame: &mut Frame, area: Rect, layout: &DashboardLayout, state: &TelemetryState) {
    let rows = layout.panels.len().div_ceil(2);
    if rows == 0 {
        return;
    }
    let rows = Layout::vertical(vec![Constraint::Percentage(100 / rows as u16); rows]).split(area);
    for (panel, area) in layout.panels.iter().zip(rows.iter().flat_map(|row| {
        let columns = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(*row);
        [columns[0], columns[1]]
    })) {
        render_panel(frame, area, state, panel);
    }
}

fn render_panel(frame: &mut Frame, area: Rect, state: &TelemetryState, panel: &Panel) {
    match panel.view {
        View::Value => render_value(frame, area, state, panel),
        View::Sparkline => render_sparkline(frame, area, state, panel),
        View::TimeSeries => render_time_series(frame, area, state, panel),
        View::Compare => render_compare(frame, area, state, panel),
    }
}

fn one_signal(panel: &Panel) -> Option<&str> {
    match panel.signals.as_slice() {
        [signal] => Some(signal),
        _ => None,
    }
}

fn unsupported(frame: &mut Frame, area: Rect, panel: &Panel, message: impl Into<String>) {
    frame.render_widget(
        Paragraph::new(format!("unsupported: {}", message.into())).block(
            Block::default()
                .borders(Borders::ALL)
                .title(panel.title.as_str()),
        ),
        area,
    );
}

fn render_value(frame: &mut Frame, area: Rect, state: &TelemetryState, panel: &Panel) {
    let Some(signal) = one_signal(panel) else {
        return unsupported(frame, area, panel, "Value requires one signal");
    };
    let value = state
        .current(signal)
        .map(|sample| format!("{:.2} {}", sample.value, sample.unit));
    match value {
        Some(value) => frame.render_widget(
            Paragraph::new(vec![
                Line::from(value).style(Style::default().fg(Color::Cyan)),
                Line::from(signal),
            ])
            .wrap(Wrap { trim: true })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(panel.title.as_str()),
            ),
            area,
        ),
        None => unsupported(frame, area, panel, signal),
    }
}

fn render_sparkline(frame: &mut Frame, area: Rect, state: &TelemetryState, panel: &Panel) {
    let Some(signal) = one_signal(panel) else {
        return unsupported(frame, area, panel, "Sparkline requires one signal");
    };
    let Some(history) = state.history(signal) else {
        return unsupported(frame, area, panel, signal);
    };
    frame.render_widget(
        Sparkline::default()
            .data(sparkline_values(Some(history)))
            .style(Style::default().fg(Color::Green))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(panel.title.as_str()),
            ),
        area,
    );
}

fn render_time_series(frame: &mut Frame, area: Rect, state: &TelemetryState, panel: &Panel) {
    let Some(signal) = one_signal(panel) else {
        return unsupported(frame, area, panel, "TimeSeries requires one signal");
    };
    let Some(history) = state.history(signal).filter(|history| !history.is_empty()) else {
        return unsupported(frame, area, panel, signal);
    };
    let points = time_points_from(history, history.front().unwrap().timestamp_ms);
    let (x_minimum, x_maximum, y_minimum, y_maximum) = chart_bounds(&points, &[]);
    let chart = Chart::new(vec![Dataset::default()
        .name(signal)
        .graph_type(GraphType::Line)
        .style(Style::default().fg(Color::Cyan))
        .data(&points)])
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(panel.title.as_str()),
    )
    .x_axis(
        Axis::default()
            .title("seconds")
            .bounds([x_minimum, x_maximum]),
    )
    .y_axis(
        Axis::default()
            .title(history.front().unwrap().unit)
            .bounds([y_minimum, y_maximum]),
    );
    frame.render_widget(chart, area);
}

fn sparkline_values(history: Option<&VecDeque<Sample>>) -> Vec<u64> {
    let Some(history) = history else {
        return Vec::new();
    };
    let minimum = history
        .iter()
        .map(|sample| sample.value)
        .fold(f64::INFINITY, f64::min);
    let maximum = history
        .iter()
        .map(|sample| sample.value)
        .fold(f64::NEG_INFINITY, f64::max);
    let span = maximum - minimum;
    history
        .iter()
        .map(|sample| {
            if span == 0.0 {
                100
            } else {
                ((sample.value - minimum) * 100.0 / span) as u64
            }
        })
        .collect()
}

fn time_points_from(history: &VecDeque<Sample>, origin: u128) -> Vec<(f64, f64)> {
    history
        .iter()
        .map(|sample| {
            (
                sample.timestamp_ms.saturating_sub(origin) as f64 / 1_000.0,
                sample.value,
            )
        })
        .collect()
}

fn render_compare(frame: &mut Frame, area: Rect, state: &TelemetryState, panel: &Panel) {
    let [left, right] = panel.signals.as_slice() else {
        return unsupported(frame, area, panel, "Compare requires two signals");
    };
    let (Some(left_history), Some(right_history)) = (state.history(left), state.history(right))
    else {
        return unsupported(frame, area, panel, format!("{left} / {right}"));
    };
    let (Some(left_unit), Some(right_unit)) = (left_history.front(), right_history.front()) else {
        return unsupported(frame, area, panel, format!("{left} / {right}"));
    };
    if left_unit.unit != right_unit.unit {
        frame.render_widget(
            Paragraph::new(format!(
                "incompatible units: {} vs {}",
                left_unit.unit, right_unit.unit
            ))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(panel.title.as_str()),
            ),
            area,
        );
        return;
    }
    let origin = left_unit.timestamp_ms.min(right_unit.timestamp_ms);
    let left_points = time_points_from(left_history, origin);
    let right_points = time_points_from(right_history, origin);
    let (x_minimum, x_maximum, y_minimum, y_maximum) = chart_bounds(&left_points, &right_points);
    let chart = Chart::new(vec![
        Dataset::default()
            .name(left.as_str())
            .graph_type(GraphType::Line)
            .style(Style::default().fg(Color::Cyan))
            .data(&left_points),
        Dataset::default()
            .name(right.as_str())
            .graph_type(GraphType::Line)
            .style(Style::default().fg(Color::Yellow))
            .data(&right_points),
    ])
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(panel.title.as_str()),
    )
    .x_axis(
        Axis::default()
            .title("seconds")
            .bounds([x_minimum, x_maximum]),
    )
    .y_axis(
        Axis::default()
            .title(left_unit.unit)
            .bounds([y_minimum, y_maximum]),
    );
    frame.render_widget(chart, area);
}

fn chart_bounds(left: &[(f64, f64)], right: &[(f64, f64)]) -> (f64, f64, f64, f64) {
    let mut values = left.iter().chain(right.iter());
    let Some((first_x, first_y)) = values.next().copied() else {
        return (0.0, 1.0, 0.0, 1.0);
    };
    let (mut x_minimum, mut x_maximum, mut y_minimum, mut y_maximum) =
        (first_x, first_x, first_y, first_y);
    for (x, y) in values {
        x_minimum = x_minimum.min(*x);
        x_maximum = x_maximum.max(*x);
        y_minimum = y_minimum.min(*y);
        y_maximum = y_maximum.max(*y);
    }
    if x_minimum == x_maximum {
        x_maximum += 1.0;
    }
    if y_minimum == y_maximum {
        y_maximum += 1.0;
    }
    (x_minimum, x_maximum, y_minimum, y_maximum)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{prepare_read, telemetry::TelemetryState};

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    fn temp_path(label: &str) -> std::path::PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "obdentic-{label}-{}-{nonce}.layout",
            std::process::id()
        ))
    }

    fn compatible_compare() -> Panel {
        panel(
            "RPM comparison",
            View::Compare,
            &["engine.rpm", "engine.rpm"],
        )
    }

    fn custom_layout() -> DashboardLayout {
        DashboardLayout {
            name: "custom".into(),
            panels: vec![
                panel("Custom value", View::Value, &["engine.rpm"]),
                panel("Custom sparkline", View::Sparkline, &["engine.rpm"]),
                panel("Custom time series", View::TimeSeries, &["engine.rpm"]),
                compatible_compare(),
            ],
        }
    }

    fn audit(transactions: &[&Transaction]) -> Vec<AuditEntry> {
        transactions
            .iter()
            .map(|transaction| AuditEntry {
                timestamp_ms: transaction.timestamp_ms(),
                source: transaction.source().into(),
                semantic: transaction.semantic(),
                request: transaction.request().to_vec(),
                response: transaction.response().to_vec(),
            })
            .collect()
    }

    #[test]
    fn renders_decoded_samples_without_requesting_transport() {
        let transaction = prepare_read("engine.rpm")
            .unwrap()
            .complete("demo", vec![0x41, 0x0c, 0x1a, 0xf8])
            .unwrap();
        let mut state = TelemetryState::new(2).unwrap();
        state.ingest(&transaction);
        let maf = prepare_read("engine.maf")
            .unwrap()
            .complete("demo", vec![0x41, 0x10, 0x01, 0x90])
            .unwrap();
        state.ingest(&maf);
        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        let layout = engine_overview();
        let audit = audit(&[&transaction, &maf]);
        terminal
            .draw(|frame| render(frame, &layout, &state, &audit))
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("Engine RPM"));
        assert!(text.contains("01 0C"));
        assert!(text.contains("1726.00 rpm"));
        assert!(text.contains("incompatible units: rpm vs g/s"));
    }

    #[test]
    fn chart_data_respects_sample_order_timestamps_and_units() {
        let history = VecDeque::from([
            Sample {
                timestamp_ms: 1_000,
                value: 1.0,
                unit: "rpm",
            },
            Sample {
                timestamp_ms: 2_500,
                value: 3.0,
                unit: "rpm",
            },
            Sample {
                timestamp_ms: 6_000,
                value: 2.0,
                unit: "rpm",
            },
        ]);
        assert_eq!(sparkline_values(Some(&history)), [0, 100, 50]);
        assert_eq!(
            time_points_from(&history, 1_000),
            [(0.0, 1.0), (1.5, 3.0), (5.0, 2.0)]
        );

        let compatible = VecDeque::from([Sample {
            timestamp_ms: 1_000,
            value: 1.0,
            unit: "rpm",
        }]);
        assert_eq!(time_points_from(&compatible, 0), [(1.0, 1.0)]);
    }

    #[test]
    fn renders_compatible_compare_as_a_chart() {
        let first = prepare_read("engine.rpm")
            .unwrap()
            .complete("demo", vec![0x41, 0x0c, 0x0c, 0x80])
            .unwrap()
            .with_timestamp_ms(1_000);
        let second = prepare_read("engine.rpm")
            .unwrap()
            .complete("demo", vec![0x41, 0x0c, 0x12, 0xc0])
            .unwrap()
            .with_timestamp_ms(3_500);
        let mut state = TelemetryState::new(2).unwrap();
        state.ingest(&first);
        state.ingest(&second);
        let layout = compatible_compare();
        let mut terminal = Terminal::new(TestBackend::new(80, 12)).unwrap();
        terminal
            .draw(|frame| {
                let area = frame.area();
                render_compare(frame, area, &state, &layout)
            })
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("RPM comparison"));
        assert!(!text.contains("incompatible units"));
    }

    #[test]
    fn renders_panels_from_the_declared_layout() {
        let transaction = prepare_read("engine.rpm")
            .unwrap()
            .complete("demo", vec![0x41, 0x0c, 0x1a, 0xf8])
            .unwrap();
        let mut state = TelemetryState::new(2).unwrap();
        state.ingest(&transaction);
        let layout = custom_layout();
        let audit = audit(&[&transaction]);
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        terminal
            .draw(|frame| render(frame, &layout, &state, &audit))
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        for title in [
            "Custom value",
            "Custom sparkline",
            "Custom time series",
            "RPM comparison",
        ] {
            assert!(text.contains(title));
        }
    }

    #[test]
    fn saves_and_loads_private_semantic_layouts_without_overwriting() {
        let path = temp_path("layout");
        let layout = engine_overview();
        save_layout(&path, &layout).unwrap();
        assert_eq!(load_layout(&path).unwrap(), layout);
        assert!(fs::read_to_string(&path)
            .unwrap()
            .starts_with("OBDENTIC-LAYOUT\t1\n"));
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(save_layout(&path, &layout).is_err());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn rejects_non_semantic_layout_content_and_renders_missing_signals() {
        let invalid = DashboardLayout {
            name: "invalid".into(),
            panels: vec![panel("Raw PID", View::Value, &["010C"])],
        };
        assert!(validate_layout(&invalid).is_err());

        let layout = DashboardLayout {
            name: "missing".into(),
            panels: vec![panel("Future signal", View::Value, &["future.signal"])],
        };
        let transaction = prepare_read("engine.rpm")
            .unwrap()
            .complete("demo", vec![0x41, 0x0c, 0x1a, 0xf8])
            .unwrap();
        let mut state = TelemetryState::new(1).unwrap();
        state.ingest(&transaction);
        let audit = audit(&[&transaction]);
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal
            .draw(|frame| render(frame, &layout, &state, &audit))
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("unsupported: future.signal"));
    }
}
