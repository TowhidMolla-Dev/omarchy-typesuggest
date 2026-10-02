# TypeSuggest

A blazing fast, native **Windows-style hardware keyboard text suggestion tool** for Linux on Wayland (Hyprland / wlroots), written in **Rust**.

---

## ✨ Features

- **Exact Windows Text Suggestions UX:**
  - Floats a compact 3-pill suggestion bar just below the text caret (above it near the bottom of the screen), starting from the very 1st letter typed.
  - The bar highlights its best suggestion from the moment it appears, so `Tab` takes it with no navigation first.
  - **Press `Up`**: Enters suggestion navigation mode without moving the document caret.
  - **Press `Left` / `Right`**: Cycles between the suggestion pills.
  - **Press `Down`, `Escape`, or `Up`**: Cancels suggestion navigation and returns focus directly to the document caret (swallowing the key).
  - **Press `Tab`** while navigating: cycles to the next pill, and commits it if it is the last one, so alternatives can be compared before taking one.
  - **Press `Enter`, `Space`, or `Tab`**: Commits the chosen candidate + trailing space and **swallows the keystroke** (preventing accidental message sending in chat apps like Discord, Slack, WhatsApp, Notion, etc.). Which keys commit and whether a space follows are configurable. `Space` and `Enter` only commit once the bar has been engaged; while it is merely up they keep their normal meaning.
- **Unrestricted Normal Typing & Caret Freedom:**
  - Words commit directly to the application; typing is never trapped in a locked pre-edit state.
  - Regular arrow keys and document navigation remain 100% free and unhindered.
- **Contextual Trigram Language Model & Dynamic Phrase Learning:**
  - Embedded 320,000 contextual word pairs (`bigrams.tsv`) and 319,000 word triples (`trigrams.tsv`) to intelligently bias predictions based on sentence context (e.g. typing "m" after "good" suggests "morning" rather than "much", and after "as soon" it gets closer still).
  - Suggestions are scored by interpolating the three orders of word evidence — the word's own frequency, what follows the previous word, and what follows the previous two words — so a phrase can override a word that is simply more common elsewhere.
  - Learns which word you pick after which, saved to `~/.config/typesuggest/user_bigrams.tsv` (only pairs of dictionary words, so names, codes and passphrases are never stored).
  - Can be toggled on/off on the fly via `--learn`/`--no-learn` or in configuration.
- **Microsecond Prefix Matching:**
  - High-performance in-memory Trie with top 50,000 common English words ranked by frequency.
  - Typical lookup latency: **< 0.005 ms** (5 microseconds).
  - Preserves user capitalization (e.g. `prog` -> `program`, `Prog` -> `Program`, `PROG` -> `PROGRAM`).
- **Identifier-Aware Completion:**
  - Completes only the segment you are typing inside `snake_case`, `kebab-case`, and `camelCase` names, leaving the rest of the identifier untouched (`myProg` -> `myProgram`, `get_prog` -> `get_program`, `HTTPServ` -> `HTTPScreen`).
- **Bidirectional Retro-Editing & Cross-Word Navigation:**
  - Moving the cursor into existing words queries the dictionary for the word at the caret.
  - Committing cleanly replaces both the prefix before the cursor and the suffix after the cursor.
  - Full support for terminal shortcuts (`Ctrl+W`, `Ctrl+U`, `Ctrl+K`, `Ctrl+A`, `Ctrl+E`).
- **Follows Your Omarchy Theme & HiDPI Crispness:**
  - Minimalist 3-pill bar that takes its colors from the current Omarchy theme and follows theme switches automatically. Without Omarchy it uses a dark teal palette (`#060f12` background, `#7fc9c4` cyan accent).
  - Rendered at 2x for crisp text on scaled displays, using Liberation Sans or DejaVu Sans (or Segoe UI if installed under `~/.local/share/fonts/windows`), or any font you configure.
- **Lightweight:**
  - Event-driven: 0% CPU while idle.
  - ~100–120 MB RAM (the 50,000-word dictionary and 320,000-pair language model are kept in memory).
  - Single ~7 MB binary with the dictionary embedded.

---

## ✅ Requirements

- A Wayland compositor with `zwp_input_method_v2` and `zwp_virtual_keyboard_v1` (Hyprland, Sway and other wlroots compositors).
- **Only one input method can run at a time.** Omarchy starts Fcitx5 by default; disable it first, otherwise TypeSuggest exits with *"Input method unavailable"* (and the service is not restarted):
  ```bash
  systemctl --user disable --now omarchy-fcitx5.service
  ```
- Rust toolchain (`cargo`), only to build from source.

## 🧩 App Compatibility

Suggestions appear in apps that support the Wayland `text-input-v3` protocol. In every other app (XWayland programs, games, apps without IME support) TypeSuggest stays out of the way and passes keys through untouched.

| App type | Status | Notes |
|---|---|---|
| Terminals (foot, Ghostty and others with Wayland IME support) | ✅ | |
| GTK 3 / GTK 4 apps | ✅ | |
| Qt 6 apps | ✅ | Needs `QT_IM_MODULE=wayland` or unset (Omarchy sets it to `fcitx`); see below |
| Chromium, Brave, Chrome | ✅ | Needs `--enable-wayland-ime`; see below |
| Electron apps (Notion, Obsidian, VS Code, ...) | ✅ | Needs `--enable-wayland-ime`; see below |
| XWayland apps | ➖ | No suggestions; typing is unaffected |

To turn TypeSuggest off in particular apps, list their window classes in `disabled_apps` (see Configuration below).

On Omarchy, the plugin's *Install TypeSuggest* button offers to make the Chromium, Electron and Qt changes below for you (and its uninstall script undoes them).

**Chromium and Electron apps:** add these lines to the app's flags file (e.g. `~/.config/brave-flags.conf`, `~/.config/chromium-flags.conf`, `~/.config/electron-flags.conf`), then fully restart the app:
```text
--ozone-platform=wayland
--enable-wayland-ime
--force-device-scale-factor=1
```
`--force-device-scale-factor=1` is only needed when the desktop text size is not the default (e.g. Omarchy's *Text size* setting, GNOME's text scaling factor). Chromium applies that factor internally but does not account for it in the caret position it reports, so without the flag the bar drifts right and below the caret. The flag keeps your display scaling; these apps then ignore the custom text size.

**Qt apps on Omarchy:** Omarchy points Qt at Fcitx5 (`QT_IM_MODULE=fcitx` in `/usr/lib/environment.d/10-omarchy-fcitx.conf`). Override it with a file of your own, then log out and back in:
```bash
mkdir -p ~/.config/environment.d
echo 'QT_IM_MODULE=wayland' > ~/.config/environment.d/90-typesuggest.conf
```

---

## 🚀 Installation & Usage

### 1. Install

**Omarchy:** add the [TypeSuggest Omarchy plugin](https://github.com/AbdulrahmanHR/omarchy-typesuggest) for a bar icon with on/off and quick settings; its *Install TypeSuggest* button does everything below for you.

**Prebuilt binary** (x86_64), from the [latest release](https://github.com/AbdulrahmanHR/omarchy-typesuggest/releases/latest):
```bash
curl -fLO https://github.com/AbdulrahmanHR/omarchy-typesuggest/releases/latest/download/typesuggest-x86_64
curl -fLO https://github.com/AbdulrahmanHR/omarchy-typesuggest/releases/latest/download/typesuggest-x86_64.sha256
sha256sum -c typesuggest-x86_64.sha256 && install -Dm755 typesuggest-x86_64 ~/.local/bin/typesuggest
typesuggest --enable-autostart
systemctl --user start typesuggest
```

**From source** (needs Rust):
```bash
cargo build --release
mkdir -p ~/.local/bin
cp target/release/typesuggest ~/.local/bin/typesuggest
typesuggest --enable-autostart   # writes a user unit pointing at this binary
```

### 2. Run Manually
```bash
typesuggest
```

### Uninstall
```bash
typesuggest --disable-autostart
systemctl --user stop typesuggest
rm -f ~/.local/bin/typesuggest ~/.config/systemd/user/typesuggest.service
rm -rf ~/.config/typesuggest      # settings and learned phrases
systemctl --user enable --now omarchy-fcitx5.service   # Omarchy: bring Fcitx5 back if you disabled it
```

### 3. Autostart & Service Management
TypeSuggest provides built-in commands to easily manage background execution and autostart on login:

```bash
# Enable autostart on login (creates user unit and enables it)
typesuggest --enable-autostart

# Check autostart and active running status
typesuggest --status

# Toggle service on or off at runtime
typesuggest --toggle

# Disable autostart on login
typesuggest --disable-autostart
```

You can also control the systemd user service directly:
```bash
systemctl --user status typesuggest.service
systemctl --user start typesuggest.service
systemctl --user stop typesuggest.service
systemctl --user restart typesuggest.service
```

> **Tip:** You can bind `--toggle` to a keyboard shortcut. On Omarchy, add this to `~/.config/hypr/bindings.lua`:
> ```lua
> o.bind("SUPER + CTRL + I", "Toggle TypeSuggest", "typesuggest --toggle")
> ```

---

## ⌨️ Controls & Keybindings

| Key | When Idle / Typing | When in Suggestions Navigation (`Up` active) |
|---|---|---|
| **Letters / Numbers** | Types normally into application | Leaves navigation and types normally |
| **`Up`** | **Enters suggestions navigation** | **Cancels navigation** (swallowed) |
| **`Right`** | Moves caret right in document | Cycles to next suggestion pill |
| **`Left`** | Moves caret left in document | Cycles to previous suggestion pill |
| **`Down` / `Escape`** | Moves caret down / unfocuses | **Cancels navigation** (returns to caret without moving, swallowed) |
| **`Enter` / `Return`** | Inserts newline / submits in app | **Commits candidate + space** (swallowed, zero accidental chat sends) |
| **`Space`** | Inserts space in document | **Commits candidate + space** (swallowed) |
| **`Tab`** | **Commits the highlighted candidate + space** (swallowed) | **Cycles to the next pill**; on the last pill it commits it |

The bar highlights its best suggestion as soon as it appears, so `Tab` takes it without any
navigation first. Once you have entered navigation with `Up` or an arrow key, `Tab` walks the
alternatives instead, and `Enter` or `Space` commits whichever pill is highlighted.

`Space` and `Enter` deliberately keep their normal meaning while the bar is merely up. Every word
in the dictionary is the prefix of some longer entry (`work` of `work-` and `set-up`, `set` of
`setback`), so committing on `Space` would rewrite the word you had just finished typing.

The keys that commit (`accept_keys`) and the space after the word (`trailing_space`) can be changed in the configuration. A key left out of `accept_keys` ends navigation and reaches the app as usual, so with `accept_keys = space, tab` Enter always sends your message.

---

## ⚙️ Configuration

TypeSuggest automatically generates a configuration file at `~/.config/typesuggest/config.toml` on first run. Edits take effect the next time you focus a text field; there is no need to restart the service.

```toml
# ~/.config/typesuggest/config.toml

# Enable or disable dynamic learning of user bigrams/phrases
learn = true

# Minimum number of letters typed before suggestions popup appears (default: 1)
min_prefix_length = 1

# Maximum number of suggestion pills to display in popup bar (1 - 5, default: 3)
max_candidates = 3

# Size of the suggestion bar as a multiplier of the default size (0.5 - 3.0, default: 1.0)
bar_scale = 1.0

# Show the bar "below" (default) or "above" the text caret
bar_position = "below"

# Bar colors: "omarchy" follows the current Omarchy theme, "default" always uses the
# built-in dark teal palette (default: "omarchy")
theme = "omarchy"

# Optional color overrides that win over the theme, as "#rrggbb" or "#rrggbbaa" (default: unset)
color_background = ""
color_border = ""
color_pill = ""
color_pill_border = ""
color_text = ""
color_accent = ""
color_accent_text = ""

# Keys that commit the highlighted suggestion after pressing Up (default: enter, space, tab)
accept_keys = enter, space, tab

# Insert a space after the committed word (default: true)
trailing_space = true

# Hyprland window classes where TypeSuggest stays off (default: none)
disabled_apps =

# Suggest similar words for typos when nothing matches the typed prefix (default: true)
typo_correction = true

# Font for the suggestion bar: a family name or a .ttf/.otf path (default: "" = built-in choice)
font = ""
```

| Setting | Default | Description |
|---|---|---|
| `learn` | `true` | Learn your phrases and rank them higher in future suggestions. |
| `min_prefix_length` | `1` | Letters typed before suggestions appear (1 - 10). |
| `max_candidates` | `3` | Number of suggestion pills (1 - 5). |
| `bar_scale` | `1.0` | Bar size multiplier (0.5 - 3.0). |
| `bar_position` | `"below"` | `"below"` or `"above"` the caret. Near the bottom of the screen the bar always goes above. With `"above"`, moving the mouse over the area above the caret hides the bar so it never blocks clicks (Hyprland gives that area to the input method while the bar is shown). |
| `theme` | `"omarchy"` | `"omarchy"` takes the bar colors from the current Omarchy theme (`$XDG_STATE_HOME/omarchy/current/theme/colors.toml`) and follows theme switches automatically, falling back to the built-in palette when no Omarchy theme is found. `"default"` always uses the built-in dark teal palette. |
| `color_background`, `color_border`, `color_pill`, `color_pill_border`, `color_text`, `color_accent`, `color_accent_text` | unset | Override single colors (bar background and border, unselected pill background, border and text, selected pill background and text) as `"#rrggbb"` or `"#rrggbbaa"`. Invalid values are ignored. |
| `accept_keys` | `enter, space, tab` | Keys that commit the highlighted suggestion. Any of `enter`, `space`, `tab`; also written as `["enter", "tab"]`. Other keys end navigation and reach the app. |
| `trailing_space` | `true` | Add a space after the committed word. The accept key itself is still swallowed. |
| `disabled_apps` | none | Window classes where TypeSuggest does nothing at all, e.g. `code, org.wezfurlong.wezterm` or `["code", "steam*"]`. Matching is case-insensitive and a trailing `*` matches any suffix. Find a window's class with `hyprctl activewindow`. |
| `typo_correction` | `true` | When nothing starts with the typed letters, suggest similar words (`teh` -> `the`). |
| `font` | `""` | Empty uses Segoe UI (if installed under `~/.local/share/fonts/windows`), then Liberation Sans, then DejaVu Sans. A family name (e.g. `"Inter"`, or a fontconfig pattern like `"Inter:bold"`) is looked up with `fc-match`; a value containing `/` is a font file path (`~/` allowed). Fonts that cannot be loaded fall back to the default with a warning in the log. |

Omarchy theme switches are picked up the same way, the next time a text field is focused. Command-line options (below) keep overriding the file after it is reloaded.

### CLI Commands & Overrides
Any setting or service operation can be controlled from the command line:
```bash
typesuggest --enable-autostart   # Enable automatic startup on login
typesuggest --disable-autostart  # Disable automatic startup
typesuggest --status             # Check autostart and service running state
typesuggest --toggle             # Quickly toggle suggestions on/off
typesuggest --no-learn           # Run in private / incognito mode without updating bigrams
typesuggest --show-learned       # Display all learned bigrams and usage counts
typesuggest --clear-learned      # Reset all dynamically learned phrases
typesuggest --min-prefix 2       # Only show suggestions after typing at least 2 letters
typesuggest --max-candidates 5   # Display up to 5 pills instead of 3
typesuggest --bar-scale 1.25     # Make the suggestion bar 25% larger
typesuggest --bar-position above # Show the suggestion bar above the caret
typesuggest --set bar_scale 1.25 # Change a setting in config.toml (validated)
typesuggest --config-json        # Print the settings in config.toml as JSON
typesuggest --no-typo-correction # Only suggest words that start with the typed letters
```

---

## 🔒 Privacy & Security

TypeSuggest is an input method, so **every key you press passes through it** before reaching the app. It is a local program: it opens no network connections (the systemd unit only allows local sockets), and it keeps the text of the line you are typing in memory only while that field is focused.

- **Stored on disk:** only learned word pairs, and only when both words are in the dictionary (or your `words.txt`), in `~/.config/typesuggest/user_bigrams.tsv` (mode 600). Turn it off with `learn = false`; view it with `typesuggest --show-learned`; erase it with `typesuggest --clear-learned`.
- **Passwords:** suggestions switch off and nothing is recorded when:
  - an app marks the field as a password, PIN or sensitive data (GTK, Qt, Chromium and Electron password fields);
  - a terminal is reading a password with echo turned off (sudo, ssh, passwd, `read -s`, git, mysql, …), including inside tmux, screen and zellij;
  - a known credential program runs in the focused terminal (sudo, su, doas, passwd, pkexec, ssh, pinentry, …), or an authentication dialog (polkit, pinentry) is focused.
- **Limits:** a password prompt that is neither started from the terminal nor reading in line mode is not recognised, e.g. GnuPG's curses pinentry (started by gpg-agent, drawing its own input). Add such apps to `disabled_apps` if you need certainty.
- No core dumps are written for the TypeSuggest process.

---

## 📖 Custom Vocabulary

Add your own custom words, names, or technical terms in `~/.config/typesuggest/words.txt` (one per line). They are automatically given top suggestion priority on startup.

Example:
```text
# ~/.config/typesuggest/words.txt
omarchy
hyprland
kubernetes
wayland
```

---

## 📄 License & Credits

The code is MIT licensed; see [LICENSE](LICENSE). The built-in data keeps its own licenses (details and changes in [`data/README.md`](data/README.md)):

- **Word list:** [FrequencyWords](https://github.com/hermitdave/FrequencyWords) by Hermit Dave, from OpenSubtitles 2018 — [CC BY-SA 4.0](https://creativecommons.org/licenses/by-sa/4.0/)
- **Word pairs:** built from [Tatoeba](https://tatoeba.org) English sentences — [CC BY 2.0 FR](https://creativecommons.org/licenses/by/2.0/fr/)

The Rust libraries compiled into the binary and their licenses are listed in [THIRD_PARTY_LICENSES.md](THIRD_PARTY_LICENSES.md).

