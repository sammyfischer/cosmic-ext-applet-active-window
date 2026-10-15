// SPDX-License-Identifier: MPL-2.0

use cosmic::cosmic_config::{self, CosmicConfigEntry, cosmic_config_derive::CosmicConfigEntry};

#[derive(Debug, Clone, CosmicConfigEntry, Eq, PartialEq)]
#[version = 1]
pub struct Config {
  // demo: String,
  // icon_position: enum {Left, Right},
  pub max_width: u32,
  // text_mode: enum {AppName, TitleText, Both},
}

impl Default for Config {
  fn default() -> Self {
    Self { max_width: 250 }
  }
}
