//! The toolbar's content and the session indicator.

use filecargo_app_core::prelude::*;

/// What the indicator at the right of the toolbar says for the session.
pub fn session_label(state: &AppState) -> String {
    let name = |site: SiteId| {
        state
            .servers
            .site(site)
            .map_or_else(|| "?".to_owned(), |s| s.name.clone())
    };
    match &state.session {
        SessionState::Disconnected => "Not connected".to_owned(),
        SessionState::Connecting { site, step } => {
            let step = match step {
                ConnectStep::Resolving => "resolving",
                ConnectStep::Connecting => "connecting",
                ConnectStep::Authenticating => "authenticating",
                ConnectStep::Listing => "listing",
            };
            format!("Connecting to {} ({step})…", name(*site))
        }
        SessionState::Connected { site, .. } => {
            let protocol = state.servers.site(*site).map_or("", |s| match s.protocol {
                Protocol::Sftp => "SFTP",
                Protocol::Ftp => "FTP",
                Protocol::FtpsExplicit => "FTPS explicit",
                Protocol::FtpsImplicit => "FTPS implicit",
            });
            format!("● {} ({protocol})", name(*site))
        }
        SessionState::Failed { site, error } => format!("✗ {}: {error}", name(*site)),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn state() -> AppState {
        AppState {
            startup_error: None,
            servers: Arc::new(ServerTree::default()),
            settings: Arc::new(Settings::default()),
            session: SessionState::Disconnected,
            local: filecargo_app_core::Pane {
                path: "/".into(),
                entries: Arc::from(Vec::new()),
                sort: Sort::default(),
                loading: false,
                error: None,
                generation: 0,
            },
            remote: None,
            queue: Arc::new(QueueSnapshot::default()),
            terminal: TerminalState::NotAvailable,
            prompt: None,
            notices: Vec::new(),
            log_generation: 0,
        }
    }

    #[test]
    fn the_indicator_follows_the_session() {
        let mut state = state();
        assert_eq!(session_label(&state), "Not connected");
        state.session = SessionState::Failed {
            site: SiteId::new(),
            error: "refused".into(),
        };
        assert_eq!(session_label(&state), "✗ ?: refused");
    }
}
