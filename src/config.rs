// SPDX-License-Identifier: MPL-2.0

use cosmic::cosmic_config::{self, CosmicConfigEntry, cosmic_config_derive::CosmicConfigEntry};

#[derive(Debug, Clone, CosmicConfigEntry, Eq, PartialEq)]
#[version = 1]
pub struct Config {
  // pub icon_position: enum IconPosition {Left, Right},
  pub max_width: u32,
  // pub format: Option<String>,
}

impl Default for Config {
  fn default() -> Self {
    Self { max_width: 400 }
  }
}
