#![cfg(feature = "integration")]
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! The shell channel shares the SFTP session's SSH connection.

mod support;

use std::time::Duration;

use filecargo_remote_fs::{ShellChannel, ShellInput, ShellOutput};
use support::docker_sftp;
use tokio::time::timeout;

/// Reads output until `needle` appears (or panics after 10 s), returning everything read.
async fn read_until(shell: &mut ShellChannel, needle: &str) -> String {
    let mut seen = String::new();
    timeout(Duration::from_secs(10), async {
        while let Some(msg) = shell.output.recv().await {
            if let ShellOutput::Data(bytes) = msg {
                seen.push_str(&String::from_utf8_lossy(&bytes));
                if seen.contains(needle) {
                    return;
                }
            }
        }
        panic!("channel ended before {needle:?}; saw {seen:?}");
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {needle:?}; saw {seen:?}"));
    seen
}

fn type_line(shell: &ShellChannel, line: &str) {
    shell
        .input
        .send(ShellInput::Data(format!("{line}\n").into_bytes()))
        .unwrap();
}

#[tokio::test]
async fn shell_runs_a_command_and_exits_with_its_status() {
    let (session, _, _dir) = docker_sftp().await;
    let mut shell = session
        .shell
        .as_ref()
        .expect("sftp sessions have a shell")
        .open("xterm-256color", 80, 24)
        .await
        .unwrap();
    type_line(&shell, "echo hello-$((40+2))");
    read_until(&mut shell, "hello-42").await;
    type_line(&shell, "exit 3");
    let mut exit = None;
    let mut closed = false;
    while let Some(msg) = timeout(Duration::from_secs(10), shell.output.recv())
        .await
        .unwrap()
    {
        match msg {
            ShellOutput::Exit(code) => exit = Some(code),
            ShellOutput::Closed => {
                closed = true;
                break;
            }
            ShellOutput::Data(_) => {}
        }
    }
    assert_eq!(exit, Some(Some(3)));
    assert!(closed, "Closed must be the last message");
}

#[tokio::test]
async fn shell_resize_changes_stty_size() {
    let (session, _, _dir) = docker_sftp().await;
    let mut shell = session
        .shell
        .as_ref()
        .unwrap()
        .open("xterm", 80, 24)
        .await
        .unwrap();
    type_line(&shell, "stty size");
    read_until(&mut shell, "24 80").await;
    shell
        .input
        .send(ShellInput::Resize {
            cols: 132,
            rows: 43,
        })
        .unwrap();
    // give the server a moment to apply SIGWINCH before asking again
    tokio::time::sleep(Duration::from_millis(300)).await;
    type_line(&shell, "stty size");
    read_until(&mut shell, "43 132").await;
    shell.input.send(ShellInput::Close).unwrap();
}

#[tokio::test]
async fn shell_unread_output_does_not_stall_sftp() {
    let (session, _, _dir) = docker_sftp().await;
    let shell = session
        .shell
        .as_ref()
        .unwrap()
        .open("xterm", 80, 24)
        .await
        .unwrap();
    // ~4 MB of terminal output that nobody reads
    type_line(
        &shell,
        "head -c 4000000 /dev/zero | tr '\\0' 'x'; echo done",
    );
    let started = std::time::Instant::now();
    let home = session.fs.home().await.unwrap();
    for _ in 0..20 {
        session.fs.list(&home).await.unwrap();
    }
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "sftp stalled behind the shell"
    );
    drop(shell);
}
