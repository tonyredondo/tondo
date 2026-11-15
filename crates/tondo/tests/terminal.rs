//! A program's terminal (standard library 14.32): `cli.Terminal` takes it
//! and gives it back however the program ends and around Ctrl-Z and `fg`,
//! its keys and its sizes reach the program, and the questions of `std.cli`
//! ask in its mode. Each program runs on a pseudo-terminal of its own, as
//! on a person's.

mod common;

use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const PROGRAM: &str = r#"import std.cli
import std.console
import std.env
import std.time

fn main(): !cli.TerminalError {
    match env.args().get(0) ?? "" {
        "keys" => pressing()?
        "screen" => drawing()?
        "secret" => console.println("got {cli.secret("Password")}")
        "raw" => raw()?
        "live" => living()
        "search" => {
            let drinks = ["tea", "coffee", "water", "cocoa"]
            console.println("found {drinks[cli.choose("Find", drinks, search: true)]}")
            let picked = cli.chooseMany("Pick", drinks, search: true)
            console.println("picked {picked}")
        }
        "sizes" => {
            // Only a change of size can wake it: no deadlock. It says when
            // it listens for one, which the test then makes.
            let sizes = cli.redraws()
            console.println("waiting")
            for size in sizes {
                console.println("drawn at {size}")
                break
            }
        }
        _ => console.println("output is a terminal: {cli.isTerminal()}")
    }
}

fn pressing(): !cli.TerminalError {
    console.println("size {cli.terminalSize()}")
    let terminal = cli.Terminal()?
    match cli.Terminal() {
        ok(_) => console.println("two")
        err(error) => console.println("second: {error}")
    }
    let keys = terminal.keys()
    let sizes = cli.redraws()
    // Taken, and listening for keys and sizes.
    console.println("taken")
    for {
        select {
            let key = keys.receive() => match key {
                some(cli.Key.Char('q')) | none => break
                some(pressed) => console.println("key {pressed}")
            }
            let resized = sizes.receive() => console.println("drawn at {resized}")
        }
    }
    terminal.release()
    console.println("name {cli.ask("Name")}")
    console.println("chose {cli.choose("Pick", ["tea", "coffee", "water"])}")
    console.println("picked {cli.chooseMany("Parts", ["docs", "tests", "examples"], chosen: [0])}")
}

fn drawing(): !cli.TerminalError {
    let terminal = cli.Terminal(screen: true)?
    console.println("on the screen")
    for key in terminal.keys() {
        if key == cli.Key.Char('p') {
            panic("pressed p")
        }
    }
}

fn living() {
    let bar = cli.progress("Building", total: 2)
    console.println("above the bar")
    bar.advance()
    let name = cli.ask("Name")
    console.println("name {name}")
    bar.advance()
    bar.finish()
    let work = cli.progress("Waiting")
    let live = cli.Live()
    live.show("live one")
    console.eprintln("an error")
    live.show("live two")
    work.say("Still waiting")
    time.sleep(time.milliseconds(400))
    work.finish(failed: true)
}

fn raw(): !cli.TerminalError {
    let terminal = cli.Terminal(signals: false)?
    console.println("taken")
    for key in terminal.keys() {
        console.println("key {key}")
        if key == cli.Key.Ctrl('z') {
            break
        }
    }
}
"#;

/// The program, built once.
fn program() -> PathBuf {
    static BUILT: Mutex<Option<PathBuf>> = Mutex::new(None);
    let mut built = BUILT.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(path) = &*built {
        return path.clone();
    }
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("terminal");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("terminal.to"), PROGRAM).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["build", "terminal.to", "--output", "terminal"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    *built = Some(dir.join("terminal"));
    dir.join("terminal")
}

/// A program on a pseudo-terminal, which it has as its controlling one, and
/// what it wrote there.
struct Terminal {
    master: OwnedFd,
    child: Child,
    written: Arc<Mutex<Vec<u8>>>,
    /// Where the next `expect` looks from.
    seen: usize,
}

impl Terminal {
    /// Runs the program with `argument`, alone or, with `shell`, as a job
    /// of a shell that controls jobs and brings it back with `fg` after
    /// Ctrl-Z stops it.
    fn start(argument: &str, shell: bool) -> Terminal {
        Terminal::start_at(argument, shell, 100, 30)
    }

    /// As `start`, on a terminal of that size.
    fn start_at(argument: &str, shell: bool, columns: u16, rows: u16) -> Terminal {
        let program = program();
        let (master, slave) = open();
        let size = libc::winsize { ws_row: rows, ws_col: columns, ws_xpixel: 0, ws_ypixel: 0 };
        unsafe { libc::ioctl(master.as_raw_fd(), libc::TIOCSWINSZ, &size) };
        let mut command = if shell {
            // Interactive, as `set -m` alone does not bring a job back on
            // macOS; with no file of the environment's to read.
            let mut command = Command::new("/bin/sh");
            command.args(["-i", "-c", &format!("'{}' {argument}; sleep 1; fg", program.display())]).env_remove("ENV");
            command
        } else {
            let mut command = Command::new(&program);
            command.arg(argument);
            command
        };
        for set in [Command::stdin, Command::stdout, Command::stderr] {
            set(&mut command, Stdio::from(slave.try_clone().unwrap()));
        }
        // A session of its own, with the terminal as its controlling one.
        unsafe {
            use std::os::unix::process::CommandExt;
            command.pre_exec(|| {
                libc::setsid();
                libc::ioctl(0, libc::TIOCSCTTY as _, 0);
                Ok(())
            });
        }
        let child = command.spawn().unwrap();
        drop(slave);
        let written = Arc::new(Mutex::new(Vec::new()));
        let (mut reading, into) = (std::fs::File::from(master.try_clone().unwrap()), written.clone());
        std::thread::spawn(move || {
            let mut buffer = [0u8; 4096];
            // It ends when the program, and those it started, close the terminal.
            while let Ok(count @ 1..) = reading.read(&mut buffer) {
                into.lock().unwrap().extend_from_slice(&buffer[..count]);
            }
        });
        Terminal { master, child, written, seen: 0 }
    }

    fn send(&self, keys: &str) {
        std::fs::File::from(self.master.try_clone().unwrap()).write_all(keys.as_bytes()).unwrap();
    }

    /// What it wrote, with the terminal's line ends as the program wrote them.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.written.lock().unwrap()).replace("\r\n", "\n")
    }

    /// Waits for the program to write `text` after what the last call saw.
    fn expect(&mut self, text: &str) {
        let seen = self.seen;
        let at = common::until_showing(
            &format!("{text:?}"),
            || self.text().get(seen..).and_then(|rest| rest.find(text)),
            || format!("after {:?}", self.text().get(seen..).unwrap_or_default()),
        );
        self.seen += at + text.len();
    }

    /// Whether the terminal is in lines and shows what is typed.
    fn mode(&self) -> (bool, bool) {
        let mut settings: libc::termios = unsafe { std::mem::zeroed() };
        assert_eq!(unsafe { libc::tcgetattr(self.master.as_raw_fd(), &mut settings) }, 0);
        (settings.c_lflag & libc::ICANON != 0, settings.c_lflag & libc::ECHO != 0)
    }

    /// Waits for the terminal's mode to be `lines` and `echo`.
    fn expect_mode(&self, lines: bool, echo: bool) {
        common::until_showing(
            &format!("the mode {:?}", (lines, echo)),
            || (self.mode() == (lines, echo)).then_some(()),
            || format!("the mode stayed {:?}: {}", self.mode(), self.text()),
        );
    }

    fn resize(&self, columns: u16, rows: u16) {
        let size = libc::winsize { ws_row: rows, ws_col: columns, ws_xpixel: 0, ws_ypixel: 0 };
        unsafe { libc::ioctl(self.master.as_raw_fd(), libc::TIOCSWINSZ, &size) };
    }

    /// Its exit status, once it ends.
    fn status(&mut self) -> i32 {
        use std::os::unix::process::ExitStatusExt;
        let written = &self.written;
        let shown = || String::from_utf8_lossy(&written.lock().unwrap()).into_owned();
        let status = common::until_showing("its end", || self.child.try_wait().unwrap(), shown);
        status.code().unwrap_or_else(|| 128 + status.signal().unwrap_or(0))
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A new pseudo-terminal of 100 columns and 30 rows: its two ends.
fn open() -> (OwnedFd, OwnedFd) {
    static NAMING: Mutex<()> = Mutex::new(());
    unsafe {
        let master = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY);
        assert!(master >= 0 && libc::grantpt(master) == 0 && libc::unlockpt(master) == 0);
        let master = OwnedFd::from_raw_fd(master);
        let name = {
            let _naming = NAMING.lock().unwrap();
            std::ffi::CStr::from_ptr(libc::ptsname(master.as_raw_fd())).to_owned()
        };
        let slave = libc::open(name.as_ptr(), libc::O_RDWR | libc::O_NOCTTY);
        assert!(slave >= 0);
        let size = libc::winsize { ws_row: 30, ws_col: 100, ws_xpixel: 0, ws_ypixel: 0 };
        libc::ioctl(master.as_raw_fd(), libc::TIOCSWINSZ, &size);
        (master, OwnedFd::from_raw_fd(slave))
    }
}

#[test]
fn a_taken_terminal_gives_keys_sizes_and_questions() {
    let mut terminal = Terminal::start("keys", false);
    terminal.expect("size some(Size { columns: 100, rows: 30 })\n");
    terminal.expect("second: another cli.Terminal holds the terminal\ntaken\n");
    terminal.expect_mode(false, false);
    // Keys as they are pressed, a lone Escape told from a sequence.
    for (keys, shown) in [
        ("a", "Char(a)"),
        ("\x1b[A", "Up"),
        ("\x1b[1;5D", "Left"),
        ("\x1bOP", "F(1)"),
        ("\x1b[15~", "F(5)"),
        ("\x1b[3~", "Delete"),
        ("\x1bx", "Alt(x)"),
        ("ñ", "Char(ñ)"),
        ("\x01", "Ctrl(a)"),
        ("\t", "Tab"),
        ("\r", "Enter"),
        ("\x7f", "Backspace"),
        ("\x1b", "Escape"),
    ] {
        terminal.send(keys);
        terminal.expect(&format!("key {shown}\n"));
    }
    terminal.resize(80, 24);
    terminal.expect("drawn at some(Size { columns: 80, rows: 24 })\n");
    // Given back, a question asks in lines and shows what is typed.
    terminal.send("q");
    terminal.expect("Name: ");
    terminal.expect_mode(true, true);
    terminal.send("Ada\r");
    terminal.expect("Ada\nname Ada\n");
    // A choice with the arrows, and Enter.
    terminal.expect("❯ tea");
    terminal.expect_mode(false, false);
    terminal.send("\x1b[B");
    terminal.expect("❯ coffee");
    terminal.send("\x1b[B\x1b[B\x1b[A\r");
    terminal.expect("Pick: water\nchose 2\n");
    // Several, Space marking each, with the first marked from the start.
    terminal.expect("❯ ◉ docs");
    terminal.send("\x1b[B");
    terminal.expect("❯ ○ tests");
    terminal.send(" ");
    terminal.expect("❯ ◉ tests");
    terminal.send("\x1b[B \x1b[A\r");
    terminal.expect("Parts: docs, tests, examples\npicked [0, 1, 2]\n");
    assert_eq!(terminal.status(), 0);
    assert_eq!(terminal.mode(), (true, true));
}

#[test]
fn ctrl_z_gives_the_terminal_back_until_fg() {
    let mut terminal = Terminal::start("keys", true);
    terminal.expect("taken\n");
    terminal.expect_mode(false, false);
    terminal.send("a");
    terminal.expect("key Char(a)\n");
    // Stopped, the shell has the terminal as it was; `fg` brings the
    // program back, which takes it again and draws.
    terminal.send("\x1a");
    terminal.expect_mode(true, true);
    terminal.expect("drawn at some(Size { columns: 100, rows: 30 })\n");
    terminal.expect_mode(false, false);
    terminal.send("b");
    terminal.expect("key Char(b)\n");
    terminal.send("q");
    terminal.expect("Name: ");
    terminal.send("Ada\r");
    terminal.expect("❯ tea");
    terminal.send("\r");
    terminal.expect("chose 0\n");
    terminal.expect("❯ ◉ docs");
    terminal.send("\r");
    terminal.expect("picked [0]\n");
    assert_eq!(terminal.status(), 0);
}

#[test]
fn a_panic_shows_on_the_main_screen() {
    let mut terminal = Terminal::start("screen", false);
    terminal.expect("on the screen\n");
    terminal.send("p");
    terminal.expect("\x1b[?1049l");
    terminal.expect("panic[P0008]: explicit-panic: pressed p");
    assert_ne!(terminal.status(), 0);
    assert_eq!(terminal.mode(), (true, true));
}

#[test]
fn a_secret_cancelled_leaves_the_echo_on() {
    let mut terminal = Terminal::start("secret", false);
    terminal.expect("Password: ");
    terminal.expect_mode(true, false);
    terminal.send("\x03");
    assert_eq!(terminal.status(), 130);
    assert_eq!(terminal.mode(), (true, true));
    let mut terminal = Terminal::start("secret", false);
    terminal.expect("Password: ");
    terminal.send("hunter2\r");
    terminal.expect("\ngot hunter2\n");
    assert_eq!(terminal.status(), 0);
}

#[test]
fn without_signals_ctrl_c_and_ctrl_z_are_keys() {
    let mut terminal = Terminal::start("raw", false);
    terminal.expect("taken\n");
    terminal.send("\x03");
    terminal.expect("key Ctrl(c)\n");
    terminal.send("\x1a");
    terminal.expect("key Ctrl(z)\n");
    assert_eq!(terminal.status(), 0);
}

#[test]
fn waiting_for_a_new_size_is_no_deadlock() {
    let mut terminal = Terminal::start("sizes", false);
    terminal.expect("waiting\n");
    terminal.resize(90, 20);
    terminal.expect("drawn at Size { columns: 90, rows: 20 }\n");
    assert_eq!(terminal.status(), 0);
}

#[test]
fn is_terminal_asks_of_the_output() {
    // The input a terminal and the output a pipe: what it writes goes to a
    // file or another program, with no codes.
    let (_master, slave) = open();
    let output = Command::new(program()).arg("is").stdin(Stdio::from(slave)).stdout(Stdio::piped()).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&output.stdout), "output is a terminal: false\n");
    // The output a terminal, whatever the input is. Its other end stays
    // open here until it is read, as macOS drops what the terminal holds
    // once the last one closes.
    let (master, slave) = open();
    let mut child = Command::new(program())
        .arg("is")
        .stdin(Stdio::null())
        .stdout(Stdio::from(slave.try_clone().unwrap()))
        .spawn()
        .unwrap();
    assert!(child.wait().unwrap().success());
    let mut written = vec![0; 256];
    let count = std::fs::File::from(master).read(&mut written).unwrap();
    drop(slave);
    assert_eq!(String::from_utf8_lossy(&written[..count]), "output is a terminal: true\r\n");
}

#[test]
fn a_terminal_without_a_size_gives_none_until_it_has_one() {
    // 0 by 0, as `script` from a pipe or a serial console reports.
    let mut terminal = Terminal::start_at("keys", false, 0, 0);
    terminal.expect("size none\n");
    terminal.expect("taken\n");
    // Time is what is under test: a draw with no size would come meanwhile.
    std::thread::sleep(Duration::from_millis(300));
    assert!(!terminal.text().contains("drawn at"), "{}", terminal.text());
    terminal.resize(90, 20);
    terminal.expect("drawn at some(Size { columns: 90, rows: 20 })\n");
    terminal.resize(0, 0);
    terminal.send("q");
    terminal.expect("Name: ");
    terminal.send("Ada\r");
    // A choice draws once, at the size it supposes, and waits.
    terminal.expect("❯ tea");
    // Time is what is under test: a second draw would come meanwhile.
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(terminal.text().matches("❯ tea").count(), 1, "{}", terminal.text());
    terminal.send("\r");
    terminal.expect("chose 0\n");
    terminal.expect("❯ ◉ docs");
    terminal.send("\r");
    terminal.expect("picked [0]\n");
    assert_eq!(terminal.status(), 0);
}

#[test]
fn a_live_region_stays_below_what_the_program_prints() {
    let mut terminal = Terminal::start("live", false);
    terminal.expect("Building ");
    terminal.expect(" 0/2");
    // A line printed goes above the bar, which is drawn again below it.
    terminal.expect("above the bar\n");
    terminal.expect(" 1/2");
    // A question asks below, and the region comes back after the answer.
    terminal.expect("Name: ");
    terminal.send("Ada\r");
    terminal.expect("name Ada\n");
    // A bar that finishes stays written, full, above what comes after.
    terminal.expect(" 2/2\n");
    terminal.expect("live one");
    terminal.expect("an error\n");
    // A spinner turns by itself while the program waits.
    terminal.expect("⠙ Still waiting");
    terminal.expect("✗ Still waiting\n");
    // The program ends with the region's last text written.
    terminal.expect("live two\n");
    assert_eq!(terminal.status(), 0);
}

#[test]
fn a_choice_with_search_shows_what_matches_what_is_typed() {
    let mut terminal = Terminal::start("search", false);
    terminal.expect("❯ tea");
    // Typed letters narrow the list to the options that have them in order.
    terminal.send("co");
    terminal.expect("/ co");
    terminal.expect("❯ coffee");
    terminal.send("a");
    terminal.expect("❯ cocoa");
    terminal.send("\x7f\x7f\x7f\x1b[B\r");
    terminal.expect("Find: coffee\nfound coffee\n");
    // Several: Tab marks, as Space is typed.
    terminal.expect("❯ ○ tea");
    terminal.send("\tw\t\r");
    terminal.expect("Pick: tea, water\npicked [0, 2]\n");
    assert_eq!(terminal.status(), 0);
}
