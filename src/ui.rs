use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use std::time::Duration;
use tui_textarea::TextArea;

use crate::agent::{self, Session, StoredMessage};
use crate::model;

pub async fn run_chat(session: &mut Session) -> Result<i32> {
    let mut terminal = ratatui::init();
    let mut app = ChatApp::new(session);
    loop {
        terminal.draw(|frame| app.draw(frame, session))?;
        if !event::poll(Duration::from_millis(50))? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match app.handle_key(key) {
            ChatAction::None => {}
            ChatAction::Quit => {
                ratatui::restore();
                return Ok(0);
            }
            ChatAction::Command(command) => {
                ratatui::restore();
                match crate::cli::handle_chat_command(session, &command).await? {
                    true => terminal = ratatui::init(),
                    false => return Ok(0),
                }
            }
            ChatAction::Submit(prompt) => {
                ratatui::restore();
                let answer = agent::run_prompt(session, &prompt).await?;
                app.status = preview_line(&answer);
                terminal = ratatui::init();
                app.scroll_to_bottom(session);
            }
        }
    }
}

pub fn choose_model(current: Option<&str>, items: &[String]) -> Result<Option<String>> {
    if items.is_empty() {
        return Ok(None);
    }
    let mut terminal = ratatui::init();
    let mut app = ModelPicker::new(current, items);
    loop {
        terminal.draw(|frame| app.draw(frame))?;
        if !event::poll(Duration::from_millis(50))? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match app.handle_key(key) {
            PickerAction::None => {}
            PickerAction::Cancel => {
                ratatui::restore();
                return Ok(None);
            }
            PickerAction::Select(value) => {
                ratatui::restore();
                return Ok(Some(value));
            }
        }
    }
}

pub fn ask(question: &str, choices: Option<&[String]>) -> Result<String> {
    let mut terminal = ratatui::init();
    let mut app = AskApp::new(question, choices);
    loop {
        terminal.draw(|frame| app.draw(frame))?;
        if !event::poll(Duration::from_millis(50))? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match app.handle_key(key) {
            AskAction::None => {}
            AskAction::Cancel => {
                ratatui::restore();
                return Ok(String::new());
            }
            AskAction::Submit(value) => {
                ratatui::restore();
                return Ok(value);
            }
        }
    }
}

enum ChatAction {
    None,
    Submit(String),
    Command(String),
    Quit,
}

struct ChatApp {
    input: TextArea<'static>,
    status: String,
    scroll: u16,
}

impl ChatApp {
    fn new(session: &Session) -> Self {
        let mut input = TextArea::default();
        input.set_block(Block::default().borders(Borders::ALL).title("Input"));
        let mut app = Self {
            input,
            status: format!(
                "model={} agent={} Enter=send Shift+Enter=newline Ctrl+Q=quit",
                model::to_genai_model_spec(&session.model),
                session.agent
            ),
            scroll: 0,
        };
        app.scroll_to_bottom(session);
        app
    }

    fn draw(&mut self, frame: &mut Frame, session: &Session) {
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(8),
                Constraint::Length(4),
            ])
            .split(frame.area());
        frame.render_widget(
            Paragraph::new(self.status.clone())
                .block(Block::default().borders(Borders::ALL).title("oy")),
            layout[0],
        );
        frame.render_widget(
            Paragraph::new(transcript_text(session))
                .block(Block::default().borders(Borders::ALL).title("Transcript"))
                .wrap(Wrap { trim: false })
                .scroll((self.scroll, 0)),
            layout[1],
        );
        frame.render_widget(&self.input, layout[2]);
    }

    fn handle_key(&mut self, key: KeyEvent) -> ChatAction {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('q') {
            return ChatAction::Quit;
        }
        match key.code {
            KeyCode::PageUp => {
                self.scroll = self.scroll.saturating_sub(5);
                ChatAction::None
            }
            KeyCode::PageDown => {
                self.scroll = self.scroll.saturating_add(5);
                ChatAction::None
            }
            KeyCode::Enter if !key.modifiers.contains(KeyModifiers::SHIFT) => {
                let text = self
                    .input
                    .lines()
                    .join(
                        "
",
                    )
                    .trim()
                    .to_string();
                if text.is_empty() {
                    return ChatAction::None;
                }
                self.reset_input();
                if text.starts_with('/') {
                    ChatAction::Command(text)
                } else {
                    ChatAction::Submit(text)
                }
            }
            _ => {
                self.input.input(key);
                ChatAction::None
            }
        }
    }

    fn reset_input(&mut self) {
        self.input = TextArea::default();
        self.input
            .set_block(Block::default().borders(Borders::ALL).title("Input"));
    }

    fn scroll_to_bottom(&mut self, session: &Session) {
        let lines = transcript_line_count(session);
        self.scroll = lines.saturating_sub(10) as u16;
    }
}

fn transcript_text(session: &Session) -> Text<'static> {
    let mut lines = Vec::new();
    for message in &session.transcript.messages {
        match message {
            StoredMessage::User { content } => push_prefixed(&mut lines, "you", content),
            StoredMessage::Assistant { content } => push_prefixed(&mut lines, "oy", content),
            StoredMessage::AssistantToolCalls { tool_calls } => {
                for call in tool_calls {
                    lines.push(Line::styled(
                        format!(
                            "tool> {} {}",
                            call.fn_name,
                            preview_line(&call.fn_arguments.to_string())
                        ),
                        Style::default().add_modifier(Modifier::DIM),
                    ));
                }
            }
            StoredMessage::Tool {
                call_id: _,
                content,
            } => {
                push_prefixed(&mut lines, "tool", &preview_block(content, 24));
            }
        }
        lines.push(Line::from(""));
    }
    if lines.is_empty() {
        lines.push(Line::from("No messages yet. Type a prompt or /help."));
    }
    Text::from(lines)
}

fn transcript_line_count(session: &Session) -> usize {
    session
        .transcript
        .messages
        .iter()
        .map(|message| match message {
            StoredMessage::User { content } | StoredMessage::Assistant { content } => {
                content.lines().count().max(1) + 1
            }
            StoredMessage::AssistantToolCalls { tool_calls } => tool_calls.len() + 1,
            StoredMessage::Tool { content, .. } => content.lines().take(24).count().max(1) + 1,
        })
        .sum()
}

fn push_prefixed(lines: &mut Vec<Line<'static>>, prefix: &str, content: &str) {
    let mut content_lines = content.lines();
    if let Some(first) = content_lines.next() {
        lines.push(Line::from(format!("{prefix}> {first}")));
    }
    for line in content_lines {
        lines.push(Line::from(format!("    {line}")));
    }
}

fn preview_line(text: &str) -> String {
    let out = text.lines().next().unwrap_or_default().trim();
    if out.is_empty() {
        "done".to_string()
    } else {
        out.chars().take(120).collect()
    }
}

fn preview_block(text: &str, max_lines: usize) -> String {
    let mut lines = text.lines().take(max_lines).collect::<Vec<_>>();
    if text.lines().count() > max_lines {
        lines.push("...");
    }
    lines.join(
        "
",
    )
}

enum AskAction {
    None,
    Submit(String),
    Cancel,
}

struct AskApp {
    question: String,
    choices: Vec<String>,
    selected: usize,
    input: TextArea<'static>,
}

impl AskApp {
    fn new(question: &str, choices: Option<&[String]>) -> Self {
        let mut input = TextArea::default();
        input.set_block(Block::default().borders(Borders::ALL).title("Answer"));
        Self {
            question: question.to_string(),
            choices: choices.unwrap_or(&[]).to_vec(),
            selected: 0,
            input,
        }
    }

    fn draw(&mut self, frame: &mut Frame) {
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(5),
                Constraint::Length(4),
            ])
            .split(frame.area());
        frame.render_widget(
            Paragraph::new(self.question.clone())
                .block(Block::default().borders(Borders::ALL).title("Question")),
            layout[0],
        );
        let choice_lines = if self.choices.is_empty() {
            vec![Line::from("Type a response and press Enter. Esc cancels.")]
        } else {
            self.choices
                .iter()
                .enumerate()
                .map(|(idx, item)| {
                    let style = if idx == self.selected {
                        Style::default().add_modifier(Modifier::REVERSED)
                    } else {
                        Style::default()
                    };
                    Line::styled(item.clone(), style)
                })
                .collect::<Vec<_>>()
        };
        frame.render_widget(
            Paragraph::new(Text::from(choice_lines))
                .block(Block::default().borders(Borders::ALL).title("Choices"))
                .wrap(Wrap { trim: false }),
            layout[1],
        );
        frame.render_widget(&self.input, layout[2]);
    }

    fn handle_key(&mut self, key: KeyEvent) -> AskAction {
        match key.code {
            KeyCode::Esc => AskAction::Cancel,
            KeyCode::Up => {
                self.selected = self.selected.saturating_sub(1);
                AskAction::None
            }
            KeyCode::Down => {
                if self.selected + 1 < self.choices.len() {
                    self.selected += 1;
                }
                AskAction::None
            }
            KeyCode::Enter if self.choices.is_empty() => AskAction::Submit(
                self.input
                    .lines()
                    .join(
                        "
",
                    )
                    .trim()
                    .to_string(),
            ),
            KeyCode::Enter => {
                let typed = self
                    .input
                    .lines()
                    .join(
                        "
",
                    )
                    .trim()
                    .to_string();
                if !typed.is_empty() {
                    AskAction::Submit(typed)
                } else {
                    AskAction::Submit(self.choices.get(self.selected).cloned().unwrap_or_default())
                }
            }
            _ => {
                self.input.input(key);
                AskAction::None
            }
        }
    }
}

enum PickerAction {
    None,
    Select(String),
    Cancel,
}

struct ModelPicker {
    items: Vec<String>,
    filtered: Vec<usize>,
    selected: usize,
    query: TextArea<'static>,
}

impl ModelPicker {
    fn new(current: Option<&str>, items: &[String]) -> Self {
        let mut query = TextArea::default();
        query.set_block(Block::default().borders(Borders::ALL).title("Filter"));
        let current_idx = current
            .and_then(|value| items.iter().position(|item| item == value))
            .unwrap_or(0);
        Self {
            items: items.to_vec(),
            filtered: (0..items.len()).collect(),
            selected: current_idx.min(items.len().saturating_sub(1)),
            query,
        }
    }

    fn draw(&mut self, frame: &mut Frame) {
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(4), Constraint::Min(10)])
            .split(frame.area());
        frame.render_widget(&self.query, layout[0]);
        let lines = if self.filtered.is_empty() {
            vec![Line::from("No matches. Edit the filter or press Esc.")]
        } else {
            self.filtered
                .iter()
                .enumerate()
                .map(|(row, idx)| {
                    let style = if row == self.selected {
                        Style::default().add_modifier(Modifier::REVERSED)
                    } else {
                        Style::default()
                    };
                    Line::styled(self.items[*idx].clone(), style)
                })
                .collect::<Vec<_>>()
        };
        frame.render_widget(
            Paragraph::new(Text::from(lines))
                .block(Block::default().borders(Borders::ALL).title("Pick a model"))
                .wrap(Wrap { trim: false }),
            layout[1],
        );
    }

    fn handle_key(&mut self, key: KeyEvent) -> PickerAction {
        match key.code {
            KeyCode::Esc => PickerAction::Cancel,
            KeyCode::Up => {
                self.selected = self.selected.saturating_sub(1);
                PickerAction::None
            }
            KeyCode::Down => {
                if self.selected + 1 < self.filtered.len() {
                    self.selected += 1;
                }
                PickerAction::None
            }
            KeyCode::Enter => self
                .filtered
                .get(self.selected)
                .and_then(|idx| self.items.get(*idx))
                .cloned()
                .map(PickerAction::Select)
                .unwrap_or(PickerAction::None),
            _ => {
                self.query.input(key);
                let needle = self
                    .query
                    .lines()
                    .join(
                        "
",
                    )
                    .to_ascii_lowercase();
                self.filtered = self
                    .items
                    .iter()
                    .enumerate()
                    .filter(|(_, item)| item.to_ascii_lowercase().contains(&needle))
                    .map(|(idx, _)| idx)
                    .collect();
                self.selected = 0;
                PickerAction::None
            }
        }
    }
}
