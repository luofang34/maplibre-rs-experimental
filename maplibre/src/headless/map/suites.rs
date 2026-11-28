//! Headless map test suites and injected loader fixtures.

mod injected_loader;
pub(crate) mod symbol_worker;
mod terrain_coverage;
mod tests;
#[cfg(feature = "experimental")]
mod vertical_perspective;
