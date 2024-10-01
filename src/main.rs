use std::{fs, io, iter, mem};
use anyhow::Context;
use chrono::Local;
use crossterm::cursor::position;
use crossterm::event;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{DefaultTerminal, Frame};
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::symbols::border;
use ratatui::text::{Line, Span, Text, ToSpan};
use ratatui::widgets::block::Title;
use ratatui::widgets::{Block, List, ListDirection, ListItem, Paragraph, Widget};
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

    let solution = wordle_api_response["solution"].to_string().to_uppercase();
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

#[derive(Debug)]
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

    fn color_line(&self, guess: &Vec<ParsedLetter>) -> Line {
        let colored_spans = guess.iter()
            .map(|g|
                Span::from(g.letter.to_string()).style(Style::default().fg(g.position.color()))
            )
            .collect::<Vec<Span>>();

        Line::from(colored_spans)
    }

    fn parse_guess(&self, guess: &str) -> Vec<ParsedLetter> {
        guess.chars()
            .enumerate()
            .map(|(i, c)| ParsedLetter {
                letter: c,
                position: match self.solution.chars().nth(i) {
                    Some(char_at_pos) if char_at_pos == c => LetterPosition::Correct,

                    _ => match self.solution.contains(c) {
                        true => LetterPosition::WrongPlacement,
                        false => LetterPosition::None
                    },
                },
            })
            .collect::<Vec<ParsedLetter>>()
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

        let parsed_guess = self.parse_guess(&g);
        self.guesses_parsed.push(parsed_guess);

        if self.solution == g || self.guesses_parsed.len() == 6 {
            self.exit = true;
        }
    }

    fn color_from_known_information(&self, input: &str) -> Line {
        let best_guess_options = iter::repeat(LetterPosition::None)
            .take(input.len())
            .collect::<Vec<LetterPosition>>();

        for guessed_word in self.guesses_parsed {
            for (parsed_letter_index, parsed_letter) in guessed_word.iter().enumerate() {
                if !input.contains(&parsed_letter.letter) {
                    continue;
                }

                for (input_letter_index, input_letter) in best_guess_options.iter().enumerate() {
                    if best_guess_options[input_letter_index] == LetterPosition::Correct {
                        // already best
                        continue;
                    }

                    if parsed_letter != input_letter {
                        continue;
                    }

                    if input_letter_index == parsed_letter_index {
                        best_guess_options[input_letter_index] = LetterPosition::Correct;
                    } else if best_guess_options[input_letter_index] == LetterPosition::None {
                        best_guess_options[input_letter_index] = LetterPosition::WrongPlacement;
                    }
                }
            }
        }
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
            .map(|g| ListItem::new(self.color_line(&g)))
            .collect();

        let guess_list = List::new(guesses)
            .direction(ListDirection::TopToBottom)
            .block(Block::bordered());

        frame.render_widget(guess_list, guess_area);

        let parsed_current_input = self.parse_guess(&self.current_guess_input);
        let input = Paragraph::new(
            self.color_line(&parsed_current_input)
        );

        frame.render_widget(input, input_area);
        frame.set_cursor_position(Position::new(
            input_area.x + self.current_guess_input.len() as u16,
            input_area.y + 1,
        ));
    }
}
