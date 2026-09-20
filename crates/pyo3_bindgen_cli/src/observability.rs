use std::io::IsTerminal;
use tracing_subscriber::{fmt::format::FmtSpan, EnvFilter};

fn ansi_enabled(stderr_is_terminal: bool, no_color: bool) -> bool {
    stderr_is_terminal && !no_color
}

/// Install the CLI's structured tracing subscriber.
///
/// `RUST_LOG` controls verbosity. Without it, warnings from the engine remain visible
/// while informational phase and timing events stay quiet. If an embedding application
/// already installed a global subscriber, this function leaves it unchanged.
pub fn init_observability() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("pyo3_bindgen_cli=off,pyo3_bindgen_engine=warn"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_span_events(FmtSpan::CLOSE)
        .with_writer(std::io::stderr)
        .with_ansi(ansi_enabled(
            std::io::stderr().is_terminal(),
            std::env::var_os("NO_COLOR").is_some(),
        ))
        .try_init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ansi_requires_a_terminal_and_no_color_absence() {
        assert!(ansi_enabled(true, false));
        assert!(!ansi_enabled(true, true));
        assert!(!ansi_enabled(false, false));
        assert!(!ansi_enabled(false, true));
    }
}
