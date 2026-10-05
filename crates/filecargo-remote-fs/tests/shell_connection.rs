#![cfg(all(feature = "integration", target_os = "linux"))]
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! A shell must ride on the SFTP session's SSH connection, not open a second TCP connection.
//! This is the only test in its binary so the process's sockets are all ours: it counts the
//! established sockets this process holds to the server's port, via /proc.

mod support;

use std::collections::HashSet;

use filecargo_remote_fs::ShellInput;
use support::docker_sftp;

const SERVER_PORT: u16 = 2222;

fn established_inodes_to_server() -> HashSet<u64> {
    let mut inodes = HashSet::new();
    for table in ["/proc/net/tcp", "/proc/net/tcp6"] {
        let Ok(text) = std::fs::read_to_string(table) else {
            continue;
        };
        for line in text.lines().skip(1) {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let (Some(remote), Some(state), Some(inode)) =
                (fields.get(2), fields.get(3), fields.get(9))
            else {
                continue;
            };
            let port = remote
                .rsplit(':')
                .next()
                .and_then(|p| u16::from_str_radix(p, 16).ok());
            if port == Some(SERVER_PORT) && *state == "01" {
                inodes.extend(inode.parse::<u64>());
            }
        }
    }
    inodes
}

/// Sockets to the server that are open in *this* process.
fn own_connections() -> usize {
    let wanted = established_inodes_to_server();
    std::fs::read_dir("/proc/self/fd")
        .unwrap()
        .filter_map(Result::ok)
        .filter_map(|fd| std::fs::read_link(fd.path()).ok())
        .filter_map(|target| {
            let name = target.to_string_lossy().into_owned();
            name.strip_prefix("socket:[")?
                .strip_suffix(']')?
                .parse::<u64>()
                .ok()
        })
        .filter(|inode| wanted.contains(inode))
        .count()
}

#[tokio::test]
async fn shell_rides_on_the_sftp_connection_without_a_second_tcp_connection() {
    let (session, _, _dir) = docker_sftp().await;
    assert_eq!(
        own_connections(),
        1,
        "the SFTP session is one TCP connection"
    );

    let shell = session
        .shell
        .as_ref()
        .unwrap()
        .open("xterm", 80, 24)
        .await
        .unwrap();
    shell
        .input
        .send(ShellInput::Data(b"true\n".to_vec()))
        .unwrap();
    let home = session.fs.home().await.unwrap();
    assert!(session.fs.stat(&home).await.unwrap().is_some());
    assert_eq!(
        own_connections(),
        1,
        "opening a shell must not add a connection"
    );

    let second = session
        .shell
        .as_ref()
        .unwrap()
        .open("xterm", 80, 24)
        .await
        .unwrap();
    assert_eq!(own_connections(), 1, "neither does a second shell");
    drop((shell, second));
}
