use crate::{hex, supported_signals, Transaction};
use ratatui::{
    backend::CrosstermBackend,
    crossterm::{
        event::{self, Event, KeyCode},
        execute,
        terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    },
    layout::{Constraint, Layout},
    style::{Color, Style},
    text::Line,
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
    Frame, Terminal,
};
use std::io;

#[cfg(test)]
use ratatui::backend::TestBackend;

const LAYOUT_NAME: &str = "engine-overview";

pub fn run(transactions: &[Transaction]) -> Result<(), String> {
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
    let result = run_terminal(&mut terminal, transactions);
    let _ = disable_raw_mode();
    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen);
    let _ = terminal.show_cursor();
    result
}

fn run_terminal(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    transactions: &[Transaction],
) -> Result<(), String> {
    loop {
        terminal
            .draw(|frame| render(frame, transactions))
            .map_err(|error| error.to_string())?;
        match event::read().map_err(|error| error.to_string())? {
            Event::Key(key) if matches!(key.code, KeyCode::Char('q') | KeyCode::Esc) => {
                return Ok(())
            }
            _ => {}
        }
    }
}

fn render(frame: &mut Frame, transactions: &[Transaction]) {
    let areas = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(15),
        Constraint::Length(6),
    ])
    .split(frame.area());
    let source = transactions
        .first()
        .map(|transaction| transaction.source.as_str())
        .unwrap_or("none");
    frame.render_widget(
        Paragraph::new(format!(
            " OBDentic  |  {LAYOUT_NAME}  |  offline {source}  |  q / Esc closes"
        ))
        .block(Block::default().borders(Borders::ALL).title("Connection")),
        areas[0],
    );

    let rows =
        Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)]).split(areas[1]);
    let panels = [
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).split(rows[0]),
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).split(rows[1]),
    ]
    .concat();
    for (area, signal) in panels.into_iter().zip(supported_signals()) {
        let metadata = signal.metadata();
        let latest = transactions
            .iter()
            .rev()
            .find(|transaction| transaction.semantic == metadata.semantic);
        let value = latest
            .map(|transaction| format!("{:.2} {}", transaction.value, transaction.unit))
            .unwrap_or_else(|| "unsupported".into());
        let samples = transactions
            .iter()
            .filter(|transaction| transaction.semantic == metadata.semantic)
            .count();
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(value).style(Style::default().fg(Color::Cyan)),
                Line::from(format!("decode: {}", metadata.decoder)),
                Line::from(format!(
                    "{} / {}",
                    metadata.confidence, metadata.hardware_validation
                )),
                Line::from(format!("history: {samples} sample(s), chart pending")),
            ])
            .wrap(Wrap { trim: true })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(metadata.semantic),
            ),
            area,
        );
    }

    let bottom = Layout::horizontal([Constraint::Percentage(65), Constraint::Percentage(35)])
        .split(areas[2]);
    let raw = transactions.iter().map(|transaction| {
        ListItem::new(format!(
            "{}  TX {}  RX {}",
            transaction.semantic,
            hex(&transaction.request),
            hex(&transaction.response)
        ))
    });
    frame.render_widget(
        List::new(raw).block(Block::default().borders(Borders::ALL).title("Raw TX/RX")),
        bottom[0],
    );
    let activity = transactions.iter().map(|transaction| {
        ListItem::new(format!(
            "{} -> read {}",
            transaction.source, transaction.semantic
        ))
    });
    frame.render_widget(
        List::new(activity).block(Block::default().borders(Borders::ALL).title("Activity")),
        bottom[1],
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prepare_read;

    #[test]
    fn renders_decoded_samples_without_requesting_transport() {
        let transaction = prepare_read("engine.rpm")
            .unwrap()
            .complete("demo", vec![0x41, 0x0c, 0x1a, 0xf8])
            .unwrap();
        let backend = TestBackend::new(120, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render(frame, &[transaction]))
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("engine.rpm"));
        assert!(text.contains("01 0C"));
        assert!(text.contains("1726.00 rpm"));
    }
}
