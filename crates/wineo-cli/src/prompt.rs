//! Terminal prompts for the guided flows: numbered choices, visible defaults,
//! and ordinary line input. What the user types arrives through `Input`, so a
//! flow can be driven by a script in tests as well as by a terminal.

use std::fmt;
use std::io::{self, IsTerminal, Write};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
use signal_hook::iterator::{Handle, Signals};

/// Ends a guided flow early, on Ctrl+C, Ctrl+D, or `q`. Carried as an error so
/// `?` unwinds the flow, and turned into exit code 130 at the top.
#[derive(Debug)]
pub struct Canceled;

impl fmt::Display for Canceled {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("canceled")
    }
}

impl std::error::Error for Canceled {}

/// What answering a question came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply<T> {
    Value(T),
    /// Go back to the question before.
    Back,
}

/// A line the user typed, or the end of input: EOF or an interrupt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    Text(String),
    End,
}

pub trait Input {
    fn line(&mut self) -> io::Result<Line>;

    /// Drops interrupts something else already acted on, like a Ctrl+C that
    /// canceled an installer, so they don't cancel the next question too.
    fn forget_interrupts(&mut self) {}
}

/// Whether both ends are a terminal, which every guided flow needs.
pub fn interactive() -> bool {
    io::stdin().is_terminal() && io::stdout().is_terminal()
}

/// One entry of a menu. Keys are numbers for the usual entries and letters
/// for actions, like `p` for entering a path.
#[derive(Debug, Clone)]
pub struct Choice<T> {
    key: String,
    label: String,
    value: T,
}

impl<T> Choice<T> {
    pub fn letter(key: char, label: impl Into<String>, value: T) -> Self {
        assert!(!matches!(key, 'b' | 'q'), "`b` and `q` are back and cancel");
        Self {
            key: key.to_string(),
            label: label.into(),
            value,
        }
    }
}

/// Numbers `entries` from 1, as the numbered part of a menu.
pub fn numbered<T>(entries: impl IntoIterator<Item = (String, T)>) -> Vec<Choice<T>> {
    entries
        .into_iter()
        .enumerate()
        .map(|(index, (label, value))| Choice {
            key: (index + 1).to_string(),
            label,
            value,
        })
        .collect()
}

/// What `choose_many` picked: numbered entries, or one lettered action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Picked<T> {
    Many(Vec<T>),
    One(T),
}

pub struct Prompt<I, W> {
    input: I,
    out: W,
}

impl<I: Input, W: Write> Prompt<I, W> {
    pub fn new(input: I, out: W) -> Self {
        Self { input, out }
    }

    pub fn input(&mut self) -> &mut I {
        &mut self.input
    }

    #[cfg(test)]
    pub fn output(&self) -> &W {
        &self.out
    }

    /// Prints a line of the conversation.
    pub fn say(&mut self, text: impl fmt::Display) -> io::Result<()> {
        writeln!(self.out, "{text}")
    }

    /// One entry from a menu. `back` offers going back, which the first
    /// question of a flow has nowhere to go to.
    pub fn choose<T: Clone>(
        &mut self,
        heading: &str,
        label: &str,
        choices: &[Choice<T>],
        default: Option<&str>,
        back: bool,
    ) -> anyhow::Result<Reply<T>> {
        self.menu(heading, choices, back)?;
        loop {
            let answer = self.ask(label, default)?;
            if let Some(reply) = self.special(&answer, back)? {
                return Ok(reply);
            }
            match choices.iter().find(|choice| choice.key == answer) {
                Some(choice) => return Ok(Reply::Value(choice.value.clone())),
                None => self.say(format!("`{answer}` is not one of the choices"))?,
            }
        }
    }

    /// Any number of the numbered entries, like `1 3` or `2-4`, or a single
    /// lettered one.
    pub fn choose_many<T: Clone>(
        &mut self,
        heading: &str,
        label: &str,
        choices: &[Choice<T>],
        default: Option<&str>,
        back: bool,
    ) -> anyhow::Result<Reply<Picked<T>>> {
        self.menu(heading, choices, back)?;
        loop {
            let answer = self.ask(label, default)?;
            if let Some(reply) = self.special(&answer, back)? {
                return Ok(reply);
            }
            if let Some(choice) = choices
                .iter()
                .find(|choice| choice.key == answer && !is_number(&choice.key))
            {
                return Ok(Reply::Value(Picked::One(choice.value.clone())));
            }
            let numbered: Vec<&Choice<T>> = choices
                .iter()
                .filter(|choice| is_number(&choice.key))
                .collect();
            match parse_numbers(&answer, numbered.len()) {
                Ok(picked) => {
                    return Ok(Reply::Value(Picked::Many(
                        picked
                            .into_iter()
                            .map(|index| numbered[index].value.clone())
                            .collect(),
                    )));
                }
                Err(problem) => self.say(problem)?,
            }
        }
    }

    /// A line of text. Enter takes `default`, and `<` goes back. `parse`
    /// explains what is wrong with an answer, and the question is asked again.
    pub fn text<T>(
        &mut self,
        label: &str,
        default: Option<&str>,
        mut parse: impl FnMut(&str) -> Result<T, String>,
    ) -> anyhow::Result<Reply<T>> {
        let label = format!("{label}, or < to go back");
        loop {
            let answer = self.ask(&label, default)?;
            if answer == "<" {
                return Ok(Reply::Back);
            }
            match parse(&answer) {
                Ok(value) => return Ok(Reply::Value(value)),
                Err(problem) => self.say(problem)?,
            }
        }
    }

    /// Yes or no.
    pub fn confirm(&mut self, label: &str, default: bool) -> anyhow::Result<Reply<bool>> {
        let shown = if default { "Y/n" } else { "y/N" };
        loop {
            write!(self.out, "{label} [{shown}], or < to go back: ")?;
            self.out.flush()?;
            let answer = self.read()?;
            match answer.to_ascii_lowercase().as_str() {
                "" => return Ok(Reply::Value(default)),
                "y" | "yes" => return Ok(Reply::Value(true)),
                "n" | "no" => return Ok(Reply::Value(false)),
                "<" => return Ok(Reply::Back),
                _ => self.say("Answer y or n")?,
            }
        }
    }

    /// Values one per line, so each keeps its spaces, until an empty line.
    pub fn list(&mut self, item: &str) -> anyhow::Result<Reply<Vec<String>>> {
        let mut values = Vec::new();
        loop {
            write!(self.out, "  {item} {}: ", values.len() + 1)?;
            self.out.flush()?;
            let answer = self.read_raw()?;
            if values.is_empty() && answer.trim() == "<" {
                return Ok(Reply::Back);
            }
            if answer.is_empty() {
                return Ok(Reply::Value(values));
            }
            values.push(answer);
        }
    }

    fn menu<T>(&mut self, heading: &str, choices: &[Choice<T>], back: bool) -> io::Result<()> {
        writeln!(self.out, "{heading}")?;
        for choice in choices {
            writeln!(self.out, "  {}) {}", choice.key, choice.label)?;
        }
        if back {
            writeln!(self.out, "  b) back   q) cancel")
        } else {
            writeln!(self.out, "  q) cancel")
        }
    }

    /// Back and cancel, which every menu answers the same way.
    fn special<T>(&mut self, answer: &str, back: bool) -> anyhow::Result<Option<Reply<T>>> {
        match answer {
            "q" => Err(Canceled.into()),
            "b" if back => Ok(Some(Reply::Back)),
            _ => Ok(None),
        }
    }

    /// Asks with the default shown, and returns the trimmed answer, or the
    /// default for an empty one.
    fn ask(&mut self, label: &str, default: Option<&str>) -> anyhow::Result<String> {
        match default {
            Some(default) => write!(self.out, "{label} [{default}]: ")?,
            None => write!(self.out, "{label}: ")?,
        }
        self.out.flush()?;
        let answer = self.read()?;
        Ok(match (answer.is_empty(), default) {
            (true, Some(default)) => default.to_owned(),
            _ => answer,
        })
    }

    fn read(&mut self) -> anyhow::Result<String> {
        Ok(self.read_raw()?.trim().to_owned())
    }

    /// A line as typed, without its newline. The end of input cancels.
    fn read_raw(&mut self) -> anyhow::Result<String> {
        match self.input.line()? {
            Line::Text(text) => Ok(text),
            Line::End => {
                // The cursor is still on the question.
                let _ = writeln!(self.out);
                Err(Canceled.into())
            }
        }
    }
}

fn is_number(key: &str) -> bool {
    key.bytes().all(|byte| byte.is_ascii_digit())
}

/// Zero-based indexes from something like `1 3, 5-7`, in the order given and
/// without repeats.
fn parse_numbers(answer: &str, count: usize) -> Result<Vec<usize>, String> {
    if count == 0 {
        return Err(format!("`{answer}` is not one of the choices"));
    }
    let invalid = || format!("`{answer}` is not one of the choices. Pick numbers like 1 3 or 2-4");
    let mut picked = Vec::new();
    let parts: Vec<&str> = answer
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|part| !part.is_empty())
        .collect();
    if parts.is_empty() {
        return Err(invalid());
    }
    for part in parts {
        let (first, last) = match part.split_once('-') {
            Some((first, last)) => (first, last),
            None => (part, part),
        };
        let (Ok(first), Ok(last)) = (first.parse::<usize>(), last.parse::<usize>()) else {
            return Err(invalid());
        };
        if first == 0 || last < first || last > count {
            return Err(format!("Pick numbers between 1 and {count}"));
        }
        for number in first..=last {
            if !picked.contains(&(number - 1)) {
                picked.push(number - 1);
            }
        }
    }
    Ok(picked)
}

/// The terminal, read a line at a time on request. Ctrl+C at a question ends
/// the input, and so do SIGTERM and SIGHUP, rather than killing wineo on the
/// spot, so a flow can say what it leaves behind.
pub struct Terminal {
    events: Receiver<Event>,
    requests: Sender<()>,
    /// A read still under way from a question an interrupt cut short.
    pending: bool,
    stash: Option<io::Result<Option<String>>>,
    signals: Handle,
}

enum Event {
    Line(io::Result<Option<String>>),
    Interrupt,
}

impl Terminal {
    pub fn open() -> anyhow::Result<Self> {
        let mut signals = Signals::new([SIGINT, SIGTERM, SIGHUP])?;
        let handle = signals.handle();
        let (sender, events) = mpsc::channel();
        let interrupts = sender.clone();
        thread::spawn(move || {
            for _ in signals.forever() {
                if interrupts.send(Event::Interrupt).is_err() {
                    break;
                }
            }
        });
        let (requests, asked) = mpsc::channel::<()>();
        thread::spawn(move || {
            while asked.recv().is_ok() {
                let mut line = String::new();
                let read = io::stdin()
                    .read_line(&mut line)
                    .map(|read| (read > 0).then_some(line));
                if sender.send(Event::Line(read)).is_err() {
                    break;
                }
            }
        });
        Ok(Self {
            events,
            requests,
            pending: false,
            stash: None,
            signals: handle,
        })
    }
}

impl Input for Terminal {
    fn line(&mut self) -> io::Result<Line> {
        let read = match self.stash.take() {
            Some(read) => read,
            None => {
                if !self.pending {
                    let _ = self.requests.send(());
                    self.pending = true;
                }
                match self.events.recv() {
                    Ok(Event::Line(read)) => read,
                    Ok(Event::Interrupt) | Err(_) => return Ok(Line::End),
                }
            }
        };
        self.pending = false;
        Ok(match read? {
            Some(mut text) => {
                if text.ends_with('\n') {
                    text.pop();
                    if text.ends_with('\r') {
                        text.pop();
                    }
                }
                Line::Text(text)
            }
            None => Line::End,
        })
    }

    fn forget_interrupts(&mut self) {
        while let Ok(event) = self.events.try_recv() {
            if let Event::Line(read) = event {
                self.stash = Some(read);
            }
        }
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        self.signals.close();
    }
}

/// Answers from a list, for tests. Running out is the end of input.
#[cfg(test)]
pub struct Script(pub std::collections::VecDeque<String>);

#[cfg(test)]
impl Script {
    pub fn new(lines: &[&str]) -> Self {
        Self(lines.iter().map(|line| (*line).to_owned()).collect())
    }
}

#[cfg(test)]
impl Input for Script {
    fn line(&mut self) -> io::Result<Line> {
        Ok(self.0.pop_front().map_or(Line::End, Line::Text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prompt(lines: &[&str]) -> Prompt<Script, Vec<u8>> {
        Prompt::new(Script::new(lines), Vec::new())
    }

    fn printed(prompt: &Prompt<Script, Vec<u8>>) -> String {
        String::from_utf8_lossy(prompt.output()).into_owned()
    }

    fn menu() -> Vec<Choice<&'static str>> {
        let mut choices = numbered([
            ("None, choose each setting".to_owned(), "none"),
            ("gaming".to_owned(), "gaming"),
        ]);
        choices.push(Choice::letter('p', "enter a path", "path"));
        choices
    }

    #[test]
    fn a_menu_lists_its_choices_and_takes_the_default() {
        let mut p = prompt(&[""]);
        let reply = p
            .choose("Choose a template:", "Template", &menu(), Some("1"), true)
            .unwrap();
        assert_eq!(reply, Reply::Value("none"));
        assert_eq!(
            printed(&p),
            "Choose a template:\n  1) None, choose each setting\n  2) gaming\n  p) enter a path\n  b) back   q) cancel\nTemplate [1]: "
        );
    }

    #[test]
    fn a_wrong_answer_is_explained_and_asked_again() {
        let mut p = prompt(&["7", "gaming", "2"]);
        let reply = p.choose("Pick:", "Template", &menu(), None, true).unwrap();
        assert_eq!(reply, Reply::Value("gaming"));
        let text = printed(&p);
        assert!(text.contains("`7` is not one of the choices\n"), "{text}");
        assert!(
            text.contains("`gaming` is not one of the choices\n"),
            "{text}"
        );
        assert_eq!(text.matches("Template: ").count(), 3, "{text}");
    }

    #[test]
    fn back_and_cancel_answer_every_menu() {
        let mut p = prompt(&["b"]);
        assert_eq!(
            p.choose("Pick:", "Template", &menu(), None, true).unwrap(),
            Reply::Back
        );

        // With nowhere to go back to, `b` is just a wrong answer.
        let mut p = prompt(&["b", "q"]);
        let err = p
            .choose("Pick:", "Template", &menu(), None, false)
            .unwrap_err();
        assert!(err.is::<Canceled>());
        assert!(printed(&p).contains("  q) cancel\n"));
        assert!(!printed(&p).contains("b) back"));

        // Running out of input is Ctrl+D, which cancels too.
        let err = prompt(&[])
            .choose("Pick:", "Template", &menu(), None, true)
            .unwrap_err();
        assert!(err.is::<Canceled>());
    }

    #[test]
    fn several_numbers_or_one_letter() {
        let choices = numbered((1..=5).map(|n| (format!("program {n}"), n)));
        let mut with_path = choices.clone();
        with_path.push(Choice::letter('p', "enter a path", 0));

        let mut p = prompt(&["1 3,4-5 3"]);
        assert_eq!(
            p.choose_many("Programs:", "Add", &with_path, None, true)
                .unwrap(),
            Reply::Value(Picked::Many(vec![1, 3, 4, 5]))
        );
        let mut p = prompt(&["p"]);
        assert_eq!(
            p.choose_many("Programs:", "Add", &with_path, None, true)
                .unwrap(),
            Reply::Value(Picked::One(0))
        );
        let mut p = prompt(&["0", "6", "2-1", "x", "2"]);
        assert_eq!(
            p.choose_many("Programs:", "Add", &with_path, None, true)
                .unwrap(),
            Reply::Value(Picked::Many(vec![2]))
        );
        assert_eq!(
            printed(&p).matches("Pick numbers between 1 and 5").count(),
            3,
            "{}",
            printed(&p)
        );
    }

    #[test]
    fn text_keeps_spaces_and_needs_no_quoting() {
        let mut p = prompt(&["  /home/me/My Games/setup (1).exe  "]);
        let reply = p
            .text("Installer", None, |answer| {
                Ok::<_, String>(answer.to_owned())
            })
            .unwrap();
        assert_eq!(
            reply,
            Reply::Value("/home/me/My Games/setup (1).exe".to_owned())
        );
        assert_eq!(printed(&p), "Installer, or < to go back: ");
    }

    #[test]
    fn text_takes_its_default_validates_and_goes_back() {
        let mut p = prompt(&["", "bad name", "<"]);
        let parse = |answer: &str| {
            if wineo_core::name::is_valid(answer) {
                Ok(answer.to_owned())
            } else {
                Err(format!("`{answer}` is not a valid name"))
            }
        };
        assert_eq!(
            p.text("Name", Some("hades"), parse).unwrap(),
            Reply::Value("hades".to_owned())
        );
        assert_eq!(p.text("Name", Some("hades"), parse).unwrap(), Reply::Back);
        let text = printed(&p);
        assert!(text.contains("Name, or < to go back [hades]: "), "{text}");
        assert!(text.contains("`bad name` is not a valid name\n"), "{text}");
    }

    #[test]
    fn confirm_takes_y_n_and_the_default() {
        let mut p = prompt(&["", "maybe", "N", "<"]);
        assert_eq!(
            p.confirm("Desktop entry?", true).unwrap(),
            Reply::Value(true)
        );
        assert_eq!(
            p.confirm("Desktop entry?", true).unwrap(),
            Reply::Value(false)
        );
        assert_eq!(p.confirm("Desktop entry?", false).unwrap(), Reply::Back);
        assert!(printed(&p).contains("Answer y or n\n"));
        assert!(printed(&p).contains("Desktop entry? [y/N], or < to go back: "));
    }

    #[test]
    fn a_list_keeps_each_value_whole() {
        let mut p = prompt(&["-config", r"C:\My Games\settings.ini", "  spaced  ", ""]);
        assert_eq!(
            p.list("argument").unwrap(),
            Reply::Value(vec![
                "-config".to_owned(),
                r"C:\My Games\settings.ini".to_owned(),
                "  spaced  ".to_owned(),
            ])
        );
        let mut p = prompt(&["<"]);
        assert_eq!(p.list("argument").unwrap(), Reply::Back);
        let mut p = prompt(&[""]);
        assert_eq!(p.list("argument").unwrap(), Reply::Value(Vec::new()));
    }

    #[test]
    fn numbers_parse_in_order_without_repeats() {
        assert_eq!(parse_numbers("3 1", 3).unwrap(), [2, 0]);
        assert_eq!(parse_numbers("1-3,2", 3).unwrap(), [0, 1, 2]);
        assert!(parse_numbers("", 3).is_err());
        assert!(parse_numbers("1-", 3).is_err());
        assert!(parse_numbers("4", 3).is_err());
    }
}
