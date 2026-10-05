#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use filecargo_remote_fs::{RemoteFs, RootedFs};
use filecargo_transfer::{Direction, ItemState, NewTransfer, Outcome};
use support::{Harness, build_tree, remote, tree_hashes};

fn dir_transfer(
    h: &Harness,
    direction: Direction,
    local: &std::path::Path,
    remote_path: &str,
) -> NewTransfer {
    NewTransfer {
        site: h.site,
        direction,
        local: local.to_path_buf(),
        remote: remote(remote_path),
        is_dir: true,
        size: None,
        conflict: None,
    }
}

#[tokio::test]
async fn a_tree_of_1000_files_over_50_directories_uploads_byte_for_byte() {
    let mut h = Harness::new(4).await;
    let src = h.local.path().join("src");
    build_tree(&src, 20);
    h.queue
        .enqueue(vec![dir_transfer(&h, Direction::Upload, &src, "/dst")]);
    h.idle().await;

    let snapshot = h.queue.snapshot();
    assert_eq!(snapshot.failed.len(), 0, "{:?}", snapshot.failed);
    assert_eq!(
        h.finished_ok,
        1000 + 50 + 1,
        "files + directories + the root"
    );
    assert_eq!(
        snapshot.completed.len(),
        1000,
        "the completed list is capped at 1,000"
    );
    assert!(
        h.connector.stats.peak() <= 4,
        "peak was {}",
        h.connector.stats.peak()
    );
    let (sent_files, sent_dirs) = tree_hashes(&src);
    let (got_files, got_dirs) = tree_hashes(&h.server.path().join("dst"));
    assert_eq!(sent_files.len(), 1000);
    assert_eq!(sent_dirs.len(), 50);
    assert_eq!((got_files, got_dirs), (sent_files, sent_dirs));
}

#[tokio::test]
async fn a_tree_of_1000_files_over_50_directories_downloads_byte_for_byte() {
    let mut h = Harness::new(4).await;
    build_tree(&h.server.path().join("tree"), 20);
    let dst = h.local.path().join("dst");
    h.queue
        .enqueue(vec![dir_transfer(&h, Direction::Download, &dst, "/tree")]);
    h.idle().await;

    let snapshot = h.queue.snapshot();
    assert_eq!(snapshot.failed.len(), 0, "{:?}", snapshot.failed);
    assert_eq!(h.finished_ok, 1051);
    assert!(h.connector.stats.peak() <= 4);
    assert_eq!(
        tree_hashes(&dst),
        tree_hashes(&h.server.path().join("tree"))
    );
}

#[tokio::test]
async fn children_follow_their_parent_in_source_listing_order() {
    let mut h = Harness::new(1).await;
    let src = h.local.path().join("src");
    std::fs::create_dir_all(src.join("sub")).unwrap();
    for name in ["zeta", "alpha", "mid", "beta"] {
        std::fs::write(src.join(name), name).unwrap();
    }
    std::fs::write(src.join("sub").join("inner"), "i").unwrap();
    h.queue
        .enqueue(vec![dir_transfer(&h, Direction::Upload, &src, "/dst")]);
    h.idle().await;

    // what the listing says, in the order it says it
    let listing: Vec<String> = filecargo_remote_fs::local::read_dir(&src)
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    // one slot: completion order is queue order; `completed` is newest first
    let mut finished: Vec<String> = h
        .queue
        .snapshot()
        .completed
        .iter()
        .rev()
        .map(|v| {
            v.item
                .local
                .strip_prefix(&src)
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(finished.remove(0), "", "the directory itself comes first");
    let mut expected = Vec::new();
    for name in &listing {
        expected.push(name.clone());
        if name == "sub" {
            expected.push(
                "sub/inner"
                    .to_owned()
                    .replace('/', std::path::MAIN_SEPARATOR_STR),
            );
        }
    }
    assert_eq!(finished, expected);
}

#[tokio::test]
async fn an_existing_target_directory_is_merged_into() {
    let mut h = Harness::new(2).await;
    std::fs::create_dir_all(h.server.path().join("dst")).unwrap();
    std::fs::write(
        h.server.path().join("dst").join("already-there"),
        b"keep me",
    )
    .unwrap();
    let src = h.local.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("new"), b"new").unwrap();
    h.queue
        .enqueue(vec![dir_transfer(&h, Direction::Upload, &src, "/dst")]);
    h.idle().await;
    assert_eq!(
        std::fs::read(h.server.path().join("dst/already-there")).unwrap(),
        b"keep me"
    );
    assert_eq!(
        std::fs::read(h.server.path().join("dst/new")).unwrap(),
        b"new"
    );
    assert!(h.queue.snapshot().failed.is_empty());
}

#[tokio::test]
async fn a_directory_item_completes_as_created() {
    let mut h = Harness::new(1).await;
    let src = h.local.path().join("empty");
    std::fs::create_dir_all(&src).unwrap();
    h.queue
        .enqueue(vec![dir_transfer(&h, Direction::Upload, &src, "/empty")]);
    h.idle().await;
    let snapshot = h.queue.snapshot();
    assert_eq!(snapshot.completed.len(), 1);
    assert!(matches!(
        snapshot.completed[0].item.state,
        ItemState::Completed {
            outcome: Outcome::Created,
            ..
        }
    ));
    assert!(h.server.path().join("empty").is_dir());
}

#[cfg(unix)]
#[tokio::test]
async fn symlinks_are_skipped_not_followed() {
    let mut h = Harness::new(1).await;
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret"), b"secret").unwrap();
    let src = h.local.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("real"), b"real").unwrap();
    std::os::unix::fs::symlink(outside.path(), src.join("link-to-dir")).unwrap();
    std::os::unix::fs::symlink(outside.path().join("secret"), src.join("link-to-file")).unwrap();
    h.queue
        .enqueue(vec![dir_transfer(&h, Direction::Upload, &src, "/dst")]);
    h.idle().await;

    let (files, dirs) = tree_hashes(&h.server.path().join("dst"));
    assert_eq!(files.keys().collect::<Vec<_>>(), ["real"]);
    assert!(dirs.is_empty());
}

#[tokio::test]
async fn uploading_a_directory_onto_an_existing_file_fails_cleanly() {
    let mut h = Harness::new(1).await;
    std::fs::write(h.server.path().join("dst"), b"i am a file").unwrap();
    let src = h.local.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    h.queue
        .enqueue(vec![dir_transfer(&h, Direction::Upload, &src, "/dst")]);
    h.idle().await;
    let snapshot = h.queue.snapshot();
    assert_eq!(snapshot.failed.len(), 1);
    match &snapshot.failed[0].item.state {
        ItemState::Failed { reason, .. } => assert!(reason.contains("not a directory"), "{reason}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        std::fs::read(h.server.path().join("dst")).unwrap(),
        b"i am a file"
    );
}

#[tokio::test]
async fn the_server_path_helper_is_the_rooted_view_of_the_server_directory() {
    // sanity check of the harness itself: what the test reads on disk is what the queue wrote
    let h = Harness::new(1).await;
    let fs = RootedFs::new(h.server.path()).unwrap();
    assert!(fs.list(&remote("/")).await.unwrap().is_empty());
}
