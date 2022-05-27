//! Common example functions

use std::sync::Once;

use tracing::level_filters::LevelFilter;
use tracing_subscriber::EnvFilter;

/// Initialize test environment.
pub fn init() {
	/// Do init only once.
	static SETUP: Once = Once::new();
	SETUP.call_once(|| {
		color_eyre::install().expect("color-eyre");

		let level_filter = LevelFilter::TRACE;
		let filter = EnvFilter::from_default_env()
			.add_directive(level_filter.into())
			.add_directive("want=warn".parse().expect("parse env filter"))
			.add_directive("mio=warn".parse().expect("parse env filter"));
		tracing_subscriber::fmt().with_test_writer().with_env_filter(filter).init();
	});
}
