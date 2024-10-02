use anyhow::{bail, Context};
use chrono::Local;
use crossterm::event::{
    self,
    Event,
    KeyCode,
    KeyEvent,
    KeyEventKind,
    KeyModifiers,
};
use ratatui::{
    widgets::{Block, List, ListDirection, ListItem, Paragraph},
    text::{Line, Span},
    style::{Color, Style},
    layout::{Constraint, Layout, Position},
    DefaultTerminal,
    Frame,
};
use std::collections::HashMap;
use std::{fs, mem};
use ureq::{
    serde_json,
    serde_json::Value,
};

fn main() {
    main_wrapped().unwrap();
}

fn main_wrapped() -> anyhow::Result<()> {
    let date_string = Local::now().format("%Y-%m-%d");

    let wordle_api_response = ureq::get(&format!("https://www.nytimes.com/svc/wordle/v2/{date_string}.json"))
        .call()
        .context("Error querying wordle API")?
        .into_json::<Value>()?;

    let Value::String(solution) = &wordle_api_response["solution"] else {
        bail!("solution value was not type of string");
    };

    let word_list;

    if let Ok(word_list_cache) = fs::read_to_string(".word-list.cache.txt") {
        word_list = word_list_cache.split('\n')
            .map(ToString::to_string)
            .collect::<Vec<String>>();
    } else {
        println!("fetching word list...");

        word_list = fetch_word_list()?
            .into_iter()
            .map(|s| s.to_uppercase())
            .collect::<Vec<String>>();
        fs::write(".word-list.cache.txt", word_list.join("\n"))?;
    }

    let mut terminal = ratatui::init();

    let mut app = App {
        solution: solution.to_owned().to_uppercase(),
        word_list,
        guesses: Vec::new(),
        known_positions: HashMap::new(),
        bad_characters: Vec::new(),
        current_guess_input: String::new(),
        exit: false,
    };

    app.run(&mut terminal)?;
    ratatui::restore();

    for guess in app.guesses {
        println!("{}", guess.iter()
            .map(|(_, p)| p.unwrap_or(LetterPosition::None).emoji())
            .collect::<String>()
        );
    }
    Ok(())
}

fn fetch_word_list() -> anyhow::Result<Vec<String>> {
    let res = ureq::get("https://www.nytimes.com/games-assets/v2/9673.7e73cdd39fb6121fa17d.js")
        .call()?
        .into_string()?;

    let mut array_parts = res.splitn(2, "const o=[");
    let _ = array_parts.next();

    let array_start_json = array_parts.next().context("could not find array start")?;

    let mut array_full_parts = array_start_json.splitn(2, ']');
    let array_end_json = array_full_parts.next().context("could not find array end")?;

    serde_json::from_str::<Vec<String>>(&format!("[{array_end_json}]"))
        .map_err(|e| anyhow::anyhow!(e))
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
            Self::Correct => '🟩'
        }
    }

    const fn color(self) -> Color {
        match self {
            Self::None => Color::DarkGray,
            Self::WrongPlacement => Color::LightYellow,
            Self::Correct => Color::LightGreen
        }
    }
}

#[derive(Debug)]
struct App {
    solution: String,
    word_list: Vec<String>,

    guesses: Vec<Vec<(char, Option<LetterPosition>)>>,
    known_positions: HashMap<usize, Vec<(char, LetterPosition)>>,
    bad_characters: Vec<char>,

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
        if self.current_guess_input.len() != 5 || !self.word_list.contains(&self.current_guess_input) {
            return;
        }

        let mut g = String::new();
        mem::swap(&mut g, &mut self.current_guess_input);

        let mut parsed_guess = g.chars()
            .map(|c| (c, None))
            .collect::<Vec<(char, Option<LetterPosition>)>>();

        for (index, letter) in g.char_indices() {
            // add to bad characters if irrelevant
            if !self.solution.contains(letter) {
                if !self.bad_characters.contains(&letter) {
                    self.bad_characters.push(letter);
                }
                continue;
            }

            if self.solution.as_bytes()[index] == letter as u8 {
                parsed_guess[index].1 = Some(LetterPosition::Correct);
                continue;
            }
        }

        for (index, letter) in g.char_indices() {
            if !self.solution.contains(letter) || self.solution.as_bytes()[index] == letter as u8 {
                continue;
            }

            let solution_letter_occurrences = self.solution.chars().filter(|c| c == &letter).count();
            let existing_letter_occurrences = parsed_guess.iter()
                .filter(|(c, m)| c == &letter && m.is_some())
                .count();

            if solution_letter_occurrences > existing_letter_occurrences {
                parsed_guess[index].1 = Some(LetterPosition::WrongPlacement);
            }
        }

        // finally use the learned information to add to knowledge base
        parsed_guess.iter()
            .filter_map(|(l, p_opt)| p_opt.as_ref().map(|p| (l, p)))
            .enumerate()
            .for_each(|(index, (letter, position))| {
                self.known_positions.entry(index).or_default()
                    .push((*letter, *position));
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

                match self.known_positions.get(&input_index) {
                    Some(known_char_info) if known_char_info.iter().any(|(l, p)| l == &input_char && p == &LetterPosition::Correct) =>
                        (input_char, Some(LetterPosition::Correct)),
                    _ => (input_char, None)
                }
            })
            .map(|(input_char, input_position)| {
                let color = input_position.map_or(Color::White, LetterPosition::color);

                Span::from(input_char.to_string()).style(Style::default().fg(color))
            })
            .collect::<Vec<Span>>();

        Line::from(span_chars)
    }

    fn draw(&self, frame: &mut Frame) {
        let vertical = Layout::vertical([
            Constraint::Percentage(60),
            Constraint::Percentage(10),
        ]);
        let [guess_area, input_area] = vertical.areas(frame.area());

        let guesses: Vec<ListItem> = self
            .guesses
            .iter()
            .map(|letters| {
                let colored_spans = letters.iter()
                    .map(|(c, p)|
                        Span::from(c.to_string())
                            .style(Style::default().fg(p.unwrap_or(LetterPosition::None).color()))
                    )
                    .collect::<Vec<Span>>();

                ListItem::new(Line::from(colored_spans).centered())
            })
            .collect();

        let guess_list = List::new(guesses)
            .direction(ListDirection::TopToBottom)
            .block(Block::bordered());

        frame.render_widget(guess_list, guess_area);

        let input = Paragraph::new(self.color_from_known_information(&self.current_guess_input)).centered();

        frame.render_widget(input, input_area);

        #[allow(clippy::cast_possible_truncation)]
        frame.set_cursor_position(Position::new(
            input_area.x + self.current_guess_input.len() as u16,
            input_area.y,
        ));
    }
}
