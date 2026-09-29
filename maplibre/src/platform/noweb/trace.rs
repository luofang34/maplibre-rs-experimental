//! Optional process-wide Tracy tracing subscriber installation.

/// Installs a Tracy layer as the process-wide tracing subscriber.
///
/// # Panics
/// Panics if a global subscriber is already installed.
#[cfg(feature = "trace")]
pub fn enable_tracing() {
    use tracing_subscriber::{layer::SubscriberExt, Registry};

    let subscriber = Registry::default().with(tracing_tracy::TracyLayer::new());

    tracing::subscriber::set_global_default(subscriber).expect("setting default subscriber failed");
}
