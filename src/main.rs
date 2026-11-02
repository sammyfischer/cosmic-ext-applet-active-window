// SPDX-License-Identifier: MPL-2.0

#![allow(clippy::match_bool)]
#![allow(clippy::too_many_lines)]

mod app;
mod components;
mod config;
mod i18n;
mod template;
mod wayland;

fn main() -> cosmic::iced::Result {
  // Get the system's preferred languages.
  let requested_languages = i18n_embed::DesktopLanguageRequester::requested_languages();

  // Enable localizations to be applied.
  i18n::init(&requested_languages);

  // Initialize log tracing.
  init_logs();

  // Starts the applet's event loop with `()` as the application's flags.
  cosmic::applet::run::<app::AppModel>(())
}

fn init_logs() {
  use tracing::level_filters::LevelFilter;
  use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

  let filter = EnvFilter::builder()
    .with_default_directive(LevelFilter::INFO.into())
    .from_env_lossy()
    // cosmic::applet spams the message "40". this filters out all info messages from
    // cosmic::applet, which may remove some useful messages but shouldn't be a huge deal
    .add_directive("cosmic::applet=warn".parse().unwrap());

  // try journald
  let journald = tracing_journald::layer()
    .ok()
    .map(|layer| layer.with_syslog_identifier(env!("CARGO_PKG_NAME").to_string()));

  // use stderr if journald can't connect
  let stderr = journald
    .is_none()
    .then(|| tracing_subscriber::fmt::layer().with_ansi(false));

  tracing_subscriber::registry()
    .with(filter)
    .with(journald)
    .with(stderr)
    .init();

  tracing::info!(
    "starting {} ({})",
    env!("CARGO_PKG_NAME"),
    env!("CARGO_PKG_VERSION")
  );
}
