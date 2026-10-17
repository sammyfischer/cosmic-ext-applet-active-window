// SPDX-License-Identifier: MPL-2.0

mod app;
mod config;
mod i18n;
mod template;
mod wayland;

fn main() -> cosmic::iced::Result {
  // Get the system's preferred languages.
  let requested_languages = i18n_embed::DesktopLanguageRequester::requested_languages();

  // Enable localizations to be applied.
  i18n::init(&requested_languages);

  // Starts the applet's event loop with `()` as the application's flags.
  cosmic::applet::run::<app::AppModel>(())
}

/// Simple temporary log function. Logs are written to
/// `/tmp/active-window-applet.log`.
///
/// TODO: delete this on first release
pub fn log(msg: &str) {
  use std::io::Write;
  if let Ok(mut file) = std::fs::OpenOptions::new()
    .create(true)
    .append(true)
    .open("/tmp/active-window-applet.log")
  {
    let _ = writeln!(file, "{msg}");
  }
}
