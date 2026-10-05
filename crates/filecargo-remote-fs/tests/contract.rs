#![allow(clippy::unwrap_used, clippy::expect_used)]
//! One suite of scenarios, run against every backend so they all behave identically.
//! `RootedFs` always runs; the network backends are added under `--features integration`.

mod support;

use std::any::Any;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use filecargo_remote_fs::{EntryKind, FsError, NoProgress, Progress, RemoteFs, RemotePath};
use support::{sample_bytes, sha256_hex};

/// A backend under test plus a private, empty directory to work in.
pub struct Fixture {
    pub fs: Arc<dyn RemoteFs>,
    pub base: RemotePath,
    _keep: Box<dyn Any + Send>,
}

impl Fixture {
    fn path(&self, name: &str) -> RemotePath {
        // `name` may hold several components, e.g. "dir/file"
        name.split('/')
            .fold(self.base.clone(), |p, part| p.join(part).unwrap())
    }

    async fn put(&self, name: &str, bytes: &[u8]) {
        let mut src = bytes;
        self.fs
            .upload(&self.path(name), 0, &mut src, &NoProgress)
            .await
            .unwrap();
    }

    async fn get(&self, name: &str) -> Vec<u8> {
        let mut out = Vec::new();
        self.fs
            .download(&self.path(name), 0, &mut out, &NoProgress)
            .await
            .unwrap();
        out
    }

    /// Removes this test's directory and closes the connection (skipped when a test fails, so
    /// the debris stays for inspection).
    async fn finish(self) {
        if self.base != RemotePath::root() {
            self.fs.remove_all(&self.base).await.unwrap();
        }
        self.fs.close().await;
    }

    async fn names(&self, dir: &RemotePath) -> Vec<String> {
        let mut names: Vec<_> = self
            .fs
            .list(dir)
            .await
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        names.sort();
        names
    }
}

struct Recorder(AtomicU64, AtomicU64);

impl Progress for Recorder {
    fn advance(&self, total: u64) {
        let previous = self.0.swap(total, Ordering::SeqCst);
        assert!(
            total >= previous,
            "progress went backwards: {previous} -> {total}"
        );
        self.1.fetch_add(1, Ordering::SeqCst);
    }
}

fn entry_count() -> usize {
    std::env::var("FILECARGO_CONTRACT_ENTRIES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10_000)
}

mod scenarios {
    use super::*;

    pub async fn list_excludes_dot_entries(fx: &Fixture) {
        fx.put("a.txt", b"a").await;
        fx.fs.mkdir(&fx.path("sub")).await.unwrap();
        let entries = fx.fs.list(&fx.base).await.unwrap();
        let mut names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
        names.sort();
        assert_eq!(names, ["a.txt", "sub"]);
        let file = entries.iter().find(|e| e.name == "a.txt").unwrap();
        assert_eq!((file.kind.clone(), file.size), (EntryKind::File, 1));
        assert!(entries.iter().find(|e| e.name == "sub").unwrap().is_dir());
    }

    pub async fn stat_missing_is_none(fx: &Fixture) {
        assert_eq!(fx.fs.stat(&fx.path("nope")).await.unwrap(), None);
        fx.put("here", b"12345").await;
        let e = fx.fs.stat(&fx.path("here")).await.unwrap().unwrap();
        assert_eq!(
            (e.name.as_str(), e.kind, e.size),
            ("here", EntryKind::File, 5)
        );
    }

    pub async fn mkdir_twice_fails(fx: &Fixture) {
        fx.fs.mkdir(&fx.path("d")).await.unwrap();
        assert!(fx.fs.stat(&fx.path("d")).await.unwrap().unwrap().is_dir());
        assert!(fx.fs.mkdir(&fx.path("d")).await.is_err());
    }

    pub async fn rename_moves_a_file(fx: &Fixture) {
        fx.put("old", b"data").await;
        fx.fs
            .rename(&fx.path("old"), &fx.path("new"))
            .await
            .unwrap();
        assert_eq!(fx.fs.stat(&fx.path("old")).await.unwrap(), None);
        assert_eq!(fx.get("new").await, b"data");
    }

    pub async fn remove_file_deletes_it(fx: &Fixture) {
        fx.put("f", b"x").await;
        fx.fs.remove_file(&fx.path("f")).await.unwrap();
        assert_eq!(fx.fs.stat(&fx.path("f")).await.unwrap(), None);
        assert!(fx.fs.remove_file(&fx.path("f")).await.is_err());
    }

    pub async fn remove_dir_refuses_non_empty(fx: &Fixture) {
        fx.fs.mkdir(&fx.path("d")).await.unwrap();
        fx.put("d/inner", b"x").await;
        assert!(fx.fs.remove_dir(&fx.path("d")).await.is_err());
        assert!(fx.fs.stat(&fx.path("d/inner")).await.unwrap().is_some());
        fx.fs.remove_file(&fx.path("d/inner")).await.unwrap();
        fx.fs.remove_dir(&fx.path("d")).await.unwrap();
        assert_eq!(fx.fs.stat(&fx.path("d")).await.unwrap(), None);
    }

    pub async fn remove_all_deletes_a_tree(fx: &Fixture) {
        fx.fs.mkdir(&fx.path("t")).await.unwrap();
        fx.fs.mkdir(&fx.path("t/sub")).await.unwrap();
        fx.put("t/a", b"a").await;
        fx.put("t/sub/b", b"b").await;
        fx.put("keep", b"k").await;
        fx.fs.remove_all(&fx.path("t")).await.unwrap();
        assert_eq!(fx.names(&fx.base).await, ["keep"]);
    }

    pub async fn chmod_sets_mode_or_is_unsupported(fx: &Fixture) {
        fx.put("f", b"x").await;
        let result = fx.fs.chmod(&fx.path("f"), 0o640).await;
        if fx.fs.capabilities().chmod {
            result.unwrap();
            let mode = fx
                .fs
                .stat(&fx.path("f"))
                .await
                .unwrap()
                .unwrap()
                .permissions;
            assert_eq!(mode.map(|m| m & 0o777), Some(0o640));
        } else {
            assert_eq!(result, Err(FsError::Unsupported("chmod")));
        }
    }

    pub async fn set_modified_sets_mtime_or_is_unsupported(fx: &Fixture) {
        fx.put("f", b"x").await;
        let wanted = SystemTime::UNIX_EPOCH + Duration::from_secs(1_500_000_000);
        let result = fx.fs.set_modified(&fx.path("f"), wanted).await;
        if fx.fs.capabilities().set_modified {
            result.unwrap();
            let got = fx
                .fs
                .stat(&fx.path("f"))
                .await
                .unwrap()
                .unwrap()
                .modified
                .unwrap();
            let diff = got.duration_since(wanted).unwrap_or_else(|e| e.duration());
            assert!(diff < Duration::from_secs(2), "mtime off by {diff:?}");
        } else {
            assert_eq!(result, Err(FsError::Unsupported("set_modified")));
        }
    }

    pub async fn full_transfer_round_trips(fx: &Fixture) {
        let data = sample_bytes(300_000);
        let up = Recorder(AtomicU64::new(0), AtomicU64::new(0));
        let mut src = &data[..];
        let sent = fx
            .fs
            .upload(&fx.path("big"), 0, &mut src, &up)
            .await
            .unwrap();
        assert_eq!(sent, data.len() as u64);
        assert_eq!(up.0.load(Ordering::SeqCst), data.len() as u64);

        let down = Recorder(AtomicU64::new(0), AtomicU64::new(0));
        let mut out = Vec::new();
        let got = fx
            .fs
            .download(&fx.path("big"), 0, &mut out, &down)
            .await
            .unwrap();
        assert_eq!(got, data.len() as u64);
        assert_eq!(down.0.load(Ordering::SeqCst), data.len() as u64);
        assert_eq!(sha256_hex(&out), sha256_hex(&data));
    }

    pub async fn upload_overwrites_and_truncates(fx: &Fixture) {
        fx.put("f", b"a much longer first version").await;
        fx.put("f", b"short").await;
        assert_eq!(fx.get("f").await, b"short");
    }

    pub async fn resumed_upload_matches_hash(fx: &Fixture) {
        if !fx.fs.capabilities().resume_upload {
            return;
        }
        let data = sample_bytes(200_000);
        let split = 77_777;
        fx.put("r", &data[..split]).await;
        let mut rest = &data[split..];
        let sent = fx
            .fs
            .upload(&fx.path("r"), split as u64, &mut rest, &NoProgress)
            .await
            .unwrap();
        assert_eq!(sent, (data.len() - split) as u64);
        assert_eq!(sha256_hex(&fx.get("r").await), sha256_hex(&data));
    }

    pub async fn resumed_download_matches_hash(fx: &Fixture) {
        if !fx.fs.capabilities().resume_download {
            return;
        }
        let data = sample_bytes(200_000);
        fx.put("r", &data).await;
        let split = 123_456;
        let mut tail = Vec::new();
        let got = fx
            .fs
            .download(&fx.path("r"), split as u64, &mut tail, &NoProgress)
            .await
            .unwrap();
        assert_eq!(got, (data.len() - split) as u64);
        let mut whole = data[..split].to_vec();
        whole.extend(tail);
        assert_eq!(sha256_hex(&whole), sha256_hex(&data));
    }

    pub async fn unicode_and_space_names(fx: &Fixture) {
        let name = "héllo wörld ☃ (1).txt";
        fx.put(name, b"snow").await;
        assert_eq!(fx.names(&fx.base).await, [name]);
        assert_eq!(fx.get(name).await, b"snow");
        fx.fs.mkdir(&fx.path("dossier été")).await.unwrap();
        fx.fs
            .rename(&fx.path(name), &fx.path("dossier été/déplacé.txt"))
            .await
            .unwrap();
        assert_eq!(fx.names(&fx.path("dossier été")).await, ["déplacé.txt"]);
    }

    pub async fn missing_paths_report_not_found(fx: &Fixture) {
        assert!(matches!(
            fx.fs.list(&fx.path("nope")).await,
            Err(FsError::NotFound(_))
        ));
        let mut out = Vec::new();
        let err = fx
            .fs
            .download(&fx.path("nope"), 0, &mut out, &NoProgress)
            .await;
        assert!(matches!(err, Err(FsError::NotFound(_))), "{err:?}");
    }

    pub async fn home_is_an_existing_directory(fx: &Fixture) {
        let home = fx.fs.home().await.unwrap();
        assert!(fx.fs.stat(&home).await.unwrap().unwrap().is_dir());
    }

    pub async fn large_directory_lists_completely(fx: &Fixture) {
        let count = entry_count();
        fx.fs.mkdir(&fx.path("many")).await.unwrap();
        for i in 0..count {
            fx.put(&format!("many/f{i:05}"), b"").await;
        }
        let entries = fx.fs.list(&fx.path("many")).await.unwrap();
        assert_eq!(entries.len(), count);
    }
}

macro_rules! contract_tests {
    ($setup:path; $($name:ident),* $(,)?) => {
        $(
            #[tokio::test]
            async fn $name() {
                let fx = $setup().await;
                scenarios::$name(&fx).await;
                fx.finish().await;
            }
        )*
    };
}

macro_rules! contract_suite {
    ($module:ident, $setup:path) => {
        mod $module {
            use super::*;
            contract_tests!($setup;
                list_excludes_dot_entries,
                stat_missing_is_none,
                mkdir_twice_fails,
                rename_moves_a_file,
                remove_file_deletes_it,
                remove_dir_refuses_non_empty,
                remove_all_deletes_a_tree,
                chmod_sets_mode_or_is_unsupported,
                set_modified_sets_mtime_or_is_unsupported,
                full_transfer_round_trips,
                upload_overwrites_and_truncates,
                resumed_upload_matches_hash,
                resumed_download_matches_hash,
                unicode_and_space_names,
                missing_paths_report_not_found,
                home_is_an_existing_directory,
                large_directory_lists_completely,
            );
        }
    };
}

async fn rooted_fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let fs = filecargo_remote_fs::RootedFs::new(dir.path()).unwrap();
    Fixture {
        fs: Arc::new(fs),
        base: RemotePath::root(),
        _keep: Box::new(dir),
    }
}

contract_suite!(rooted, rooted_fixture);

#[cfg(feature = "integration")]
mod network {
    use std::sync::atomic::AtomicUsize;

    use filecargo_config::{Auth, MemoryStore, Paths, Protocol, Site};
    use filecargo_remote_fs::{ConnectContext, TrustDecision, connect};

    use super::*;
    use support::TestPrompter;

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    /// Connects `site` (prompts answered with `password`), then makes a private directory.
    async fn fixture_for(mut site: Site, password: &str) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let prompter = TestPrompter::new();
        prompter.trust(TrustDecision::TrustOnce);
        prompter.answer(&[password], false);
        let mut ctx = ConnectContext::new(
            Paths::from_override(Some(dir.path().join("cfg"))),
            Arc::new(MemoryStore::new()),
            prompter,
        );
        ctx.user_known_hosts = None;
        site.auth = Auth::Password { remember: false };
        let session = connect(&site, &ctx).await.unwrap();
        let unique = format!(
            "fc-contract-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        );
        let base = session.info.home.join(&unique).unwrap();
        session.fs.mkdir(&base).await.unwrap();
        Fixture {
            fs: session.fs,
            base,
            _keep: Box::new(dir),
        }
    }

    async fn sftp_fixture() -> Fixture {
        let mut site = Site::new("docker-sftp", Protocol::Sftp, "127.0.0.1");
        site.port = Some(2222);
        site.user = "fcuser".to_owned();
        fixture_for(site, "fcpass").await
    }

    contract_suite!(sftp, sftp_fixture);
}
