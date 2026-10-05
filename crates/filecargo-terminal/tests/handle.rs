#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use filecargo_remote_fs::{ShellChannel, ShellInput, ShellOutput};
use filecargo_terminal::{Key, Mods, TermSize, TermStatus, TerminalHandle, spawn};
use tokio::sync::mpsc;

/// The "server" side of a fake shell channel.
struct Remote {
    output: mpsc::UnboundedSender<ShellOutput>,
    input: mpsc::UnboundedReceiver<ShellInput>,
}

fn open(size: TermSize) -> (TerminalHandle, Remote) {
    let (input_tx, input_rx) = mpsc::unbounded_channel();
    let (output_tx, output_rx) = mpsc::unbounded_channel();
    let handle = spawn(
        ShellChannel {
            input: input_tx,
            output: output_rx,
        },
        size,
        5000,
    );
    (
        handle,
        Remote {
            output: output_tx,
            input: input_rx,
        },
    )
}

const SIZE: TermSize = TermSize { cols: 80, rows: 24 };

impl Remote {
    fn say(&self, text: &str) {
        self.output
            .send(ShellOutput::Data(text.as_bytes().to_vec()))
            .unwrap();
    }

    /// Everything the terminal has sent so far.
    fn received(&mut self) -> Vec<ShellInput> {
        let mut all = Vec::new();
        while let Ok(message) = self.input.try_recv() {
            all.push(message);
        }
        all
    }
}

/// Lets the feeder task run until `condition` holds.
async fn settle(handle: &TerminalHandle, condition: impl Fn(&TerminalHandle) -> bool) {
    for _ in 0..200 {
        if condition(handle) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("the terminal did not reach the expected state");
}

fn bytes(messages: &[ShellInput]) -> Vec<u8> {
    messages
        .iter()
        .filter_map(|m| match m {
            ShellInput::Data(d) => Some(d.clone()),
            _ => None,
        })
        .flatten()
        .collect()
}

#[tokio::test]
async fn output_reaches_the_screen_and_bumps_the_generation() {
    let (term, remote) = open(SIZE);
    let before = term.generation();
    remote.say("\x1b[2J\x1b[1;1Hhello");
    settle(&term, |t| t.generation() > before).await;
    assert!(term.with_screen(|s| s.contents()).starts_with("hello"));
    assert_eq!(term.with_screen(|s| s.cursor_position()), (0, 5));
}

#[tokio::test]
async fn output_split_across_messages_still_forms_escape_sequences() {
    let (term, remote) = open(SIZE);
    remote.say("\x1b[");
    remote.say("2J\x1b[1;1");
    remote.say("Hsplit");
    settle(&term, |t| {
        t.with_screen(|s| s.contents()).starts_with("split")
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn ten_resizes_in_fifty_milliseconds_send_at_most_two_window_changes() {
    let (term, mut remote) = open(SIZE);
    for i in 0..10u16 {
        term.resize(TermSize {
            cols: 81 + i,
            rows: 25,
        });
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    let resizes: Vec<_> = remote
        .received()
        .into_iter()
        .filter_map(|m| match m {
            ShellInput::Resize { cols, rows } => Some((cols, rows)),
            _ => None,
        })
        .collect();
    assert!(!resizes.is_empty() && resizes.len() <= 2, "{resizes:?}");
    assert_eq!(resizes.last(), Some(&(90, 25)), "the latest size wins");
    assert_eq!(term.with_screen(|s| s.size()), (25, 90));
}

#[tokio::test(start_paused = true)]
async fn a_single_resize_is_sent_at_once_and_a_later_one_again() {
    let (term, mut remote) = open(SIZE);
    term.resize(TermSize {
        cols: 100,
        rows: 30,
    });
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert_eq!(
        remote.received(),
        [ShellInput::Resize {
            cols: 100,
            rows: 30
        }]
    );
    tokio::time::sleep(Duration::from_secs(1)).await;
    term.resize(TermSize {
        cols: 120,
        rows: 40,
    });
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert_eq!(
        remote.received(),
        [ShellInput::Resize {
            cols: 120,
            rows: 40
        }]
    );
}

#[tokio::test]
async fn paste_is_bracketed_only_when_the_application_asked_for_it() {
    let (term, mut remote) = open(SIZE);
    term.paste("a\nb");
    assert_eq!(
        bytes(&remote.received()),
        b"a\rb",
        "plain: line breaks become carriage returns"
    );
    term.paste("x\r\ny");
    assert_eq!(bytes(&remote.received()), b"x\ry");

    remote.say("\x1b[?2004h");
    settle(&term, |t| t.with_screen(|s| s.bracketed_paste())).await;
    term.paste("a\nb");
    assert_eq!(bytes(&remote.received()), b"\x1b[200~a\nb\x1b[201~");

    remote.say("\x1b[?2004l");
    settle(&term, |t| !t.with_screen(|s| s.bracketed_paste())).await;
    term.paste("c");
    assert_eq!(bytes(&remote.received()), b"c");
}

#[tokio::test]
async fn pasted_text_cannot_close_the_bracket_early() {
    let (term, mut remote) = open(SIZE);
    remote.say("\x1b[?2004h");
    settle(&term, |t| t.with_screen(|s| s.bracketed_paste())).await;
    term.paste("safe\x1b[201~rm -rf /\n");
    let sent = bytes(&remote.received());
    assert_eq!(sent, b"\x1b[200~saferm -rf /\n\x1b[201~");
    assert_eq!(sent.windows(6).filter(|w| *w == b"\x1b[201~").count(), 1);
}

#[tokio::test]
async fn scrollback_shows_earlier_lines_and_typing_returns_to_live() {
    let (term, mut remote) = open(SIZE);
    let lines: String = (1..=100).map(|n| format!("line {n:03}\r\n")).collect();
    remote.say(&lines);
    settle(&term, |t| {
        t.with_screen(|s| s.contents()).contains("line 100")
    })
    .await;
    let live = term.with_screen(|s| s.contents());
    assert!(!live.contains("line 050"));

    term.scroll(40);
    let scrolled = term.with_screen(|s| s.contents());
    assert!(scrolled.contains("line 050"), "{scrolled}");
    assert!(!scrolled.contains("line 100"));
    assert!(term.with_screen(|s| s.scrollback()) > 0);

    term.scroll(-40);
    assert_eq!(
        term.with_screen(|s| s.contents()),
        live,
        "scrolling forward by the same amount is live again"
    );

    term.scroll(30);
    term.send_key(Key::Char('x'), Mods::NONE);
    assert_eq!(
        term.with_screen(|s| s.scrollback()),
        0,
        "typing returns to the live view"
    );
    assert_eq!(bytes(&remote.received()), b"x");

    term.scroll(30);
    term.scroll(0);
    assert_eq!(
        term.with_screen(|s| s.scrollback()),
        0,
        "scroll(0) is back to live"
    );
}

#[tokio::test]
async fn keys_follow_the_applications_cursor_mode() {
    let (term, mut remote) = open(SIZE);
    term.send_key(Key::Up, Mods::NONE);
    assert_eq!(bytes(&remote.received()), b"\x1b[A");
    remote.say("\x1b[?1h");
    settle(&term, |t| t.with_screen(|s| s.application_cursor())).await;
    term.send_key(Key::Up, Mods::NONE);
    assert_eq!(bytes(&remote.received()), b"\x1bOA");
    term.send_key(Key::Char('c'), Mods::CTRL);
    term.send_text("héllo");
    assert_eq!(bytes(&remote.received()), "\u{3}héllo".as_bytes());
}

#[tokio::test]
async fn exit_keeps_the_screen_and_drops_later_input() {
    let (term, mut remote) = open(SIZE);
    remote.say("bye");
    remote.output.send(ShellOutput::Exit(Some(0))).unwrap();
    settle(&term, |t| t.status() == TermStatus::Exited(Some(0))).await;
    assert!(
        term.with_screen(|s| s.contents()).starts_with("bye"),
        "the screen stays readable"
    );

    term.send_key(Key::Enter, Mods::NONE);
    term.send_text("ignored");
    term.paste("ignored");
    assert!(remote.received().is_empty(), "input after exit is dropped");

    remote.output.send(ShellOutput::Closed).unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(
        term.status(),
        TermStatus::Exited(Some(0)),
        "Closed does not overwrite the exit status"
    );
}

#[tokio::test]
async fn a_channel_that_vanishes_without_an_exit_status_is_closed() {
    let (term, remote) = open(SIZE);
    drop(remote);
    settle(&term, |t| t.status() == TermStatus::Closed).await;
    term.send_key(Key::Enter, Mods::NONE); // must not panic
}

#[tokio::test]
async fn title_bell_and_selection() {
    let (term, remote) = open(SIZE);
    remote.say("\x1b]2;my title\x07first line\r\nsecond line\x07");
    settle(&term, |t| t.title() == "my title").await;
    assert_eq!(term.bell_count(), 1);
    assert_eq!(term.selection_text((0, 0), (0, 5)), "first");
    assert_eq!(term.selection_text((0, 6), (1, 6)), "line\nsecond");
    assert_eq!(
        term.selection_text((1, 6), (0, 6)),
        "line\nsecond",
        "the order of the ends does not matter"
    );
}

#[tokio::test]
async fn close_asks_the_server_to_end_the_shell() {
    let (term, mut remote) = open(SIZE);
    term.close();
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(remote.received(), [ShellInput::Close]);
}

#[tokio::test]
async fn heavy_output_is_fed_in_pieces_and_lands_completely() {
    let (term, remote) = open(SIZE);
    let big = "x".repeat(300_000) + "END";
    remote.say(&big);
    settle(&term, |t| t.with_screen(|s| s.contents()).contains("END")).await;
}

#[tokio::test]
async fn wait_ended_returns_when_the_shell_exits_or_the_channel_closes() {
    let (term, remote) = open(SIZE);
    let waiter = tokio::spawn({
        let term = term.clone();
        async move { term.wait_ended().await }
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(!waiter.is_finished(), "a running shell has not ended");
    remote.output.send(ShellOutput::Exit(Some(7))).unwrap();
    assert_eq!(waiter.await.unwrap(), TermStatus::Exited(Some(7)));
    assert_eq!(
        term.wait_ended().await,
        TermStatus::Exited(Some(7)),
        "already ended: returns at once"
    );

    let (term, remote) = open(SIZE);
    drop(remote);
    assert_eq!(term.wait_ended().await, TermStatus::Closed);
}

#[tokio::test]
async fn raw_bytes_are_sent_as_they_are() {
    let (term, mut remote) = open(SIZE);
    term.send_raw(vec![0x1b, b'[', b'A', 0xff]);
    assert_eq!(bytes(&remote.received()), [0x1b, b'[', b'A', 0xff]);
}
