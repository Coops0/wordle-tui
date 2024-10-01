use anyhow::{bail, Context};
use chrono::Local;
use crossterm::event;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Position};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, List, ListDirection, ListItem, Paragraph};
use ratatui::{DefaultTerminal, Frame};
use std::{fs, iter, mem};
use ureq::serde_json;
use ureq::serde_json::Value;

fn main() {
    main_wrapped().unwrap();
}

fn main_wrapped() -> anyhow::Result<()> {
    let date_string = Local::now().format("%Y-%m-%d");

    let wordle_api_response = ureq::get(&format!("https://www.nytimes.com/svc/wordle/v2/{date_string}.json"))
        .call()
        .context("Error querying wordle API")?
        .into_json::<Value>()?;

    let Value::String(solution) = wordle_api_response["solution"].to_owned() else {
        bail!("solution value was not type of string");
    };

    let mut word_list = Vec::new();

    if let Ok(word_list_cache) = fs::read_to_string(".word-list.cache.txt") {
        word_list = word_list_cache.split("\n")
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
        solution,
        word_list,
        guesses_parsed: Vec::new(),
        current_guess_input: String::new(),
        exit: false,
    };

    app.run(&mut terminal)?;
    ratatui::restore();

    for guess in app.guesses_parsed {
        println!("{}", guess.iter()
            .map(|c| c.position.emoji())
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

    let mut array_full_parts = array_start_json.splitn(2, "]");
    let array_end_json = array_full_parts.next().context("could not find array end")?;

    serde_json::from_str::<Vec<String>>(&format!("[{array_end_json}]"))
        .map_err(|e| anyhow::anyhow!(e))
}

#[derive(Debug, Clone, Eq, PartialEq)]
enum LetterPosition {
    None,
    WrongPlacement,
    Correct,
}

impl LetterPosition {
    fn emoji(&self) -> char {
        match self {
            LetterPosition::None => '⬜',
            LetterPosition::Correct => '🟩',
            LetterPosition::WrongPlacement => '🟨',
        }
    }

    fn color(&self) -> Color {
        match self {
            LetterPosition::None => Color::DarkGray,
            LetterPosition::Correct => Color::LightGreen,
            LetterPosition::WrongPlacement => Color::LightYellow,
        }
    }
}

#[derive(Debug)]
struct ParsedLetter {
    letter: char,
    position: LetterPosition,
}

#[derive(Debug)]
struct App {
    solution: String,
    word_list: Vec<String>,
    guesses_parsed: Vec<Vec<ParsedLetter>>,

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

        if key_event.kind != KeyEventKind::Press {
            return Ok(());
        }

        self.handle_key_event(key_event);
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
                if self.current_guess_input.len() < 5 && c.is_alphanumeric() {
                    self.current_guess_input.push(c.to_ascii_uppercase());
                }
            }
            _ => {}
        }
    }

    fn submit_guess(&mut self) {
        if self.current_guess_input.len() != 5 {
            return;
        }

        if !self.word_list.contains(&self.current_guess_input) {
            return;
        }

        let mut g = String::new();
        mem::swap(&mut g, &mut self.current_guess_input);

        let parsed_guess = g.char_indices()
            .map(|(i, c)| {
                let position_matches = self.solution.char_indices().filter(|(_, cc)| c.eq_ignore_ascii_case(cc));
                let mut position = LetterPosition::None;

                for (matched_position, _) in position_matches {
                    if matched_position == i {
                        position = LetterPosition::Correct;
                        break;
                    }

                    position = LetterPosition::WrongPlacement;
                }

                ParsedLetter {
                    letter: c,
                    position,
                }
            })
            .collect::<Vec<ParsedLetter>>();

        println!("{:?}", parsed_guess);
        println!("{} vs {}", g, self.solution);

        self.guesses_parsed.push(parsed_guess);

        if self.solution.eq_ignore_ascii_case(&g) || self.guesses_parsed.len() == 6 {
            self.exit = true;
        }
    }

    fn color_from_known_information(&self, input: &str) -> Line {
        let mut best_guess_options = iter::repeat(LetterPosition::None)
            .take(input.len())
            .collect::<Vec<LetterPosition>>();

        for guessed_word in &self.guesses_parsed {
            for (parsed_letter_index, parsed_letter) in guessed_word.iter().enumerate() {
                if !input.contains(parsed_letter.letter) {
                    continue;
                }

                for (input_letter_index, input_letter) in best_guess_options.clone().iter().enumerate() {
                    if parsed_letter.letter != input.chars().collect::<Vec<char>>()[input_letter_index] {
                        continue;
                    }

                    if input_letter_index == parsed_letter_index {
                        best_guess_options[input_letter_index] = LetterPosition::Correct;
                    } else if input_letter == &LetterPosition::None {
                        best_guess_options[input_letter_index] = LetterPosition::WrongPlacement;
                    }
                }
            }
        }

        let span_chars = best_guess_options.iter()
            .enumerate()
            .map(|(i, p)| Span::from(input.chars().collect::<Vec<char>>()[i].to_string()).style(Style::default().fg(p.color())))
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
            .guesses_parsed
            .iter()
            .map(|g| {
                let colored_spans = g.iter()
                    .map(|g|
                        Span::from(g.letter.to_string()).style(Style::default().fg(g.position.color()))
                    )
                    .collect::<Vec<Span>>();

                ListItem::new(Line::from(colored_spans))
            })
            .collect();

        let guess_list = List::new(guesses)
            .direction(ListDirection::TopToBottom)
            .block(Block::bordered());

        frame.render_widget(guess_list, guess_area);

        let input = Paragraph::new(self.color_from_known_information(&self.current_guess_input)).centered();

        frame.render_widget(input, input_area);
        frame.set_cursor_position(Position::new(
            input_area.x + self.current_guess_input.len() as u16,
            input_area.y + 1,
        ));
    }
}
