use super::*;
use std::collections::VecDeque;

struct Lines(VecDeque<Line>);

impl Input for Lines {
    fn line(&mut self, _prompt: &str) -> io::Result<Line> {
        Ok(self.0.pop_front().unwrap_or(Line::End))
    }
}

fn lines(answers: impl IntoIterator<Item = Line>) -> Prompt<Lines, Vec<u8>> {
    Prompt::new(Lines(answers.into_iter().collect()), Vec::new())
}

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
fn first_question_escape_cancels_even_after_invalid_answers() {
    let mut p = lines([Line::Text("invalid".to_owned()), Line::Back]).with_first_question(true);
    let error = p
        .choose("Pick:", "Choice", &menu(), None, false)
        .unwrap_err();
    assert!(error.is::<Canceled>());
    let output = String::from_utf8_lossy(p.output());
    assert!(output.contains("Esc or Ctrl+C to cancel."));
    assert!(!output.contains("Esc to go back"));
}

#[test]
fn a_confirm_shows_its_keys_once_whether_or_not_it_comes_first() {
    let mut p = prompt(&[""]).with_first_question(true);
    p.confirm_hint(false).unwrap();
    assert_eq!(p.confirm("Go?", true).unwrap(), Reply::Value(true));
    assert_eq!(printed(&p), "Esc or Ctrl+C to cancel.\nGo? [Y/n]: ");

    let mut p = prompt(&[""]);
    p.confirm_hint(true).unwrap();
    assert_eq!(p.confirm("Go?", true).unwrap(), Reply::Value(true));
    assert_eq!(
        printed(&p),
        "Esc to go back, Ctrl+C to cancel.\nGo? [Y/n]: "
    );
}

#[test]
fn first_question_mode_clears_after_an_answer_and_later_escape_goes_back() {
    let mut p = lines([Line::Text("1".to_owned()), Line::Back]).with_first_question(true);
    assert_eq!(
        p.choose("First:", "Choice", &menu(), None, false).unwrap(),
        Reply::Value("none")
    );
    assert_eq!(
        p.choose("Next:", "Choice", &menu(), None, true).unwrap(),
        Reply::Back
    );
    let output = String::from_utf8_lossy(p.output());
    assert!(output.contains("Esc or Ctrl+C to cancel."));
    assert!(output.contains("Esc to go back, Ctrl+C to cancel."));
}

#[test]
fn typed_back_fallbacks_cancel_at_the_first_question() {
    let mut menu_prompt = prompt(&["b"]).with_first_question(true);
    let error = menu_prompt
        .choose("First:", "Choice", &menu(), None, true)
        .unwrap_err();
    assert!(error.is::<Canceled>());

    let mut text = prompt(&["<"]).with_first_question(true);
    let error = text
        .text("First", None, |answer| Ok(answer.to_owned()))
        .unwrap_err();
    assert!(error.is::<Canceled>());

    let mut confirm = prompt(&["<"]).with_first_question(true);
    let error = confirm.confirm("First", false).unwrap_err();
    assert!(error.is::<Canceled>());

    let mut list = prompt(&["<"]).with_first_question(true);
    let error = list.list("First").unwrap_err();
    assert!(error.is::<Canceled>());
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
        "Choose a template:\n  1) None, choose each setting\n  2) gaming\n  p) enter a path\nEsc to go back, Ctrl+C to cancel.\nTemplate [1]: "
    );
}

#[test]
fn menu_heading_uses_the_prompt_color_setting() {
    let mut colored = prompt(&["1"]).with_color(true);
    colored
        .choose("Choose:", "Template", &menu(), None, false)
        .unwrap();
    assert!(printed(&colored).contains("\u{1b}[1mChoose:\u{1b}[0m"));

    let mut plain = prompt(&["1"]).with_color(false);
    plain
        .choose("Choose:", "Template", &menu(), None, false)
        .unwrap();
    assert!(printed(&plain).starts_with("Choose:\n"));
    assert!(!printed(&plain).contains('\u{1b}'));
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
    assert!(printed(&p).contains("Ctrl+C to cancel.\n"));
    assert!(!printed(&p).contains("b) back"));

    // Running out of input is Ctrl+D, which cancels too.
    let err = prompt(&[])
        .choose("Pick:", "Template", &menu(), None, true)
        .unwrap_err();
    assert!(err.is::<Canceled>());
}

#[test]
fn back_key_returns_from_prompts_and_is_ignored_at_a_root_menu() {
    let mut p = lines([Line::Back]);
    assert_eq!(
        p.choose("Pick:", "Template", &menu(), None, true).unwrap(),
        Reply::Back
    );

    let mut p = lines([Line::Back, Line::Text("1".to_owned())]);
    assert_eq!(
        p.choose("Pick:", "Template", &menu(), None, false).unwrap(),
        Reply::Value("none")
    );

    let mut p = lines([Line::Back]);
    assert_eq!(
        p.text("Name", None, |s| Ok::<_, String>(s.to_owned()))
            .unwrap(),
        Reply::Back
    );

    let mut p = lines([Line::Back]);
    assert_eq!(p.confirm("Continue?", true).unwrap(), Reply::Back);

    let mut p = lines([Line::Text("one".to_owned()), Line::Back]);
    assert_eq!(p.list("argument").unwrap(), Reply::Back);
}

#[test]
fn review_accepts_enter_and_prints_note_above_keys() {
    let mut p = prompt(&[""]);
    let reply = p
        .review(
            "Review:",
            Some("Close the game first."),
            "import",
            &menu(),
            true,
        )
        .unwrap();
    assert_eq!(reply, Reply::Value(None));
    assert_eq!(
        printed(&p),
        "Review:\n  1) None, choose each setting\n  2) gaming\n  p) enter a path\nClose the game first.\n\nPress Enter to import, or pick a number. Esc to go back, Ctrl+C to cancel.\nChoice: "
    );
}

#[test]
fn review_picks_a_numbered_entry_to_edit() {
    let mut p = prompt(&["2"]);
    let reply = p
        .review("Review:", None, "save them", &menu(), true)
        .unwrap();
    assert_eq!(reply, Reply::Value(Some("gaming")));
}

#[test]
fn review_supports_an_empty_list_without_back() {
    let mut p = prompt(&[""]);
    let reply = p
        .review::<()>("Review:", None, "create it", &[], false)
        .unwrap();
    assert_eq!(reply, Reply::Value(None));
    assert_eq!(
        printed(&p),
        "Review:\n\nPress Enter to create it, Ctrl+C to cancel.\nCreate it? "
    );
}

#[test]
fn review_many_takes_several_numbers_on_a_line() {
    let choices = numbered([
        ("one".to_owned(), 1),
        ("two".to_owned(), 2),
        ("three".to_owned(), 3),
        ("four".to_owned(), 4),
    ]);
    for line in ["1 3 4", "1,3,4", "1, 3 4"] {
        let mut p = prompt(&[line]);
        let reply = p
            .review_many("Review:", Some("Note."), "fix them", &choices, false)
            .unwrap();
        assert_eq!(reply, Reply::Value(Some(vec![1, 3, 4])), "{line}");
    }
    let mut p = prompt(&["2"]);
    let reply = p
        .review_many("Review:", None, "fix them", &choices, false)
        .unwrap();
    assert_eq!(reply, Reply::Value(Some(vec![2])));
    assert_eq!(
        printed(&p),
        "Review:\n  1) one\n  2) two\n  3) three\n  4) four\n\nPress Enter to fix them, or pick numbers. Ctrl+C to cancel.\nChoice: "
    );
}

#[test]
fn review_many_asks_again_after_a_bad_token() {
    let choices = numbered([("one".to_owned(), 1), ("two".to_owned(), 2)]);
    let mut p = prompt(&["1 5", "1 x", ""]);
    let reply = p
        .review_many("Review:", None, "fix them", &choices, false)
        .unwrap();
    assert_eq!(reply, Reply::Value(None));
    let text = printed(&p);
    assert!(text.contains("Pick numbers between 1 and 2\n"), "{text}");
    assert!(text.contains("`1 x` is not one of the choices"), "{text}");
}

#[test]
fn review_keeps_typed_back_and_cancel() {
    let mut p = prompt(&["b"]);
    assert_eq!(
        p.review("Review:", None, "done", &menu(), true).unwrap(),
        Reply::Back
    );
    let mut p = prompt(&["q"]);
    let err = p
        .review("Review:", None, "done", &menu(), true)
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
    assert_eq!(printed(&p), "Installer: ");
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
    assert!(text.contains("Name [hades]: "), "{text}");
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
    assert!(printed(&p).contains("Desktop entry? [y/N]: "));
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

/// A terminal that records whether echo was on each time it is written.
struct Probe {
    terminal: std::fs::File,
    echo_when_written: Vec<bool>,
}

impl Write for Probe {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let modes = tcgetattr(&self.terminal).unwrap().local_modes;
        self.echo_when_written
            .push(modes.contains(LocalModes::ECHO));
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Tests that watch signals take turns, since a signal reaches every
/// reader at once.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The real thing, except that a signal no other test listens for stands
/// in for Ctrl+C and the process isn't ended.
fn hooks() -> Hooks {
    Hooks {
        exit: Arc::new(|| {}),
        signals: vec![signal_hook::consts::SIGUSR2],
        ..Hooks::real()
    }
}

fn echo_is_on(terminal: &std::fs::File) -> bool {
    tcgetattr(terminal)
        .unwrap()
        .local_modes
        .contains(LocalModes::ECHO)
}

fn pty_slave() -> (rustix::fd::OwnedFd, std::fs::File) {
    use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};
    let master = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY).unwrap();
    grantpt(&master).unwrap();
    unlockpt(&master).unwrap();
    let name = ptsname(&master, Vec::new()).unwrap();
    let slave = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(name.to_str().unwrap())
        .unwrap();
    (master, slave)
}

#[test]
fn a_secret_turns_echo_off_before_the_prompt_is_shown() {
    let _serial = serial();
    let (_master, slave) = pty_slave();
    let mut probe = Probe {
        terminal: slave.try_clone().unwrap(),
        echo_when_written: Vec::new(),
    };
    let mut input = io::Cursor::new(b"pasted\n".to_vec());
    let read = read_secret(
        slave.as_fd(),
        true,
        &mut input,
        &mut probe,
        "Key: ",
        &hooks(),
    )
    .unwrap();
    assert_eq!(read.as_deref(), Some("pasted"));
    // The label went out with echo off, and the newline after it is back
    // on.
    assert_eq!(probe.echo_when_written, [false, true]);
    // And it stays on afterwards.
    let modes = tcgetattr(&slave).unwrap().local_modes;
    assert!(modes.contains(LocalModes::ECHO));
}

#[test]
fn a_secret_is_not_read_when_echo_cant_be_turned_off() {
    // Not a terminal, though told to treat it as one.
    let not_a_terminal = std::fs::File::open("/dev/null").unwrap();
    let mut input = io::Cursor::new(b"pasted\n".to_vec());
    let mut out = Vec::new();
    let error = read_secret(
        not_a_terminal.as_fd(),
        true,
        &mut input,
        &mut out,
        "Key: ",
        &hooks(),
    )
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Can't turn off echo here, so nothing was read."
    );
    assert!(out.is_empty(), "no prompt was shown");
    assert_eq!(input.position(), 0, "nothing was read");
}

#[test]
fn piped_input_is_read_as_is_without_a_prompt() {
    let not_a_terminal = std::fs::File::open("/dev/null").unwrap();
    let mut input = io::Cursor::new(b"  piped  \n".to_vec());
    let mut out = Vec::new();
    let read = read_secret(
        not_a_terminal.as_fd(),
        false,
        &mut input,
        &mut out,
        "Key: ",
        &hooks(),
    )
    .unwrap();
    assert_eq!(read.as_deref(), Some("piped"));
    assert!(out.is_empty());
}

#[test]
fn a_secret_is_not_read_when_the_terminal_wont_take_the_change() {
    let _serial = serial();
    let (_master, slave) = pty_slave();
    let mut input = io::Cursor::new(b"pasted\n".to_vec());
    let mut out = Vec::new();
    // Reading modes works, but turning echo off is refused.
    let refuse_quiet = Hooks {
        set_modes: Arc::new(|_, modes| {
            if modes.local_modes.contains(LocalModes::ECHO) {
                Ok(())
            } else {
                Err(rustix::io::Errno::IO)
            }
        }),
        ..hooks()
    };
    let error = read_secret(
        slave.as_fd(),
        true,
        &mut input,
        &mut out,
        "Key: ",
        &refuse_quiet,
    )
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Can't turn off echo here, so nothing was read."
    );
    assert!(out.is_empty(), "no prompt was shown");
    assert_eq!(input.position(), 0, "nothing was read");
    assert!(echo_is_on(&slave));
}

#[test]
fn echo_comes_back_when_the_prompt_cant_be_written() {
    struct Closed;
    impl Write for Closed {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::ErrorKind::BrokenPipe.into())
        }
        fn flush(&mut self) -> io::Result<()> {
            Err(io::ErrorKind::BrokenPipe.into())
        }
    }
    let _serial = serial();
    let (_master, slave) = pty_slave();
    let mut input = io::Cursor::new(b"pasted\n".to_vec());
    let error = read_secret(
        slave.as_fd(),
        true,
        &mut input,
        &mut Closed,
        "Key: ",
        &hooks(),
    )
    .unwrap_err();
    let error = error.downcast::<io::Error>().unwrap();
    assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    assert_eq!(input.position(), 0, "nothing was read");
    assert!(echo_is_on(&slave));
}

#[test]
fn echo_comes_back_when_reading_fails() {
    struct Broken;
    impl io::Read for Broken {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::ErrorKind::Other.into())
        }
    }
    impl BufRead for Broken {
        fn fill_buf(&mut self) -> io::Result<&[u8]> {
            Err(io::ErrorKind::Other.into())
        }
        fn consume(&mut self, _: usize) {}
    }
    let _serial = serial();
    let (_master, slave) = pty_slave();
    let mut out = Vec::new();
    let error = read_secret(
        slave.as_fd(),
        true,
        &mut Broken,
        &mut out,
        "Key: ",
        &hooks(),
    )
    .unwrap_err();
    assert_eq!(
        error.downcast::<io::Error>().unwrap().kind(),
        io::ErrorKind::Other
    );
    assert!(echo_is_on(&slave));
}

#[test]
fn a_signal_before_echo_goes_off_stops_it_going_off_afterwards() {
    use std::sync::atomic::AtomicBool;
    use std::time::{Duration, Instant};

    let _serial = serial();
    let (_master, slave) = pty_slave();
    let exited = Arc::new(AtomicBool::new(false));
    let echo_set_to = Arc::new(std::sync::Mutex::new(Vec::new()));
    let hooks = Hooks {
        set_modes: {
            let echo_set_to = echo_set_to.clone();
            Arc::new(move |fd, modes| {
                echo_set_to
                    .lock()
                    .unwrap()
                    .push(modes.local_modes.contains(LocalModes::ECHO));
                tcsetattr(fd, OptionalActions::Now, modes)
            })
        },
        exit: {
            let exited = exited.clone();
            Arc::new(move || exited.store(true, Ordering::SeqCst))
        },
        // The signal arrives after the modes were saved and before echo
        // would be turned off, and is handled completely first.
        before_echo_off: {
            let exited = exited.clone();
            Arc::new(move || {
                signal_hook::low_level::raise(signal_hook::consts::SIGUSR2).unwrap();
                let deadline = Instant::now() + Duration::from_secs(10);
                while !exited.load(Ordering::SeqCst) {
                    assert!(Instant::now() < deadline, "the signal wasn't handled");
                    thread::sleep(Duration::from_millis(2));
                }
            })
        },
        ..hooks()
    };
    let mut input = io::Cursor::new(b"pasted\n".to_vec());
    let mut out = Vec::new();
    let error =
        read_secret(slave.as_fd(), true, &mut input, &mut out, "Key: ", &hooks).unwrap_err();
    assert!(error.is::<Canceled>());
    // Only the signal's restore touched the terminal: echo was never
    // turned off for it to be left that way.
    assert_eq!(*echo_set_to.lock().unwrap(), [true]);
    assert!(echo_is_on(&slave));
    assert!(out.is_empty(), "no prompt was shown");
    assert_eq!(input.position(), 0, "nothing was read");
}

#[test]
fn a_signal_while_a_secret_is_read_restores_echo() {
    use std::sync::atomic::AtomicBool;
    use std::time::{Duration, Instant};

    let _serial = serial();
    let (_master, slave) = pty_slave();
    let exited = Arc::new(AtomicBool::new(false));
    let echo_when_exited = Arc::new(std::sync::Mutex::new(None));
    let terminal = slave.try_clone().unwrap();
    let hooks = Hooks {
        exit: {
            let (exited, echo_when_exited) = (exited.clone(), echo_when_exited.clone());
            Arc::new(move || {
                *echo_when_exited.lock().unwrap() = Some(echo_is_on(&terminal));
                exited.store(true, Ordering::SeqCst);
            })
        },
        ..hooks()
    };
    // Reading waits for a line that arrives after the signal.
    let (mut reader, mut writer) = io::pipe().unwrap();
    let mut input = io::BufReader::new(&mut reader);
    let signaller = thread::spawn({
        let exited = exited.clone();
        move || {
            thread::sleep(Duration::from_millis(100));
            signal_hook::low_level::raise(signal_hook::consts::SIGUSR2).unwrap();
            let deadline = Instant::now() + Duration::from_secs(10);
            while !exited.load(Ordering::SeqCst) && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(2));
            }
            writer.write_all(b"late\n").unwrap();
        }
    });
    let mut out = Vec::new();
    let read = read_secret(slave.as_fd(), true, &mut input, &mut out, "Key: ", &hooks);
    signaller.join().unwrap();
    assert!(read.is_ok());
    assert_eq!(*echo_when_exited.lock().unwrap(), Some(true));
    assert!(echo_is_on(&slave));
}
