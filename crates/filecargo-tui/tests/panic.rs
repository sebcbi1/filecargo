#![allow(clippy::unwrap_used, clippy::expect_used)]
//! AC5: a panic inside the render code restores the terminal first. This is the only test in
//! its binary because the panic hook is process-global.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use filecargo_tui::guard::install_panic_restore;

#[test]
fn a_panic_while_rendering_runs_the_restore_before_the_message_is_printed() {
    let restored = Arc::new(AtomicUsize::new(0));
    let counter = restored.clone();
    let order = Arc::new(std::sync::Mutex::new(Vec::<&'static str>::new()));
    let log = order.clone();

    // a stand-in for the default hook, recording that it ran after the restore
    std::panic::set_hook(Box::new(move |_| log.lock().unwrap().push("message")));
    let log = order.clone();
    install_panic_restore(move || {
        counter.fetch_add(1, Ordering::SeqCst);
        log.lock().unwrap().push("restore");
    });

    let render = || -> () { panic!("boom while drawing") };
    let result = std::panic::catch_unwind(render);
    assert!(result.is_err());
    assert_eq!(
        restored.load(Ordering::SeqCst),
        1,
        "the terminal must be restored"
    );
    assert_eq!(
        *order.lock().unwrap(),
        ["restore", "message"],
        "restore first, so the message stays readable"
    );
    let _ = std::panic::take_hook();
}

#[test]
fn leaving_without_a_terminal_does_not_panic() {
    // `leave` is also called on error paths where the terminal never came up (CI, pipes)
    filecargo_tui::guard::leave();
    filecargo_tui::guard::leave();
}
