#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]
//! Headless harness: a temp config dir, an in-memory keychain, and helpers that wait on the
//! published snapshots (blocking on the app's own runtime, so tests are plain `#[test]`s).

use std::sync::Arc;
use std::time::Duration;

use filecargo_app_core::{App, AppHandle, AppState, StartOptions};
use filecargo_config::{MemoryStore, Paths};

pub struct Fixture {
    pub app: AppHandle,
    pub config: tempfile::TempDir,
}

impl Fixture {
    pub fn new() -> Self {
        Self::with(|_| {})
    }

    /// Starts an app on a fresh config dir after letting `prepare` write files into it.
    pub fn with(prepare: impl FnOnce(&std::path::Path)) -> Self {
        let config = tempfile::tempdir().unwrap();
        prepare(config.path());
        let app = App::start(StartOptions {
            paths: Some(Paths::from_override(Some(config.path().to_path_buf()))),
            secrets: Some(Arc::new(MemoryStore::new())),
            connector: None,
        })
        .unwrap();
        Self { app, config }
    }

    pub fn state(&self) -> Arc<AppState> {
        self.app.state().borrow().clone()
    }

    /// Waits (up to 5 s) for a snapshot satisfying `condition` and returns it.
    pub fn wait_for(&self, what: &str, condition: impl Fn(&AppState) -> bool) -> Arc<AppState> {
        let mut rx = self.app.state();
        self.app.runtime().block_on(async {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            loop {
                {
                    let state = rx.borrow_and_update().clone();
                    if condition(&state) {
                        return state;
                    }
                }
                if tokio::time::timeout_at(deadline, rx.changed())
                    .await
                    .is_err()
                {
                    panic!(
                        "timed out waiting for {what}; last state: {:#?}",
                        rx.borrow()
                    );
                }
            }
        })
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.app.clone().shutdown(Duration::from_secs(2));
    }
}

impl Fixture {
    /// Waits (up to 5 s) for a spawned task and returns its result.
    pub fn join<T: Send + 'static>(&self, task: tokio::task::JoinHandle<T>) -> T {
        self.app.runtime().block_on(async {
            tokio::time::timeout(Duration::from_secs(5), task)
                .await
                .expect("the task did not finish within 5 s")
                .unwrap()
        })
    }

    /// Waits for a prompt other than `after` to be showing.
    pub fn wait_for_prompt_after(
        &self,
        after: Option<filecargo_app_core::PromptId>,
    ) -> filecargo_app_core::Prompt {
        let state = self.wait_for("a new prompt", |s| {
            s.prompt.as_ref().is_some_and(|p| Some(p.id) != after)
        });
        state.prompt.clone().unwrap()
    }
}
