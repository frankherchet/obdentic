use crate::{
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
use std::{collections::VecDeque, io};

#[cfg(test)]
use ratatui::backend::TestBackend;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Value,
    Sparkline,
    TimeSeries,
    Compare,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Panel {
    pub title: &'static str,
    pub view: View,
    pub signals: &'static [&'static str],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DashboardLayout {
    pub name: &'static str,
    pub panels: &'static [Panel],
}

const ENGINE_OVERVIEW_PANELS: [Panel; 6] = [
    Panel {
        title: "Engine RPM",
        view: View::Value,
        signals: &["engine.rpm"],
    },
    Panel {
        title: "RPM history",
        view: View::Sparkline,
        signals: &["engine.rpm"],
    },
    Panel {
        title: "Coolant temperature",
        view: View::Value,
        signals: &["engine.coolant_temperature"],
    },
    Panel {
        title: "Air flow",
        view: View::TimeSeries,
        signals: &["engine.maf"],
    },
    Panel {
        title: "Road speed",
        view: View::Value,
        signals: &["vehicle.speed"],
    },
    Panel {
        title: "RPM / MAF",
        view: View::Compare,
        signals: &["engine.rpm", "engine.maf"],
    },
];

const ENGINE_OVERVIEW: DashboardLayout = DashboardLayout {
    name: "engine-overview",
    panels: &ENGINE_OVERVIEW_PANELS,
};

pub fn engine_overview() -> &'static DashboardLayout {
    &ENGINE_OVERVIEW
}

pub fn run(
    layout: &DashboardLayout,
    state: &TelemetryState,
    transactions: &[Transaction],
) -> Result<(), String> {
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
    let result = run_terminal(&mut terminal, layout, state, transactions);
    let _ = disable_raw_mode();
    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen);
    let _ = terminal.show_cursor();
    result
}

fn run_terminal(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    layout: &DashboardLayout,
    state: &TelemetryState,
    transactions: &[Transaction],
) -> Result<(), String> {
    loop {
        terminal
            .draw(|frame| render(frame, layout, state, transactions))
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
    transactions: &[Transaction],
) {
    let areas = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(15),
        Constraint::Length(6),
    ])
    .split(frame.area());
    let source = transactions
        .first()
        .map(|transaction| transaction.source())
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
    let raw = transactions.iter().map(|transaction| {
        ListItem::new(format!(
            "{}  TX {}  RX {}",
            transaction.semantic(),
            hex(transaction.request()),
            hex(transaction.response())
        ))
    });
    frame.render_widget(
        List::new(raw).block(Block::default().borders(Borders::ALL).title("Raw TX/RX")),
        bottom[0],
    );
    let activity = transactions.iter().map(|transaction| {
        ListItem::new(format!(
            "{} -> read {}",
            transaction.source(),
            transaction.semantic()
        ))
    });
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

fn one_signal(panel: &Panel) -> Option<&'static str> {
    match panel.signals {
        [signal] => Some(signal),
        _ => None,
    }
}

fn unsupported(frame: &mut Frame, area: Rect, panel: &Panel, message: impl Into<String>) {
    frame.render_widget(
        Paragraph::new(format!("unsupported: {}", message.into()))
            .block(Block::default().borders(Borders::ALL).title(panel.title)),
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
            .block(Block::default().borders(Borders::ALL).title(panel.title)),
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
            .block(Block::default().borders(Borders::ALL).title(panel.title)),
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
    .block(Block::default().borders(Borders::ALL).title(panel.title))
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
    let &[left, right] = panel.signals else {
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
            .block(Block::default().borders(Borders::ALL).title(panel.title)),
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
            .name(left)
            .graph_type(GraphType::Line)
            .style(Style::default().fg(Color::Cyan))
            .data(&left_points),
        Dataset::default()
            .name(right)
            .graph_type(GraphType::Line)
            .style(Style::default().fg(Color::Yellow))
            .data(&right_points),
    ])
    .block(Block::default().borders(Borders::ALL).title(panel.title))
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

    const COMPATIBLE_COMPARE: Panel = Panel {
        title: "RPM comparison",
        view: View::Compare,
        signals: &["engine.rpm", "engine.rpm"],
    };
    const CUSTOM_PANELS: [Panel; 4] = [
        Panel {
            title: "Custom value",
            view: View::Value,
            signals: &["engine.rpm"],
        },
        Panel {
            title: "Custom sparkline",
            view: View::Sparkline,
            signals: &["engine.rpm"],
        },
        Panel {
            title: "Custom time series",
            view: View::TimeSeries,
            signals: &["engine.rpm"],
        },
        COMPATIBLE_COMPARE,
    ];
    const CUSTOM_LAYOUT: DashboardLayout = DashboardLayout {
        name: "custom",
        panels: &CUSTOM_PANELS,
    };

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
        terminal
            .draw(|frame| render(frame, engine_overview(), &state, &[transaction, maf]))
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
        let mut terminal = Terminal::new(TestBackend::new(80, 12)).unwrap();
        terminal
            .draw(|frame| {
                let area = frame.area();
                render_compare(frame, area, &state, &COMPATIBLE_COMPARE)
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
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        terminal
            .draw(|frame| render(frame, &CUSTOM_LAYOUT, &state, &[transaction]))
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
}
