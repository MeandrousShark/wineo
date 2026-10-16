//! Terminal prompts for the guided flows: numbered choices, visible defaults,
//! and ordinary line input. What the user types arrives through `Input`, so a
//! flow can be driven by a script in tests as well as by a terminal.

use std::fmt;
use std::io::{self, BufRead, IsTerminal, Write};
use std::os::fd::{AsFd, BorrowedFd};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use rustix::termios::{LocalModes, OptionalActions, Termios, tcgetattr, tcsetattr};
use rustyline::error::ReadlineError;
use rustyline::{
    Cmd, ConditionalEventHandler, Config, DefaultEditor, Event as KeyBindingEvent, EventContext,
    EventHandler, KeyCode, KeyEvent, Modifiers, Movement, Word,
};
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

impl<T> Reply<T> {
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Reply<U> {
        match self {
            Reply::Value(value) => Reply::Value(f(value)),
            Reply::Back => Reply::Back,
        }
    }
}

/// A line the user typed, a back key, or the end of input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    Text(String),
    Back,
    End,
}

pub trait Input {
    /// The next answer, asked for with `prompt`.
    fn line(&mut self, prompt: &str) -> io::Result<Line>;

    /// Whether `line` shows the prompt itself, as a line editor has to so it
    /// can redraw the line. Otherwise the prompt is printed first.
    fn shows_prompt(&self) -> bool {
        false
    }

    /// Drops interrupts something else already acted on, like a Ctrl+C that
    /// canceled an installer, so they don't cancel the next question too.
    fn forget_interrupts(&mut self) {}

    /// Whether an interrupt came in since the last question, which tells a
    /// failure it caused, like a prefix creation cut short, from any other.
    fn interrupted(&mut self) -> bool {
        false
    }
}

/// The prompt every guided command opens on the terminal.
pub type Ask = Prompt<Terminal, crate::output::Stream<io::Stdout>>;

/// Opens an `Ask` at its flow's first question, where going back cancels.
pub fn terminal() -> anyhow::Result<Ask> {
    Ok(Prompt::new(Terminal::open()?, crate::output::stdout()).with_first_question(true))
}

/// Input for a flow that only prints, like a review that never asks.
pub struct NoInput;

impl Input for NoInput {
    fn line(&mut self, _: &str) -> io::Result<Line> {
        unreachable!("this flow has no prompts")
    }
}

/// Whether both ends are a terminal, which every guided flow needs.
pub fn interactive() -> bool {
    // Unit tests inherit the terminal cargo runs in, as under makepkg, and
    // would wait on it for an answer.
    !cfg!(test) && io::stdin().is_terminal() && io::stdout().is_terminal()
}

/// Reads one line without echoing it, for a secret like an API key. A signal
/// puts the terminal back before wineo exits, so echo is never left off.
/// Piped input is read as is, without a prompt. None is the end of input.
/// At a terminal where echo can't be turned off, nothing is read.
pub fn secret(label: &str) -> anyhow::Result<Option<String>> {
    read_stdin_secret(label, true)
}

/// Like `secret`, for a password: only the line ending is removed, so
/// spaces around it are part of the password.
pub fn password(label: &str) -> anyhow::Result<Option<String>> {
    read_stdin_secret(label, false)
}

fn read_stdin_secret(label: &str, trim: bool) -> anyhow::Result<Option<String>> {
    let stdin = io::stdin();
    // Plain stdout, not the wrapper that exits on a closed pipe: that would
    // skip putting the terminal back. A closed pipe takes the usual exit
    // once echo is restored.
    let result = read_secret_with(
        stdin.as_fd(),
        stdin.is_terminal(),
        &mut stdin.lock(),
        &mut io::stdout(),
        label,
        &Hooks::real(),
        trim,
    );
    if let Err(error) = &result
        && let Some(error) = error.downcast_ref::<io::Error>()
        && error.kind() == io::ErrorKind::BrokenPipe
    {
        crate::output::handle(Err(io::Error::from(io::ErrorKind::BrokenPipe)));
    }
    result
}

/// Sets a terminal's modes.
type SetModes = Arc<dyn Fn(BorrowedFd<'_>, &Termios) -> rustix::io::Result<()> + Send + Sync>;

/// The parts of reading a secret a test replaces.
struct Hooks {
    set_modes: SetModes,
    /// Ends the process once a signal has put the terminal back.
    exit: Arc<dyn Fn() + Send + Sync>,
    /// Runs between saving the terminal's modes and turning echo off.
    before_echo_off: Arc<dyn Fn() + Send + Sync>,
    /// The signals that restore the terminal.
    signals: Vec<i32>,
}

impl Hooks {
    fn real() -> Self {
        Self {
            set_modes: Arc::new(|fd, modes| tcsetattr(fd, OptionalActions::Now, modes)),
            exit: Arc::new(|| {
                println!();
                std::process::exit(i32::from(crate::cancel::EXIT))
            }),
            before_echo_off: Arc::new(|| {}),
            signals: vec![SIGINT, SIGTERM, SIGHUP],
        }
    }
}

/// Whether a signal has taken over and is ending the process.
type Leaving = Arc<std::sync::Mutex<bool>>;

fn locked(leaving: &Leaving) -> std::sync::MutexGuard<'_, bool> {
    leaving
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
fn read_secret(
    fd: BorrowedFd<'_>,
    terminal: bool,
    input: &mut dyn BufRead,
    out: &mut dyn Write,
    label: &str,
    hooks: &Hooks,
) -> anyhow::Result<Option<String>> {
    read_secret_with(fd, terminal, input, out, label, hooks, true)
}

/// `trim` removes all surrounding whitespace; without it, only the line
/// ending goes.
fn read_secret_with(
    fd: BorrowedFd<'_>,
    terminal: bool,
    input: &mut dyn BufRead,
    out: &mut dyn Write,
    label: &str,
    hooks: &Hooks,
    trim: bool,
) -> anyhow::Result<Option<String>> {
    let mut restore_on_exit = None;
    if terminal {
        let saved = tcgetattr(fd)
            .map_err(|_| anyhow::anyhow!("Can't turn off echo here, so nothing was read."))?;
        // A signal puts the terminal back before wineo exits. Changing the
        // terminal's modes and that restore-then-exit share one lock, which
        // the signal thread keeps until the process is gone, so a signal can
        // never restore the modes and leave echo to be turned off afterwards.
        let leaving = Leaving::default();
        let mut signals = Signals::new(hooks.signals.iter().copied())?;
        let guard = signals.handle();
        let for_thread = fd.try_clone_to_owned()?;
        {
            let (leaving, restore) = (leaving.clone(), saved.clone());
            let (set_modes, exit) = (hooks.set_modes.clone(), hooks.exit.clone());
            thread::spawn(move || {
                if signals.forever().next().is_some() {
                    let mut leaving = locked(&leaving);
                    *leaving = true;
                    let _ = set_modes(for_thread.as_fd(), &restore);
                    exit();
                }
            });
        }
        (hooks.before_echo_off)();
        let mut quiet = saved.clone();
        quiet.local_modes.remove(LocalModes::ECHO);
        {
            let leaving = locked(&leaving);
            if *leaving {
                guard.close();
                return Err(Canceled.into());
            }
            if (hooks.set_modes)(fd, &quiet).is_err() {
                guard.close();
                anyhow::bail!("Can't turn off echo here, so nothing was read.");
            }
        }
        restore_on_exit = Some((saved, guard, leaving));
        // Only now is it safe to ask, so a paste that arrives at once
        // doesn't show.
        let shown = write!(out, "{label}").and_then(|()| out.flush());
        if let Err(error) = shown {
            put_back(fd, restore_on_exit.take(), out, hooks);
            return Err(error.into());
        }
    }
    let mut line = String::new();
    let read = input.read_line(&mut line);
    put_back(fd, restore_on_exit, out, hooks);
    Ok((read? > 0).then(|| {
        if trim {
            line.trim().to_owned()
        } else {
            let line = line.strip_suffix('\n').unwrap_or(&line);
            line.strip_suffix('\r').unwrap_or(line).to_owned()
        }
    }))
}

/// Turns echo back on and ends the signal watch.
fn put_back(
    fd: BorrowedFd<'_>,
    saved: Option<(Termios, signal_hook::iterator::Handle, Leaving)>,
    out: &mut dyn Write,
    hooks: &Hooks,
) {
    if let Some((saved, guard, leaving)) = saved {
        {
            let _held = locked(&leaving);
            let _ = (hooks.set_modes)(fd, &saved);
        }
        let _ = writeln!(out);
        guard.close();
    }
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
    color: bool,
    first_question: bool,
}

impl<I: Input, W: Write> Prompt<I, W> {
    pub fn new(input: I, out: W) -> Self {
        Self {
            input,
            out,
            color: false,
            first_question: false,
        }
    }

    /// Enables bold menu titles, for callers whose `out` is a real,
    /// color-capable terminal.
    pub fn with_color(mut self, enabled: bool) -> Self {
        self.color = enabled;
        self
    }

    pub fn with_first_question(mut self, enabled: bool) -> Self {
        self.first_question = enabled;
        self
    }

    pub fn set_first_question(&mut self, enabled: bool) {
        self.first_question = enabled;
    }

    fn key_hint(&self, back: bool) -> &'static str {
        if self.first_question {
            "Esc or Ctrl+C to cancel."
        } else if back {
            "Esc to go back, Ctrl+C to cancel."
        } else {
            "Ctrl+C to cancel."
        }
    }

    pub fn input(&mut self) -> &mut I {
        &mut self.input
    }

    /// Where the conversation is written, for output that isn't a question.
    pub fn writer(&mut self) -> &mut W {
        &mut self.out
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
        let result = (|| {
            self.menu(heading, choices, back)?;
            loop {
                let answer = match self.ask(label, default)? {
                    Reply::Value(answer) => answer,
                    Reply::Back if back => return Ok(Reply::Back),
                    Reply::Back => continue,
                };
                if let Some(reply) = self.special(&answer, back)? {
                    return Ok(reply);
                }
                match choices.iter().find(|choice| choice.key == answer) {
                    Some(choice) => return Ok(Reply::Value(choice.value.clone())),
                    None => self.say(format!("`{answer}` is not one of the choices"))?,
                }
            }
        })();
        if result.is_ok() {
            self.first_question = false;
        }
        result
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
        let result = (|| {
            self.menu(heading, choices, back)?;
            loop {
                let answer = match self.ask(label, default)? {
                    Reply::Value(answer) => answer,
                    Reply::Back if back => return Ok(Reply::Back),
                    Reply::Back => continue,
                };
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
        })();
        if result.is_ok() {
            self.first_question = false;
        }
        result
    }

    /// A review menu whose numbered entries are edited in place. Enter runs
    /// the named action, while picking an entry returns it to edit.
    pub fn review<T: Clone>(
        &mut self,
        heading: &str,
        note: Option<&str>,
        enter_action: &str,
        choices: &[Choice<T>],
        back: bool,
    ) -> anyhow::Result<Reply<Option<T>>> {
        let result = (|| {
            let label = self.review_menu(heading, note, enter_action, choices, back, "a number")?;
            self.choose_or_enter(choices, back, &label)
        })();
        if result.is_ok() {
            self.first_question = false;
        }
        result
    }

    /// `review` taking any number of entries on one line, like `1 3 4` or
    /// `1,3,4`. A line with anything that isn't one of them is asked again.
    /// Every entry has to be numbered.
    pub fn review_many<T: Clone>(
        &mut self,
        heading: &str,
        note: Option<&str>,
        enter_action: &str,
        choices: &[Choice<T>],
        back: bool,
    ) -> anyhow::Result<Reply<Option<Vec<T>>>> {
        let result = (|| {
            let label = self.review_menu(heading, note, enter_action, choices, back, "numbers")?;
            loop {
                let answer = match self.read(&label)? {
                    Reply::Value(answer) => answer,
                    Reply::Back if back => return Ok(Reply::Back),
                    Reply::Back => continue,
                };
                if answer.is_empty() {
                    return Ok(Reply::Value(None));
                }
                if let Some(reply) = self.special(&answer, back)? {
                    return Ok(reply.map(Some));
                }
                match parse_numbers(&answer, choices.len()) {
                    Ok(picked) => {
                        return Ok(Reply::Value(Some(
                            picked
                                .into_iter()
                                .map(|index| choices[index].value.clone())
                                .collect(),
                        )));
                    }
                    Err(problem) => self.say(problem)?,
                }
            }
        })();
        if result.is_ok() {
            self.first_question = false;
        }
        result
    }

    /// Prints a review menu and returns the label to ask with. `pick` is what
    /// the keys line offers besides Enter.
    fn review_menu<T>(
        &mut self,
        heading: &str,
        note: Option<&str>,
        enter_action: &str,
        choices: &[Choice<T>],
        back: bool,
        pick: &str,
    ) -> anyhow::Result<String> {
        writeln!(
            self.out,
            "{}",
            crate::style::title_with_color(heading, self.color)
        )?;
        for choice in choices {
            writeln!(self.out, "  {}) {}", choice.key, choice.label)?;
        }
        if let Some(note) = note {
            self.say(note)?;
        }
        writeln!(self.out)?;
        let enter = if choices.is_empty() {
            format!("Press Enter to {enter_action},")
        } else {
            format!("Press Enter to {enter_action}, or pick {pick}.")
        };
        let rest = self.key_hint(back);
        self.say(format!("{enter} {rest}"))?;
        // "Choice:" with nothing numbered to choose reads like a missing list.
        Ok(if choices.is_empty() {
            let mut chars = enter_action.chars();
            let first = chars.next().map(|c| c.to_uppercase().to_string());
            format!("{}{}? ", first.unwrap_or_default(), chars.as_str())
        } else {
            "Choice: ".to_owned()
        })
    }

    fn choose_or_enter<T: Clone>(
        &mut self,
        choices: &[Choice<T>],
        back: bool,
        label: &str,
    ) -> anyhow::Result<Reply<Option<T>>> {
        loop {
            let answer = match self.read(label)? {
                Reply::Value(answer) => answer,
                Reply::Back if back => return Ok(Reply::Back),
                Reply::Back => continue,
            };
            if answer.is_empty() {
                return Ok(Reply::Value(None));
            }
            if let Some(reply) = self.special(&answer, back)? {
                return Ok(reply.map(Some));
            }
            match choices.iter().find(|choice| choice.key == answer) {
                Some(choice) => return Ok(Reply::Value(Some(choice.value.clone()))),
                None => self.say(format!("`{answer}` is not one of the choices"))?,
            }
        }
    }

    /// A line of text. Enter takes `default`, and Esc or `<` goes back. `parse`
    /// explains what is wrong with an answer, and the question is asked again.
    pub fn text<T>(
        &mut self,
        label: &str,
        default: Option<&str>,
        mut parse: impl FnMut(&str) -> Result<T, String>,
    ) -> anyhow::Result<Reply<T>> {
        if self.first_question {
            self.say(self.key_hint(false))?;
        }
        let result = (|| {
            loop {
                let answer = match self.ask(label, default)? {
                    Reply::Value(answer) => answer,
                    Reply::Back => return Ok(Reply::Back),
                };
                if answer == "<" {
                    return if self.first_question {
                        Err(Canceled.into())
                    } else {
                        Ok(Reply::Back)
                    };
                }
                match parse(&answer) {
                    Ok(value) => return Ok(Reply::Value(value)),
                    Err(problem) => self.say(problem)?,
                }
            }
        })();
        if result.is_ok() {
            self.first_question = false;
        }
        result
    }

    /// Yes or no.
    /// Only the first question prints its own keys, so a review that ends in
    /// a later `confirm` calls this first to show them once.
    pub fn confirm_hint(&mut self, back: bool) -> io::Result<()> {
        if self.first_question {
            return Ok(());
        }
        self.say(self.key_hint(back))
    }

    pub fn confirm(&mut self, label: &str, default: bool) -> anyhow::Result<Reply<bool>> {
        if self.first_question {
            self.say(self.key_hint(false))?;
        }
        let result = (|| {
            let shown = if default { "Y/n" } else { "y/N" };
            loop {
                let answer = match self.read(&format!("{label} [{shown}]: "))? {
                    Reply::Value(answer) => answer,
                    Reply::Back => return Ok(Reply::Back),
                };
                match answer.to_ascii_lowercase().as_str() {
                    "" => return Ok(Reply::Value(default)),
                    "y" | "yes" => return Ok(Reply::Value(true)),
                    "n" | "no" => return Ok(Reply::Value(false)),
                    "<" if self.first_question => return Err(Canceled.into()),
                    "<" => return Ok(Reply::Back),
                    _ => self.say("Answer y or n")?,
                }
            }
        })();
        if result.is_ok() {
            self.first_question = false;
        }
        result
    }

    /// Values one per line, so each keeps its spaces, until an empty line.
    pub fn list(&mut self, item: &str) -> anyhow::Result<Reply<Vec<String>>> {
        if self.first_question {
            self.say(self.key_hint(false))?;
        }
        let result = (|| {
            let mut values = Vec::new();
            loop {
                let answer = match self.read_raw(&format!("  {item} {}: ", values.len() + 1))? {
                    Reply::Value(answer) => answer,
                    Reply::Back => return Ok(Reply::Back),
                };
                if values.is_empty() && answer.trim() == "<" {
                    return if self.first_question {
                        Err(Canceled.into())
                    } else {
                        Ok(Reply::Back)
                    };
                }
                if answer.is_empty() {
                    return Ok(Reply::Value(values));
                }
                values.push(answer);
            }
        })();
        if result.is_ok() {
            self.first_question = false;
        }
        result
    }

    fn menu<T>(&mut self, heading: &str, choices: &[Choice<T>], back: bool) -> io::Result<()> {
        writeln!(
            self.out,
            "{}",
            crate::style::title_with_color(heading, self.color)
        )?;
        for choice in choices {
            writeln!(self.out, "  {}) {}", choice.key, choice.label)?;
        }
        writeln!(self.out, "{}", self.key_hint(back))
    }

    /// Back and cancel, which every menu answers the same way.
    fn special<T>(&mut self, answer: &str, back: bool) -> anyhow::Result<Option<Reply<T>>> {
        match answer {
            "q" => Err(Canceled.into()),
            "b" if back && self.first_question => Err(Canceled.into()),
            "b" if back => Ok(Some(Reply::Back)),
            _ => Ok(None),
        }
    }

    /// Asks with the default shown, and returns the trimmed answer, or the
    /// default for an empty one.
    fn ask(&mut self, label: &str, default: Option<&str>) -> anyhow::Result<Reply<String>> {
        let question = match default {
            Some(default) => format!("{label} [{default}]: "),
            None => format!("{label}: "),
        };
        Ok(self.read(&question)?.map(|answer| {
            if answer.is_empty() {
                default.unwrap_or_default().to_owned()
            } else {
                answer
            }
        }))
    }

    fn read(&mut self, prompt: &str) -> anyhow::Result<Reply<String>> {
        Ok(self
            .read_raw(prompt)?
            .map(|answer| answer.trim().to_owned()))
    }

    /// An answer as typed, without its newline. The end of input cancels.
    fn read_raw(&mut self, prompt: &str) -> anyhow::Result<Reply<String>> {
        if !self.input.shows_prompt() {
            write!(self.out, "{prompt}")?;
            self.out.flush()?;
        }
        match self.input.line(prompt)? {
            Line::Text(text) => Ok(Reply::Value(text)),
            Line::Back if self.first_question => Err(Canceled.into()),
            Line::Back => Ok(Reply::Back),
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

/// The terminal, read a line at a time on request through a line editor, so
/// the arrow keys and word deletion work. Ctrl+C at a question ends the input,
/// and so do SIGTERM and SIGHUP, rather than killing wineo on the spot, so a
/// flow can say what it leaves behind.
pub struct Terminal {
    events: Receiver<Event>,
    requests: Sender<String>,
    /// A read still under way from a question an interrupt cut short.
    pending: bool,
    stash: Option<io::Result<Line>>,
    signals: Handle,
    /// The terminal's settings before any question, put back when this goes,
    /// so leaving in the middle of one never leaves it in raw mode.
    saved: Option<Termios>,
}

enum Event {
    Line(io::Result<Line>),
    Interrupt,
}

impl Terminal {
    pub fn open() -> anyhow::Result<Self> {
        let saved = tcgetattr(io::stdin()).ok();
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
        let (requests, asked) = mpsc::channel::<String>();
        thread::spawn(move || {
            let mut editor = line_editor().ok();
            while let Ok(prompt) = asked.recv() {
                let event = match editor.as_mut() {
                    Some((editor, back_key)) => match editor.readline(&prompt) {
                        Ok(line) => {
                            let _ = editor.add_history_entry(line.as_str());
                            Event::Line(Ok(Line::Text(line)))
                        }
                        Err(ReadlineError::Interrupted | ReadlineError::Signal(_)) => {
                            if back_key.swap(false, Ordering::SeqCst) {
                                Event::Line(Ok(Line::Back))
                            } else {
                                Event::Interrupt
                            }
                        }
                        Err(ReadlineError::Eof) => Event::Line(Ok(Line::End)),
                        Err(err) => Event::Line(Err(io::Error::other(err))),
                    },
                    None => {
                        print!("{prompt}");
                        let _ = crate::output::stdout().flush();
                        let mut line = String::new();
                        Event::Line(io::stdin().read_line(&mut line).map(|read| {
                            if read > 0 {
                                Line::Text(line)
                            } else {
                                Line::End
                            }
                        }))
                    }
                };
                if sender.send(event).is_err() {
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
            saved,
        })
    }
}

struct BackKey(Arc<AtomicBool>);

impl ConditionalEventHandler for BackKey {
    fn handle(&self, _: &KeyBindingEvent, _: u16, _: bool, _: &EventContext) -> Option<Cmd> {
        self.0.store(true, Ordering::SeqCst);
        Some(Cmd::Interrupt)
    }
}

fn line_editor() -> rustyline::Result<(DefaultEditor, Arc<AtomicBool>)> {
    // A lone Esc needs a timeout to separate it from arrow and Alt sequences.
    let config = Config::builder().keyseq_timeout(Some(500)).build();
    let mut editor = DefaultEditor::with_config(config)?;
    let back_key = Arc::new(AtomicBool::new(false));
    editor.bind_sequence(
        KeyEvent(KeyCode::Esc, Modifiers::NONE),
        EventHandler::Conditional(Box::new(BackKey(back_key.clone()))),
    );
    // Terminals sending plain ^H cannot distinguish Ctrl+Backspace from Backspace.
    editor.bind_sequence(
        KeyEvent(KeyCode::Backspace, Modifiers::CTRL),
        EventHandler::Simple(Cmd::Kill(Movement::BackwardWord(1, Word::Emacs))),
    );
    Ok((editor, back_key))
}

impl Input for Terminal {
    fn line(&mut self, prompt: &str) -> io::Result<Line> {
        let read = match self.stash.take() {
            Some(read) => read,
            None => {
                if !self.pending {
                    let _ = self.requests.send(prompt.to_owned());
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
            Line::Text(mut text) => {
                if text.ends_with('\n') {
                    text.pop();
                    if text.ends_with('\r') {
                        text.pop();
                    }
                }
                Line::Text(text)
            }
            other => other,
        })
    }

    fn shows_prompt(&self) -> bool {
        true
    }

    fn forget_interrupts(&mut self) {
        self.interrupted();
    }

    fn interrupted(&mut self) -> bool {
        let mut interrupted = false;
        while let Ok(event) = self.events.try_recv() {
            match event {
                Event::Line(read) => self.stash = Some(read),
                Event::Interrupt => interrupted = true,
            }
        }
        interrupted
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        self.signals.close();
        if let Some(saved) = &self.saved {
            let _ = tcsetattr(io::stdin(), OptionalActions::Now, saved);
        }
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
    fn line(&mut self, _prompt: &str) -> io::Result<Line> {
        Ok(self.0.pop_front().map_or(Line::End, Line::Text))
    }
}

#[cfg(test)]
mod tests;
