# Active Window

Shows which window is active and adds a control panel to conveniently manage the active application.

## Features

Applet

- see the current active window in the panel or dock
- shows the app icon and window title
- right click to change settings
  - customize the text with a template using the variables `{name}` `{title}` and `{count}` (use `{{` and `}}` to include literal braces in the template)

Control panel

- view full window title
- control the current window: minimize, maximize, close, etc.
- run desktop actions for the current app
- switch between windows of the same app
  - quick controls to minimize/restore, pin/unpin, and close each window right from the switcher

## Installation

Use the [justfile](./Justfile) to build and install. Use `just build-install` to build with the release profile and install system-wide.

- `just` builds the application with the default `just build-release` recipe
- `just install` installs the project into the system
- `just vendor` creates a vendored tarball
- `just build-vendored` compiles with vendored dependencies from that tarball
- `just uninstall` uninstalls by simply deleting the installed files. It does not remove config data.

## Translators

[Fluent][fluent] is used for localization of the software. Fluent's translation files are found in the [i18n directory](./i18n). New translations may copy the [English (en) localization](./i18n/en) of the project, rename `en` to the desired [ISO 639-1 language code][iso-codes], and then translations can be provided for each [message identifier][fluent-guide]. If no translation is necessary, the message may be omitted.

## Packaging

If packaging for a Linux distribution, vendor dependencies locally with the `vendor` rule, and build with the vendored sources using the `build-vendored` rule. When installing files, use the `rootdir` and `prefix` variables to change installation paths.

```sh
just vendor
just build-vendored
just rootdir=debian/cosmic-applet-active-window prefix=/usr install
```

It is recommended to build a source tarball with the vendored dependencies, which can typically be done by running `just vendor` on the host system before it enters the build environment.

## Developers

Developers should install [rustup][rustup] and configure their editor to use [rust-analyzer][rust-analyzer].

[fluent]: https://projectfluent.org/
[fluent-guide]: https://projectfluent.org/fluent/guide/hello.html
[iso-codes]: https://en.wikipedia.org/wiki/List_of_ISO_639-1_codes
[just]: https://github.com/casey/just
[rustup]: https://rustup.rs/
[rust-analyzer]: https://rust-analyzer.github.io/
[sccache]: https://github.com/mozilla/sccache
