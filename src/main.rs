use anyhow::{bail, Context};
use chrono::Local;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{
    layout::{Constraint, Layout},
    style::{Color, Style, Stylize},
    text::{Line, Span},
    widgets::{List, ListItem, Paragraph},
    DefaultTerminal, Frame,
};
use std::{
    collections::{HashMap, HashSet},
    fs, mem,
};
use ureq::serde_json::{self, Value};

fn main() -> anyhow::Result<()> {
    let date_string = Local::now().format("%Y-%m-%d");

    let wordle_api_response = ureq::get(&format!(
        "https://www.nytimes.com/svc/wordle/v2/{date_string}.json"
    ))
        .call()
        .context("Error querying wordle API")?
        .into_json::<Value>()?;

    let Value::String(solution) = &wordle_api_response["solution"] else {
        bail!("solution value was not type of string");
    };

    let mut word_list;

    if let Ok(word_list_cache) = fs::read_to_string(".word-list.cache.txt") {
        word_list = word_list_cache
            .lines()
            .map(ToString::to_string)
            .collect::<Vec<String>>();
    } else {
        println!("fetching word list...");

        word_list = fetch_word_list()?;
        word_list.iter_mut().for_each(|s| *s = s.to_uppercase());

        fs::write(".word-list.cache.txt", word_list.join("\n"))?;
    }

    if let Ok(play_cache) = fs::read_to_string(".play.state.txt") {
        let mut lines = play_cache.lines().collect::<Vec<&str>>();
        if !lines.is_empty() && lines.remove(0) == solution {
            println!("you already played today\n{}", lines.join("\n"));
            return Ok(());
        }
    }

    let mut terminal = ratatui::init();

    let mut app = App {
        solution: solution.to_owned().to_uppercase(),
        word_list,
        guesses: Vec::new(),
        known_positions: HashMap::new(),
        bad_characters: HashSet::new(),
        current_guess_input: String::new(),
        exit: false,
    };

    app.run(&mut terminal)?;
    ratatui::restore();

    let emojis = app
        .guesses
        .iter()
        .map(|guess| {
            guess
                .iter()
                .map(|(_, p)| p.unwrap_or(LetterPosition::None).emoji())
                .collect::<String>()
        })
        .collect::<Vec<String>>();

    println!("{}", emojis.join("\n"));

    if emojis.len() == 6 || // used all guesses
        app.guesses.last().is_some_and(|guess|
            guess.iter().all(|(_, p)| p == &Some(LetterPosition::Correct))
        )
    {
        // got correct answer
        fs::write(
            ".play.state.txt",
            format!("{solution}\n{}", emojis.join("\n")),
        )?;
    }

    Ok(())
}

fn fetch_word_list() -> anyhow::Result<Vec<String>> {
    let res = ureq::get("https://www.nytimes.com/games-assets/v2/9673.7e73cdd39fb6121fa17d.js")
        .call()?
        .into_string()?;

    let (array_json, _) = res
        .split_once("const o=[")
        .and_then(|(_, p)| p.split_once(']'))
        .context("failed to split array string")?;

    serde_json::from_str::<Vec<String>>(&format!("[{array_json}]"))
        .context("failed to parse array json")
}

#[derive(Debug, Clone, Eq, PartialEq, Copy)]
enum LetterPosition {
    None,
    WrongPlacement,
    Correct,
}

impl LetterPosition {
    const fn emoji(self) -> char {
        match self {
            Self::None => '⬜',
            Self::WrongPlacement => '🟨',
            Self::Correct => '🟩',
        }
    }

    const fn color(self) -> Color {
        match self {
            Self::None => Color::DarkGray,
            Self::WrongPlacement => Color::LightYellow,
            Self::Correct => Color::LightGreen,
        }
    }
}

#[derive(Debug)]
struct App {
    solution: String,
    word_list: Vec<String>,

    guesses: Vec<Vec<(char, Option<LetterPosition>)>>,
    known_positions: HashMap<usize, Vec<(char, LetterPosition)>>,
    bad_characters: HashSet<char>,

    current_guess_input: String,

    exit: bool,
}

impl App {
    fn run(&mut self, terminal: &mut DefaultTerminal) -> anyhow::Result<()> {
        while !self.exit {
            terminal.draw(|frame| self.draw(frame))?;
            self.handle_events()?;
        }

        Ok(())
    }

    fn handle_events(&mut self) -> anyhow::Result<()> {
        let e = event::read()?;
        let Event::Key(key_event) = e else {
            return Ok(());
        };

        if key_event.kind == KeyEventKind::Press {
            self.handle_key_event(key_event);
        }

        Ok(())
    }

    fn handle_key_event(&mut self, key_event: KeyEvent) {
        if key_event.modifiers == KeyModifiers::CONTROL && key_event.code == KeyCode::Char('c') {
            self.exit = true;
            return;
        }

        match key_event.code {
            KeyCode::Enter => self.submit_guess(),
            KeyCode::Backspace => {
                let _ = self.current_guess_input.pop();
            }
            KeyCode::Char(c) => {
                if self.current_guess_input.len() < 5 && c.is_alphabetic() {
                    self.current_guess_input.push(c.to_ascii_uppercase());
                }
            }
            _ => {}
        }
    }

    fn submit_guess(&mut self) {
        if self.current_guess_input.len() != 5
            || !self.word_list.contains(&self.current_guess_input)
        {
            return;
        }

        let g = mem::take(&mut self.current_guess_input);

        let mut parsed_guess = g
            .chars()
            .map(|c| (c, None))
            .collect::<Vec<(char, Option<LetterPosition>)>>();

        let contains_letter = |letter| self.solution.contains(letter);

        for (index, letter) in g.char_indices() {
            // add to bad characters if irrelevant
            if !contains_letter(letter) {
                self.bad_characters.insert(letter);
                continue;
            }

            if self.solution.as_bytes()[index] == letter as u8 {
                parsed_guess[index].1 = Some(LetterPosition::Correct);
                continue;
            }
        }

        for (index, letter) in g.char_indices() {
            if !contains_letter(letter) || self.solution.as_bytes()[index] == letter as u8 {
                continue;
            }

            let solution_letter_occurrences =
                self.solution.chars().filter(|c| c == &letter).count();
            let existing_letter_occurrences = parsed_guess
                .iter()
                .filter(|(c, m)| c == &letter && m.is_some())
                .count();

            if solution_letter_occurrences > existing_letter_occurrences {
                parsed_guess[index].1 = Some(LetterPosition::WrongPlacement);
            }
        }

        // finally use the learned information to add to knowledge base
        parsed_guess
            .iter()
            .enumerate()
            .filter_map(|(i, &(l, pos_opt))| pos_opt.map(|pos| (i, (l, pos))))
            .for_each(|(index, (letter, position))| {
                self.known_positions
                    .entry(index)
                    .or_default()
                    .push((letter, position));
            });

        self.guesses.push(parsed_guess);

        if self.solution.eq_ignore_ascii_case(&g) || self.guesses.len() == 6 {
            self.exit = true;
        }
    }

    fn color_from_known_information(&self, input: &str) -> Line {
        let span_chars = input
            .char_indices()
            .map(|(input_index, input_char)| {
                if self.bad_characters.contains(&input_char) {
                    return (input_char, Some(LetterPosition::None));
                }

                if let Some(known_char_info) = self.known_positions.get(&input_index) {
                    if known_char_info.iter().any(|(l, p)| {
                        l.eq_ignore_ascii_case(&input_char) && p == &LetterPosition::Correct
                    }) {
                        return (input_char, Some(LetterPosition::Correct));
                    }
                }

                (input_char, None)
            })
            .map(|(input_char, input_position)| {
                let color = input_position.map_or(Color::White, LetterPosition::color);
                Span::from(input_char.to_string()).style(Style::default().fg(color))
            })
            .collect::<Vec<Span>>();

        Line::from(span_chars)
    }

    fn draw(&self, frame: &mut Frame) {
        let layout = Layout::default()
            .direction(ratatui::layout::Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(1),
                Constraint::Length(3),
            ])
            .split(frame.area());

        let title = Paragraph::new("WORDLE")
            .style(Style::default().fg(Color::LightBlue).dim())
            .centered();
        frame.render_widget(title, layout[0]);

        let guesses: Vec<ListItem> = self
            .guesses
            .iter()
            .map(|letters| {
                let colored_spans = letters
                    .iter()
                    .map(|(c, p)| {
                        Span::from(c.to_string())
                            .style(Style::default().fg(p.unwrap_or(LetterPosition::None).color()))
                    })
                    .collect::<Vec<Span>>();

                ListItem::new(Line::from(colored_spans).centered())
            })
            .collect();

        let guesses_list = List::new(guesses)
            .style(Style::default().fg(Color::White))
            .highlight_style(Style::default().fg(Color::Yellow))
            .highlight_symbol(">");

        frame.render_widget(guesses_list, layout[1]);

        let input = Paragraph::new(self.color_from_known_information(&self.current_guess_input))
            .left_aligned();
        frame.render_widget(input, layout[2]);

        #[allow(clippy::cast_possible_truncation)]
        frame.set_cursor_position((self.current_guess_input.len() as u16, layout[2].y));
    }
}
