use std::path::{Path, PathBuf};
use typesuggest::config::{
    BarPosition, CliOverrides, Config, ConfigSource, config_line, parse_bar_scale, with_setting,
};
use typesuggest::dict::{Dictionary, is_learnable_word};
use typesuggest::engine::Engine;
use wayland_client::Connection;

const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Exit status when another input method holds the seat (see RestartPreventExitStatus in the unit)
const EXIT_IME_UNAVAILABLE: i32 = 3;
const DEFAULT_SERVICE_UNIT: &str = include_str!("../typesuggest.service");

/// Where distribution packages (e.g. the AUR package) install the unit
const PACKAGED_SERVICE_UNIT: &str = "/usr/lib/systemd/user/typesuggest.service";

/// The unit for this binary: the packaged one points at /usr/bin; a manual install (e.g. into
/// ~/.local/bin) gets ExecStart pointed at wherever this executable actually is
fn service_unit_for(exe: &Path) -> String {
    let exe = exe.to_string_lossy();
    let exec = if exe.contains(char::is_whitespace) {
        format!("\"{}\"", exe)
    } else {
        exe.into_owned()
    };
    DEFAULT_SERVICE_UNIT.replace(
        "ExecStart=/usr/bin/typesuggest",
        &format!("ExecStart={}", exec),
    )
}

/// Make sure a `typesuggest.service` exists: the packaged one, or else a user unit in
/// ~/.config/systemd/user (an existing user unit is never overwritten)
fn ensure_systemd_service(config_dir: &Path) -> std::io::Result<PathBuf> {
    let service_dir = config_dir.join("systemd").join("user");
    let service_file = service_dir.join("typesuggest.service");
    if service_file.exists() || Path::new(PACKAGED_SERVICE_UNIT).exists() {
        return Ok(if service_file.exists() {
            service_file
        } else {
            PathBuf::from(PACKAGED_SERVICE_UNIT)
        });
    }
    std::fs::create_dir_all(&service_dir)?;
    let exe = std::env::current_exe()
        .and_then(|p| p.canonicalize())
        .unwrap_or_else(|_| PathBuf::from("/usr/bin/typesuggest"));
    std::fs::write(&service_file, service_unit_for(&exe))?;
    Ok(service_file)
}

/// Atomically store `key = value` in config.toml (validated; other lines are kept)
fn set_config_value(path: &Path, key: &str, value: &str) -> Result<String, String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let line = config_line(key, value)?;
    let content = std::fs::read_to_string(path)
        .unwrap_or_else(|_| Config::default_config_file_content().to_string());
    let updated = with_setting(&content, key, &line);

    let temp = path.with_extension("toml.tmp");
    let _ = std::fs::remove_file(&temp);
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(&temp)
        .and_then(|mut file| {
            file.write_all(updated.as_bytes())?;
            file.sync_all()
        })
        .and_then(|()| std::fs::rename(&temp, path));
    if let Err(e) = written {
        let _ = std::fs::remove_file(&temp);
        return Err(format!("cannot write {:?}: {}", path, e));
    }
    Ok(line)
}

fn enable_autostart(config_dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let service_file = ensure_systemd_service(config_dir)?;
    if let Ok(metadata) = std::fs::metadata(&service_file)
        && metadata.len() == 0
    {
        std::fs::write(&service_file, DEFAULT_SERVICE_UNIT)?;
    }
    let _ = std::process::Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status();
    let status = std::process::Command::new("systemctl")
        .args(["--user", "enable", "typesuggest.service"])
        .status()?;
    if status.success() {
        println!("Autostart enabled: typesuggest will start automatically on login.");
    } else {
        eprintln!("Failed to enable typesuggest.service via systemctl.");
    }
    Ok(())
}

fn disable_autostart() -> Result<(), Box<dyn std::error::Error>> {
    let status = std::process::Command::new("systemctl")
        .args(["--user", "disable", "typesuggest.service"])
        .status()?;
    if status.success() {
        println!("Autostart disabled: typesuggest will not start automatically on login.");
    } else {
        eprintln!("Failed to disable typesuggest.service via systemctl.");
    }
    Ok(())
}

fn print_autostart_status() {
    let is_enabled = std::process::Command::new("systemctl")
        .args(["--user", "is-enabled", "typesuggest.service"])
        .output();
    let enabled_str = match &is_enabled {
        Ok(out) => {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if s.is_empty() {
                String::from_utf8_lossy(&out.stderr).trim().to_string()
            } else {
                s
            }
        }
        Err(e) => format!("unknown ({})", e),
    };

    let is_active = std::process::Command::new("systemctl")
        .args(["--user", "is-active", "typesuggest.service"])
        .output();
    let active_str = match &is_active {
        Ok(out) => {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if s.is_empty() {
                String::from_utf8_lossy(&out.stderr).trim().to_string()
            } else {
                s
            }
        }
        Err(e) => format!("unknown ({})", e),
    };

    println!("TypeSuggest Service Status:");
    println!("  Autostart: {}", enabled_str);
    println!("  Service:   {}", active_str);
}

fn toggle_service(config_dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let is_active = std::process::Command::new("systemctl")
        .args(["--user", "is-active", "typesuggest.service"])
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).trim() == "active")
        .unwrap_or(false);

    if is_active {
        let status = std::process::Command::new("systemctl")
            .args(["--user", "stop", "typesuggest.service"])
            .status()?;
        if status.success() {
            println!("typesuggest service stopped.");
        } else {
            eprintln!("Failed to stop typesuggest.service via systemctl.");
        }
    } else {
        let _ = ensure_systemd_service(config_dir);
        let _ = std::process::Command::new("systemctl")
            .args(["--user", "daemon-reload"])
            .status();
        let status = std::process::Command::new("systemctl")
            .args(["--user", "start", "typesuggest.service"])
            .status()?;
        if status.success() {
            println!("typesuggest service started.");
        } else {
            eprintln!("Failed to start typesuggest.service via systemctl.");
        }
    }
    Ok(())
}

fn print_help() {
    println!(
        "typesuggest v{} - Native Windows-style hardware text suggestions for Wayland",
        VERSION
    );
    println!();
    println!("USAGE:");
    println!("    typesuggest [OPTIONS]");
    println!();
    println!("OPTIONS:");
    println!("    -h, --help               Print this help message");
    println!("    -v, --version            Print version information");
    println!("    --enable-autostart       Enable systemd user autostart on login");
    println!("    --disable-autostart      Disable systemd user autostart");
    println!("    --status, --autostart-status Check autostart and running status");
    println!("    --toggle                 Toggle service on/off (start/stop)");
    println!("    --learn                  Enable dynamic user phrase learning (default)");
    println!("    --no-learn               Disable dynamic user phrase learning");
    println!("    --config-json            Print the settings in config.toml as JSON, then exit");
    println!(
        "    --set <key> <value>      Change one setting in config.toml (validated), then exit"
    );
    println!("    --show-learned           Display learned phrases and bigrams, then exit");
    println!("    --clear-learned          Clear all learned user phrases, then exit");
    println!(
        "    --min-prefix <N>         Minimum characters typed before suggestions appear (default: 1)"
    );
    println!(
        "    --max-candidates <N>     Maximum candidates displayed in popup (1-5, default: 3)"
    );
    println!("    --bar-scale <X>          Suggestion bar size multiplier (0.5-3.0, default: 1.0)");
    println!(
        "    --bar-position <POS>     Show the bar \"below\" (default) or \"above\" the caret"
    );
    println!("    --no-typo-correction     Do not suggest similar words for typos");
    println!();
    println!("AUTOSTART & SERVICE:");
    println!("    typesuggest --enable-autostart   # Enable automatic startup on login");
    println!("    typesuggest --disable-autostart  # Disable automatic startup");
    println!("    typesuggest --status             # Check autostart and active status");
    println!("    typesuggest --toggle             # Quickly toggle suggestions on/off");
    println!();
    println!("CONFIGURATION:");
    println!("    Config file:     ~/.config/typesuggest/config.toml");
    println!("    Custom words:    ~/.config/typesuggest/words.txt");
    println!("    Learned phrases: ~/.config/typesuggest/user_bigrams.tsv");
    println!("    Systemd service: ~/.config/systemd/user/typesuggest.service");
    println!("    Theme colors:    $XDG_STATE_HOME/omarchy/current/theme/colors.toml (Omarchy)");
    println!();
    println!("    Edits to config.toml apply the next time a text field is focused (no restart),");
    println!(
        "    and so do Omarchy theme switches. Command-line options keep overriding the file."
    );
    println!();
    println!("CONFIG FILE SETTINGS (defaults in parentheses):");
    println!("    learn, min_prefix_length, max_candidates, bar_scale   Same as the options above");
    println!(
        "    theme = omarchy | default        Follow Omarchy's theme or use built-in colors (omarchy)"
    );
    println!("    color_background, color_border, color_pill, color_pill_border, color_text,");
    println!("    color_accent, color_accent_text  \"#rrggbb\" / \"#rrggbbaa\" overrides (unset)");
    println!(
        "    accept_keys = enter, space, tab  Keys that commit the highlighted word (all three)"
    );
    println!("    trailing_space = true | false    Add a space after the committed word (true)");
    println!(
        "    disabled_apps = code, steam*     Window classes where typesuggest stays off (none)"
    );
    println!("    typo_correction = true | false   Suggest similar words for typos (true)");
    println!(
        "    font = \"Inter\"                   Font family or .ttf/.otf path (\"\": built-in)"
    );
    println!();
    println!("BEHAVIOR:");
    println!(
        "    - Displays a compact suggestion bar above/at the text caret starting from 1st letter"
    );
    println!("    - Press Up arrow to navigate into suggestions");
    println!("    - Press Left / Right arrow to cycle between suggestions");
    println!("    - Press Down arrow, Escape, or Up arrow again to cancel navigation");
    println!(
        "    - Press an accept key (Enter, Space, or Tab by default) to commit the selected word"
    );
    println!("      (swallowing the key); any other key leaves navigation and reaches the app");
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config_dir =
        dirs_config_dir().ok_or("Cannot locate the config directory: HOME is not set")?;
    let app_config_dir = config_dir.join("typesuggest");
    let config_file_path = app_config_dir.join("config.toml");

    // 1. Ensure ~/.config/typesuggest exists with restricted permissions and has default config.toml if missing
    let _ = std::fs::create_dir_all(&app_config_dir);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&app_config_dir, std::fs::Permissions::from_mode(0o700));
    }
    if !config_file_path.exists() {
        let _ = std::fs::write(&config_file_path, Config::default_config_file_content());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ =
                std::fs::set_permissions(&config_file_path, std::fs::Permissions::from_mode(0o600));
        }
    }

    // 2. Parse CLI overrides
    let mut overrides = CliOverrides::default();
    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--version" | "-v" => {
                println!("typesuggest v{}", VERSION);
                return Ok(());
            }
            "--help" | "-h" => {
                print_help();
                return Ok(());
            }
            "--enable-autostart" | "--autostart-enable" => {
                enable_autostart(&config_dir)?;
                return Ok(());
            }
            "--disable-autostart" | "--autostart-disable" => {
                disable_autostart()?;
                return Ok(());
            }
            "--status" | "--autostart-status" | "status" => {
                print_autostart_status();
                return Ok(());
            }
            "--toggle" | "toggle" => {
                toggle_service(&config_dir)?;
                return Ok(());
            }
            "--autostart" | "autostart" => {
                if i + 1 < args.len() {
                    i += 1;
                    match args[i].as_str() {
                        "enable" => {
                            enable_autostart(&config_dir)?;
                            return Ok(());
                        }
                        "disable" => {
                            disable_autostart()?;
                            return Ok(());
                        }
                        "status" => {
                            print_autostart_status();
                            return Ok(());
                        }
                        sub => {
                            eprintln!(
                                "Unknown autostart subcommand: '{}'. Use 'enable', 'disable', or 'status'.",
                                sub
                            );
                            return Ok(());
                        }
                    }
                } else {
                    eprintln!("Missing autostart action. Use 'enable', 'disable', or 'status'.");
                    return Ok(());
                }
            }
            arg if arg.starts_with("--autostart=") => {
                match arg.trim_start_matches("--autostart=") {
                    "enable" => {
                        enable_autostart(&config_dir)?;
                        return Ok(());
                    }
                    "disable" => {
                        disable_autostart()?;
                        return Ok(());
                    }
                    "status" => {
                        print_autostart_status();
                        return Ok(());
                    }
                    sub => {
                        eprintln!(
                            "Unknown autostart subcommand: '{}'. Use 'enable', 'disable', or 'status'.",
                            sub
                        );
                        return Ok(());
                    }
                }
            }
            "--config-json" => {
                println!("{}", Config::load_from_file(&config_file_path).to_json());
                return Ok(());
            }
            "--set" => {
                let (Some(key), Some(value)) = (args.get(i + 1), args.get(i + 2)) else {
                    eprintln!("Usage: typesuggest --set <key> <value>");
                    std::process::exit(2);
                };
                match set_config_value(&config_file_path, key, value) {
                    Ok(line) => {
                        println!("{} (applies the next time a text field is focused)", line);
                        return Ok(());
                    }
                    Err(e) => {
                        eprintln!("typesuggest: {}", e);
                        std::process::exit(2);
                    }
                }
            }
            "--show-learned" => {
                let user_bigrams_path = app_config_dir.join("user_bigrams.tsv");
                println!("Learned user phrases (stored in {:?}):", user_bigrams_path);
                if user_bigrams_path.exists() {
                    match std::fs::read_to_string(&user_bigrams_path) {
                        Ok(content) if !content.trim().is_empty() => {
                            let mut count = 0;
                            for line in content.lines() {
                                let parts: Vec<&str> = line.split('\t').collect();
                                // Same validation as loading, so a tampered file cannot print
                                // terminal escape sequences
                                if parts.len() >= 3
                                    && is_learnable_word(parts[0].trim())
                                    && is_learnable_word(parts[1].trim())
                                    && let Ok(times) = parts[2].trim().parse::<u32>()
                                {
                                    println!(
                                        "  \"{}\" -> \"{}\" (typed {} time(s))",
                                        parts[0].trim(),
                                        parts[1].trim(),
                                        times
                                    );
                                    count += 1;
                                }
                            }
                            if count == 0 {
                                println!("  (No learned phrases recorded yet)");
                            }
                        }
                        _ => println!("  (No learned phrases recorded yet)"),
                    }
                } else {
                    println!("  (No learned phrases recorded yet)");
                }
                return Ok(());
            }
            "--clear-learned" => {
                let user_bigrams_path = app_config_dir.join("user_bigrams.tsv");
                let _ = std::fs::remove_file(user_bigrams_path.with_extension("tsv.tmp"));
                if user_bigrams_path.exists() {
                    let _ = std::fs::remove_file(&user_bigrams_path);
                    println!(
                        "Successfully cleared learned user phrases ({:?}).",
                        user_bigrams_path
                    );
                } else {
                    println!("No learned phrases file found at {:?}.", user_bigrams_path);
                }
                // A running daemon still holds the phrases in memory and would write them back
                // on the next accepted suggestion
                let restarted = std::process::Command::new("systemctl")
                    .args(["--user", "try-restart", "typesuggest.service"])
                    .status()
                    .is_ok_and(|s| s.success());
                if !restarted {
                    println!(
                        "If typesuggest is running outside systemd, restart it to forget them."
                    );
                }
                return Ok(());
            }
            "--learn" => {
                overrides.learn = Some(true);
            }
            "--no-learn" => {
                overrides.learn = Some(false);
            }
            "--no-typo-correction" => {
                overrides.typo_correction = Some(false);
            }
            "--min-prefix" => {
                if i + 1 < args.len() {
                    i += 1;
                    if let Ok(n) = args[i].parse::<usize>() {
                        overrides.min_prefix_length = Some(n.clamp(1, 10));
                    }
                }
            }
            "--max-candidates" => {
                if i + 1 < args.len() {
                    i += 1;
                    if let Ok(n) = args[i].parse::<usize>() {
                        overrides.max_candidates = Some(n.clamp(1, 5));
                    }
                }
            }
            arg if arg.starts_with("--min-prefix=") => {
                if let Ok(n) = arg.trim_start_matches("--min-prefix=").parse::<usize>() {
                    overrides.min_prefix_length = Some(n.clamp(1, 10));
                }
            }
            arg if arg.starts_with("--max-candidates=") => {
                if let Ok(n) = arg.trim_start_matches("--max-candidates=").parse::<usize>() {
                    overrides.max_candidates = Some(n.clamp(1, 5));
                }
            }
            "--bar-scale" => {
                if i + 1 < args.len() {
                    i += 1;
                    if let Some(scale) = parse_bar_scale(&args[i]) {
                        overrides.bar_scale = Some(scale);
                    }
                }
            }
            arg if arg.starts_with("--bar-scale=") => {
                if let Some(scale) = parse_bar_scale(arg.trim_start_matches("--bar-scale=")) {
                    overrides.bar_scale = Some(scale);
                }
            }
            "--bar-position" => {
                if i + 1 < args.len() {
                    i += 1;
                    overrides.bar_position = BarPosition::parse(&args[i]);
                }
            }
            arg if arg.starts_with("--bar-position=") => {
                overrides.bar_position =
                    BarPosition::parse(arg.trim_start_matches("--bar-position="));
            }
            _ => {}
        }
        i += 1;
    }

    // 3. Load config from file; the CLI overrides keep winning when it is reloaded live
    let mut config_source = ConfigSource::new(config_file_path, overrides);
    let config = config_source.load();

    // Every keystroke passes through this process: keep it out of core dumps
    let _ = rustix::process::set_dumpable_behavior(rustix::process::DumpableBehavior::NotDumpable);

    println!("Starting typesuggest v{}...", VERSION);
    println!("Configuration:");
    println!(
        "  - Dynamic phrase learning: {}",
        if config.learn { "enabled" } else { "disabled" }
    );
    println!(
        "  - Min prefix length: {} letter(s)",
        config.min_prefix_length
    );
    println!("  - Max candidates: {}", config.max_candidates);
    println!("  - Bar size: {}x", config.bar_scale);
    println!("  - Bar position: {}", config.bar_position.name());
    println!("  - Theme: {}", config.theme.name());
    println!("  - Accept keys: {}", config.accept_keys.names().join(", "));
    println!(
        "  - Trailing space: {}",
        if config.trailing_space { "on" } else { "off" }
    );
    println!(
        "  - Typo correction: {}",
        if config.typo_correction {
            "enabled"
        } else {
            "disabled"
        }
    );
    if !config.disabled_apps.is_empty() {
        println!("  - Disabled apps: {}", config.disabled_apps.join(", "));
    }
    if !config.font.is_empty() {
        println!("  - Font: {}", config.font);
    }

    // 4. Load English frequency dictionary (50,000 words embedded)
    let start_time = std::time::Instant::now();
    let embedded_freq = include_str!("../data/en_50k.txt");
    let mut dict = Dictionary::from_frequency_text(embedded_freq);

    // 4b. Load English contextual n-gram models (bigrams and trigrams embedded)
    let embedded_bigrams = include_str!("../data/bigrams.tsv");
    dict.load_bigrams_tsv(embedded_bigrams);
    let embedded_trigrams = include_str!("../data/trigrams.tsv");
    dict.load_trigrams_tsv(embedded_trigrams);
    // The embedded data splits contractions ("don" + "'t"); restore them
    dict.add_english_contractions();

    // 5. Load user custom words from ~/.config/typesuggest/words.txt if it exists
    let user_words_path = app_config_dir.join("words.txt");
    if user_words_path.exists()
        && let Ok(content) = std::fs::read_to_string(&user_words_path)
    {
        let mut user_word_count = 0;
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.len() >= 2
                && trimmed.len() <= 64
                && !trimmed.starts_with('#')
                && trimmed.chars().all(|c| c.is_alphabetic() || c == '\'')
            {
                dict.insert(&trimmed.to_lowercase(), 50_000_000); // High frequency for custom words
                user_word_count += 1;
            }
        }
        if user_word_count > 0 {
            dict.rebuild_caches();
            println!(
                "Loaded {} user custom words from {:?}",
                user_word_count, user_words_path
            );
        }
    }

    println!(
        "Dictionary and language model initialized in {:?}",
        start_time.elapsed()
    );

    // 6. Connect to Wayland
    let conn = Connection::connect_to_env().map_err(|e| {
        format!(
            "Failed to connect to Wayland display: {}. Is WAYLAND_DISPLAY set?",
            e
        )
    })?;
    let display = conn.display();
    let mut event_queue = conn.new_event_queue();
    let qh = event_queue.handle();

    let mut engine = Engine::new(dict)?;
    // Learned phrases are loaded by apply_config when learning is enabled (now or after a reload)
    engine.user_bigrams_path = Some(app_config_dir.join("user_bigrams.tsv"));
    engine.apply_config(config);
    engine.config_source = Some(config_source);

    let _registry = display.get_registry(&qh, ());

    // Initial roundtrip to discover globals
    event_queue.roundtrip(&mut engine)?;

    // Verify required Wayland globals
    let seat = engine
        .seat
        .clone()
        .ok_or("Compositor does not advertise wl_seat")?;
    let compositor = engine
        .compositor
        .clone()
        .ok_or("Compositor does not advertise wl_compositor")?;
    let im_manager = engine
        .im_manager
        .clone()
        .ok_or("Compositor does not advertise zwp_input_method_manager_v2")?;
    let vk_manager = engine
        .vk_manager
        .clone()
        .ok_or("Compositor does not advertise zwp_virtual_keyboard_manager_v1")?;
    let _shm = engine
        .shm
        .clone()
        .ok_or("Compositor does not advertise wl_shm")?;

    // 7. Bind InputMethod, VirtualKeyboard, and InputPopupSurface
    println!("Binding Wayland input method and virtual keyboard...");
    let im = im_manager.get_input_method(&seat, &qh, ());
    let vk = vk_manager.create_virtual_keyboard(&seat, &qh, ());
    let grab = im.grab_keyboard(&qh, ());

    let surface = compositor.create_surface(&qh, ());
    let popup = im.get_input_popup_surface(&surface, &qh, ());

    engine.im = Some(im);
    engine.vk = Some(vk);
    engine.grab = Some(grab);
    engine.popup_surface = Some(surface);
    engine.popup = Some(popup);

    // Second roundtrip to complete bindings
    event_queue.roundtrip(&mut engine)?;

    println!("typesuggest is running and listening for text input.");

    // 8. Main event loop: wait for Wayland events, or until a held key starts repeating in the
    // app (Engine::on_repeat_deadline)
    loop {
        event_queue.dispatch_pending(&mut engine)?;
        if engine.unavailable {
            // The unit's RestartPreventExitStatus keeps systemd from retrying in a loop
            std::process::exit(EXIT_IME_UNAVAILABLE);
        }
        event_queue.flush()?;

        let Some(guard) = event_queue.prepare_read() else {
            continue;
        };
        let timeout = engine.repeat_deadline.map(|deadline| {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            rustix::event::Timespec {
                tv_sec: left.as_secs() as i64,
                tv_nsec: i64::from(left.subsec_nanos()),
            }
        });
        let ready = {
            let fd = guard.connection_fd();
            let mut fds = [rustix::event::PollFd::new(
                &fd,
                rustix::event::PollFlags::IN,
            )];
            rustix::event::poll(&mut fds, timeout.as_ref())
        };
        match ready {
            Ok(0) => {
                drop(guard);
                engine.on_repeat_deadline();
            }
            Ok(_) => match guard.read() {
                Ok(_) => {}
                Err(wayland_client::backend::WaylandError::Io(e))
                    if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => return Err(e.into()),
            },
            Err(rustix::io::Errno::INTR) => {}
            Err(e) => return Err(e.into()),
        }
    }
}

/// $XDG_CONFIG_HOME, or ~/.config. Relative paths are ignored, as the XDG spec requires, so the
/// config never lands in whatever directory the program was started from.
fn dirs_config_dir() -> Option<PathBuf> {
    let absolute = |p: PathBuf| p.is_absolute().then_some(p);
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .and_then(absolute)
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .and_then(absolute)
                .map(|h| h.join(".config"))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ensure_systemd_service() {
        let unique_id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_dir = std::env::temp_dir().join(format!("typesuggest_unit_{}", unique_id));
        let service_file = ensure_systemd_service(&temp_dir).unwrap();
        assert!(service_file.exists());
        let content = std::fs::read_to_string(&service_file).unwrap();
        assert!(content.contains("WantedBy=graphical-session.target"));
        if !Path::new(PACKAGED_SERVICE_UNIT).exists() {
            // A manual install points the user unit at this very executable
            let exe = std::env::current_exe().unwrap().canonicalize().unwrap();
            assert!(content.contains(&format!("ExecStart={}", exe.display())));
        }
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_service_unit_for() {
        let unit = service_unit_for(Path::new("/home/me/.local/bin/typesuggest"));
        assert!(unit.contains("\nExecStart=/home/me/.local/bin/typesuggest\n"));
        assert!(unit.contains("RestartPreventExitStatus=3"));
        let spaced = service_unit_for(Path::new("/opt/my apps/typesuggest"));
        assert!(spaced.contains("ExecStart=\"/opt/my apps/typesuggest\""));
        assert!(DEFAULT_SERVICE_UNIT.contains("\nExecStart=/usr/bin/typesuggest\n"));
    }
}
