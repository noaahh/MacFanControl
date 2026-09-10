use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Cell, Gauge, Paragraph, Row, Sparkline, Table};

use crate::app::App;
use crate::fan::{Fan, FanMode};
use crate::processes::ProcessSort;

const ACCENT: Color = Color::Rgb(0, 215, 255);
const HOT: Color = Color::Rgb(255, 95, 95);
const WARM: Color = Color::Rgb(255, 215, 0);
const COOL: Color = Color::Rgb(0, 255, 135);
const DIM: Color = Color::Rgb(110, 110, 130);
const MANUAL: Color = Color::Rgb(255, 121, 198);

fn rpm_color(ratio: f64) -> Color {
    if ratio < 0.45 {
        COOL
    } else if ratio < 0.75 {
        WARM
    } else {
        HOT
    }
}

fn temp_color(t: f32) -> Color {
    if t < 60.0 {
        COOL
    } else if t < 85.0 {
        WARM
    } else {
        HOT
    }
}

pub fn draw(frame: &mut Frame, app: &App) {
    let mut constraints = vec![Constraint::Length(1)];
    constraints.extend(app.fans.iter().map(|_| Constraint::Length(4)));
    constraints.push(Constraint::Length(3));
    constraints.push(Constraint::Fill(3));
    constraints.push(Constraint::Fill(2));
    constraints.push(Constraint::Length(1));
    constraints.push(Constraint::Length(1));
    let areas = Layout::vertical(constraints).split(frame.area());

    draw_title(frame, areas[0], app);
    for (i, fan) in app.fans.iter().enumerate() {
        draw_fan(
            frame,
            areas[1 + i],
            fan,
            app.desired[i],
            i == app.selected,
            app.linked,
        );
    }
    let base = 1 + app.fans.len();
    draw_temps(frame, areas[base], app, app.selected == app.fans.len());
    draw_processes(frame, areas[base + 1], app);
    draw_history(frame, areas[base + 2], app);
    draw_status(frame, areas[base + 3], app);
    draw_help(frame, areas[base + 4]);
}

fn draw_title(frame: &mut Frame, area: Rect, app: &App) {
    let access = if app.is_root {
        Span::styled(" CONTROL ", Style::new().fg(Color::Black).bg(COOL).bold())
    } else {
        Span::styled(" READ-ONLY ", Style::new().fg(Color::Black).bg(WARM).bold())
    };
    let line = Line::from(vec![
        Span::styled(" MACFAN ", Style::new().fg(Color::Black).bg(ACCENT).bold()),
        Span::raw(" "),
        Span::styled(&app.model, Style::new().fg(Color::White).bold()),
        Span::raw("  "),
        access,
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn mode_badge(mode: FanMode) -> Span<'static> {
    match mode {
        FanMode::Manual => {
            Span::styled(" MANUAL ", Style::new().fg(Color::Black).bg(MANUAL).bold())
        }
        FanMode::System => Span::styled(
            " SYSTEM ",
            Style::new()
                .fg(Color::Black)
                .bg(Color::Rgb(150, 150, 255))
                .bold(),
        ),
        FanMode::Auto => Span::styled(" AUTO ", Style::new().fg(Color::Black).bg(ACCENT).bold()),
    }
}

fn draw_fan(
    frame: &mut Frame,
    area: Rect,
    fan: &Fan,
    desired: Option<f32>,
    selected: bool,
    linked: bool,
) {
    let marker = if selected { "▶ " } else { "  " };
    let link = if selected && linked {
        Span::styled(" linked ", Style::new().fg(DIM).italic())
    } else {
        Span::raw("")
    };
    let border_style = if selected {
        Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(DIM)
    };
    let block = Block::bordered()
        .border_type(if selected {
            BorderType::Thick
        } else {
            BorderType::Rounded
        })
        .border_style(border_style)
        .title(Line::from(vec![
            Span::styled(
                format!("{marker}{} ", fan.name),
                if selected {
                    Style::new().fg(ACCENT).bold()
                } else {
                    Style::new().fg(Color::White)
                },
            ),
            mode_badge(fan.mode),
            link,
        ]));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let rows = Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).split(inner);

    let mut spans = vec![
        Span::styled(
            format!("{:>5.0}", fan.actual),
            Style::new().fg(Color::White).bold(),
        ),
        Span::styled(" RPM", Style::new().fg(DIM)),
    ];
    if fan.mode == FanMode::Manual {
        spans.push(Span::styled(
            format!("  target {:>5.0}", fan.target),
            Style::new().fg(MANUAL).bold(),
        ));
    } else {
        spans.push(Span::styled(
            format!("  target {:>5.0}", fan.target),
            Style::new().fg(DIM),
        ));
    }
    if let Some(d) = desired
        && (d - fan.target).abs() > 1.0
    {
        spans.push(Span::styled(
            format!(" → {d:.0}"),
            Style::new().fg(WARM).bold(),
        ));
    }
    spans.push(Span::styled(
        format!("    min {:.0} · max {:.0}", fan.min, fan.max),
        Style::new().fg(DIM),
    ));
    frame.render_widget(Paragraph::new(Line::from(spans)), rows[0]);

    let span = (fan.max - fan.min).max(1.0);
    let raw = ((fan.actual - fan.min) / span) as f64;
    let ratio = if raw.is_finite() {
        raw.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let gauge = Gauge::default()
        .ratio(ratio)
        .label(Span::styled(
            format!("{:>4.0} RPM", fan.actual),
            Style::new().fg(Color::White).bold(),
        ))
        .gauge_style(Style::new().fg(rpm_color(ratio)).bg(Color::Rgb(30, 30, 40)));
    frame.render_widget(gauge, rows[1]);
}

fn draw_temps(frame: &mut Frame, area: Rect, app: &App, selected: bool) {
    let marker = if selected { "▶ " } else { "" };
    let block = Block::bordered()
        .border_type(if selected {
            BorderType::Thick
        } else {
            BorderType::Rounded
        })
        .border_style(if selected {
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(DIM)
        })
        .title(Span::styled(
            format!(" {marker}thermals "),
            if selected {
                Style::new().fg(ACCENT).bold()
            } else {
                Style::new().fg(Color::White).bold()
            },
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let Some((key, t)) = &app.temps.hottest else {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "no readable temperature sensors",
                Style::new().fg(DIM).italic(),
            ))),
            inner,
        );
        return;
    };
    let mut spans = vec![
        Span::styled("avg ", Style::new().fg(DIM)),
        Span::styled(
            format!("{:.1}°C", app.temps.avg),
            Style::new().fg(temp_color(app.temps.avg)).bold(),
        ),
        Span::styled("   hottest ", Style::new().fg(DIM)),
        Span::styled(format!("{t:.1}°C"), Style::new().fg(temp_color(*t)).bold()),
        Span::styled(format!(" ({key})"), Style::new().fg(DIM)),
    ];
    spans.push(Span::styled(
        format!("   {} sensors", app.temps.sensor_count()),
        Style::new().fg(DIM),
    ));
    frame.render_widget(Paragraph::new(Line::from(spans)), inner);
}

fn draw_history(frame: &mut Frame, area: Rect, app: &App) {
    if app.selected == app.fans.len() {
        draw_thermal_history(frame, area, app);
    } else {
        draw_rpm_history(frame, area, app);
    }
}

fn draw_rpm_history(frame: &mut Frame, area: Rect, app: &App) {
    let fan = &app.fans[app.selected];
    let history = &app.history[app.selected];
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(DIM))
        .title(Span::styled(
            format!(" {} — rpm history ", fan.name),
            Style::new().fg(Color::White).bold(),
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let width = inner.width as usize;
    let skip = history.len().saturating_sub(width);
    let data: Vec<u64> = history.iter().copied().skip(skip).collect();
    let spark = Sparkline::default()
        .data(data)
        .max(fan.max.max(1.0) as u64)
        .style(Style::new().fg(ACCENT));
    frame.render_widget(spark, inner);
}

fn draw_thermal_history(frame: &mut Frame, area: Rect, app: &App) {
    let title = match &app.temps.hottest {
        Some((key, temperature)) => {
            format!(" thermal history · hottest {temperature:.1}°C ({key}) ")
        }
        None => " thermal history ".into(),
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(DIM))
        .title(Span::styled(title, Style::new().fg(Color::White).bold()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let Some((_, temperature)) = &app.temps.hottest else {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "no temperature history available",
                Style::new().fg(DIM).italic(),
            ))),
            inner,
        );
        return;
    };
    let width = inner.width as usize;
    let skip = app.thermal_history.len().saturating_sub(width);
    let data: Vec<u64> = app.thermal_history.iter().copied().skip(skip).collect();
    let spark = Sparkline::default()
        .data(data)
        .max(1_150)
        .style(Style::new().fg(temp_color(*temperature)));
    frame.render_widget(spark, inner);
}

fn sort_badge(label: &'static str, active: bool) -> Span<'static> {
    if active {
        Span::styled(
            format!(" {label} "),
            Style::new().fg(Color::Black).bg(ACCENT).bold(),
        )
    } else {
        Span::styled(format!(" {label} "), Style::new().fg(DIM))
    }
}

fn draw_processes(frame: &mut Frame, area: Rect, app: &App) {
    let title = Line::from(vec![
        Span::styled(" top processes  ", Style::new().fg(Color::White).bold()),
        sort_badge("c CPU", app.process_sort == ProcessSort::Cpu),
        Span::raw(" "),
        sort_badge("g GPU", app.process_sort == ProcessSort::Gpu),
        Span::raw(" "),
    ]);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(DIM))
        .title(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if let Some(error) = &app.process_error {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(error, Style::new().fg(HOT)))),
            inner,
        );
        return;
    }
    if app.processes.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "collecting process activity...",
                Style::new().fg(DIM).italic(),
            ))),
            inner,
        );
        return;
    }
    if app.process_sort == ProcessSort::Gpu && !app.gpu_processes_available {
        let reason = app
            .gpu_process_error
            .as_deref()
            .unwrap_or("per-process GPU data is unavailable on this Mac");
        let message = format!("{reason}; press c for CPU");
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(message, Style::new().fg(WARM)))),
            inner,
        );
        return;
    }

    let mut processes: Vec<_> = app.processes.iter().collect();
    processes.sort_by(|a, b| {
        let primary = match app.process_sort {
            ProcessSort::Cpu => b.cpu_percent.total_cmp(&a.cpu_percent),
            ProcessSort::Gpu => b
                .gpu_percent
                .unwrap_or(0.0)
                .total_cmp(&a.gpu_percent.unwrap_or(0.0)),
        };
        primary
            .then_with(|| b.cpu_percent.total_cmp(&a.cpu_percent))
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.pid.cmp(&b.pid))
    });

    let header = Row::new(["#", "PID", "PROCESS", "CPU", "GPU"])
        .style(Style::new().fg(DIM).add_modifier(Modifier::BOLD));
    let rows = processes.into_iter().enumerate().map(|(index, process)| {
        let gpu = process
            .gpu_percent
            .map(|value| format!("{value:.1}%"))
            .unwrap_or_else(|| "n/a".into());
        Row::new(vec![
            Cell::from(format!("{}", index + 1)),
            Cell::from(process.pid.to_string()),
            Cell::from(process.name.as_str()),
            Cell::from(format!("{:.1}%", process.cpu_percent)),
            Cell::from(gpu),
        ])
    });
    let table = Table::new(
        rows,
        [
            Constraint::Length(3),
            Constraint::Length(7),
            Constraint::Fill(1),
            Constraint::Length(8),
            Constraint::Length(8),
        ],
    )
    .header(header)
    .column_spacing(1);
    frame.render_widget(table, inner);
}

fn draw_status(frame: &mut Frame, area: Rect, app: &App) {
    let line = if let Some(status) = &app.status {
        if status.error {
            Line::from(Span::styled(
                format!(" ⚠ {}", status.text),
                Style::new().fg(HOT).bold(),
            ))
        } else {
            Line::from(Span::styled(
                format!(" ◌ {}", status.text),
                Style::new().fg(WARM),
            ))
        }
    } else if !app.is_root {
        Line::from(Span::styled(
            " read-only mode — run `sudo macfan` to control fans",
            Style::new().fg(WARM),
        ))
    } else {
        Line::from(Span::styled(
            " fans restore to auto on quit (q) — use Q to keep settings",
            Style::new().fg(DIM),
        ))
    };
    frame.render_widget(Paragraph::new(line), area);
}

fn draw_help(frame: &mut Frame, area: Rect) {
    let help = Line::from(Span::styled(
        " ↑↓ panel   c/g process sort   ←→ ±100   ⇧←→ ±500   m mode   a auto   f max   space link   q restore   Q keep",
        Style::new().fg(DIM),
    ));
    frame.render_widget(Paragraph::new(help), area);
}
