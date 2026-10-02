use crate::config::{BarPosition, Config, ConfigSource};
use crate::dict::{Dictionary, write_user_bigrams_file};
use crate::security::{
    get_hyprland_active_window, get_hyprland_max_monitor_extent, has_sensitive_descendant,
    is_sensitive_window,
};
use crate::shm::{draw_pixmap_to_surface, hide_surface};
use crate::state::{InputMode, KeyAction, StateMachine};
use crate::theme::{OmarchyTheme, Palette, Theme};
use crate::ui::Renderer;
use std::os::fd::AsFd;
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_keyboard, wl_pointer, wl_registry, wl_seat, wl_shm, wl_shm_pool,
    wl_surface,
};
use wayland_client::{Connection, Dispatch, QueueHandle, WEnum};
use wayland_protocols::wp::text_input::zv3::client::zwp_text_input_v3::{
    ContentHint, ContentPurpose,
};
use wayland_protocols_misc::zwp_input_method_v2::client::{
    zwp_input_method_keyboard_grab_v2, zwp_input_method_manager_v2, zwp_input_method_v2,
    zwp_input_popup_surface_v2,
};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::{
    zwp_virtual_keyboard_manager_v1, zwp_virtual_keyboard_v1,
};
use xkbcommon::xkb;

/// Size Hyprland reports in `text_input_rectangle` when the focused app has not sent
/// `zwp_text_input_v3.set_cursor_rectangle` since it was enabled. The popup is then parked
/// 500px below the window's top-left corner (middle-left of the screen) instead of at the caret.
const PLACEHOLDER_CARET_SIZE: (i32, i32) = (500, 500);

/// Placeholder rectangles tolerated per activation before assuming the app never reports
/// its caret and showing the popup at the compositor's fallback position anyway.
const MAX_PLACEHOLDER_DEFERRALS: u8 = 3;

/// Logical height assumed for the tallest monitor when Hyprland cannot be asked
const FALLBACK_MONITOR_EXTENT: u32 = 2160;

/// Tallest buffer to attach: 16384 px is the smallest texture limit common GPUs guarantee
const MAX_BUFFER_HEIGHT: u32 = 16384;

/// Upper bound on key repeats replayed for one held key
const MAX_REPLAYED_REPEATS: u64 = 4096;

/// A forwarded key that may still be held down, so the app may be repeating it
#[derive(Debug, Clone, Copy)]
pub struct HeldKey {
    key: u32,
    /// Compositor timestamp (ms) of the press
    pressed_at: u32,
    keysym: u32,
    ch: Option<char>,
    ctrl: bool,
}

/// Repeats an app generates for a key held `held_ms` with Wayland key repeat at `rate` keys
/// per second after `delay` ms: the first repeat at `delay`, then one every 1000/rate ms
pub fn repeats_while_held(rate: i32, delay: i32, held_ms: u32) -> u64 {
    let (Ok(rate), Ok(delay)) = (u64::try_from(rate), u64::try_from(delay)) else {
        return 0;
    };
    let held_ms = u64::from(held_ms);
    if rate == 0 || held_ms < delay {
        return 0;
    }
    1 + (held_ms - delay) * rate / 1000
}

/// Modifier keys, which apps never repeat
fn is_modifier_keysym(keysym: u32) -> bool {
    (0xffe1..=0xffee).contains(&keysym) || (0xfe01..=0xfe0f).contains(&keysym) || keysym == 0xff7e
}

pub struct Engine {
    pub shm: Option<wl_shm::WlShm>,
    pub compositor: Option<wl_compositor::WlCompositor>,
    pub seat: Option<wl_seat::WlSeat>,
    pub im_manager: Option<zwp_input_method_manager_v2::ZwpInputMethodManagerV2>,
    pub vk_manager: Option<zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1>,

    pub im: Option<zwp_input_method_v2::ZwpInputMethodV2>,
    pub vk: Option<zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1>,
    pub grab: Option<zwp_input_method_keyboard_grab_v2::ZwpInputMethodKeyboardGrabV2>,
    pub popup_surface: Option<wl_surface::WlSurface>,
    pub popup: Option<zwp_input_popup_surface_v2::ZwpInputPopupSurfaceV2>,

    pub xkb_ctx: xkb::Context,
    pub xkb_state: Option<xkb::State>,

    pub dict: Dictionary,
    pub state_machine: StateMachine,
    pub renderer: Renderer,
    pub user_bigrams_path: Option<std::path::PathBuf>,
    /// The learned phrases file has been read into the dictionary (done once learning is on)
    pub user_bigrams_loaded: bool,

    /// Settings currently in effect
    pub config: Config,
    /// config.toml with the command-line overrides, checked for edits on each activation
    pub config_source: Option<ConfigSource>,
    pub omarchy_theme: OmarchyTheme,
    /// The focused window's class is in `disabled_apps`: keys pass straight through
    pub app_disabled: bool,

    pub active: bool,
    pub active_serial: u32,
    pub keymap_sent_to_vk: bool,
    pub swallowed_keys: std::collections::HashSet<u32>,
    pub is_sensitive: bool,
    pub is_sensitive_wayland: bool,
    pub terminal_sudo_pending: bool,
    pub terminal_sudo_started: Option<std::time::Instant>,
    pub active_window_pid: Option<u32>,
    pub last_window_check: std::time::Instant,
    pub is_popup_visible: bool,
    pub cursor_rect: Option<(i32, i32, i32, i32)>,
    /// Popup was hidden because the compositor has no caret rectangle yet; redraw on next done
    pub awaiting_caret_rect: bool,
    pub placeholder_deferrals: u8,
    /// Candidates and selection currently on screen
    pub last_drawn: Option<(Vec<String>, Option<usize>)>,
    /// Last surrounding text (and byte cursor) the app reported; None for apps that never do
    pub surrounding: Option<(String, usize)>,
    /// Content type and surrounding text of the update in progress; applied together on `done`,
    /// content type first, so a password field's text is never looked at
    pub pending_secret: Option<bool>,
    pub pending_surrounding: Option<(String, u32, u32)>,
    /// Another input method holds the seat; the main loop exits
    pub unavailable: bool,
    /// Logical height of the tallest monitor, for `bar_position = "above"`
    pub monitor_extent: u32,
    /// Seat pointer, used to move the bar out of the way when it is shown above the caret
    pub pointer: Option<wl_pointer::WlPointer>,
    /// Key repeat the compositor tells apps to apply (keys per second, ms before repeating)
    pub repeat_rate: i32,
    pub repeat_delay: i32,
    /// The last forwarded key while it is held down
    pub held_key: Option<HeldKey>,
    /// When the held key starts repeating in the app; the main loop calls on_repeat_deadline
    pub repeat_deadline: Option<std::time::Instant>,
    /// Background writer for the learned phrases, so the disk sync never delays typing
    learned_saver: Option<std::sync::mpsc::Sender<(std::path::PathBuf, String)>>,
}

impl Engine {
    pub fn new(dict: Dictionary) -> Result<Self, Box<dyn std::error::Error>> {
        let xkb_ctx = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let renderer = Renderer::new().map_err(|e| format!("Renderer init failed: {}", e))?;

        Ok(Self {
            shm: None,
            compositor: None,
            seat: None,
            im_manager: None,
            vk_manager: None,
            im: None,
            vk: None,
            grab: None,
            popup_surface: None,
            popup: None,
            xkb_ctx,
            xkb_state: None,
            dict,
            state_machine: StateMachine::new(3),
            renderer,
            user_bigrams_path: None,
            user_bigrams_loaded: false,
            config: Config::default(),
            config_source: None,
            omarchy_theme: OmarchyTheme::new(),
            app_disabled: false,
            active: false,
            active_serial: 0,
            keymap_sent_to_vk: false,
            swallowed_keys: std::collections::HashSet::new(),
            is_sensitive: false,
            is_sensitive_wayland: false,
            terminal_sudo_pending: false,
            terminal_sudo_started: None,
            active_window_pid: None,
            last_window_check: std::time::Instant::now()
                .checked_sub(std::time::Duration::from_secs(10))
                .unwrap_or_else(std::time::Instant::now),
            is_popup_visible: false,
            cursor_rect: None,
            awaiting_caret_rect: false,
            placeholder_deferrals: 0,
            last_drawn: None,
            surrounding: None,
            pending_secret: None,
            pending_surrounding: None,
            unavailable: false,
            learned_saver: None,
            monitor_extent: FALLBACK_MONITOR_EXTENT,
            pointer: None,
            repeat_rate: 25,
            repeat_delay: 600,
            held_key: None,
            repeat_deadline: None,
        })
    }

    /// Put a (re)loaded config into effect without disturbing the current input state
    pub fn apply_config(&mut self, config: Config) {
        self.state_machine.apply_config(&config);
        self.renderer.bar_scale = config.bar_scale;
        self.dict.set_typo_correction(config.typo_correction);

        // Read the learned phrases before learning starts, or the next save would overwrite
        // the file with only what was learned since
        if config.learn
            && !self.user_bigrams_loaded
            && let Some(path) = &self.user_bigrams_path
        {
            self.dict.load_user_bigrams_file(path);
            self.user_bigrams_loaded = true;
        }
        self.dict.set_learning(config.learn);

        if config.font != self.config.font
            && let Err(e) = self.renderer.set_font(&config.font)
        {
            eprintln!("[typesuggest] Keeping the current font: {}", e);
        }

        self.config = config;
        self.refresh_palette();
        self.last_drawn = None;
    }

    /// Pick up edits to config.toml and Omarchy theme switches. Called on activation only, so
    /// the key path never touches the disk; unchanged files cost a stat each.
    pub fn reload_settings(&mut self) {
        match self
            .config_source
            .as_mut()
            .and_then(ConfigSource::reload_if_changed)
        {
            Some(config) => {
                println!("[typesuggest] Configuration reloaded");
                self.apply_config(config);
            }
            None => self.refresh_palette(),
        }
    }

    /// Rebuild the bar colors from the configured theme and overrides. Omarchy's colors.toml is
    /// only re-read when it changed on disk.
    fn refresh_palette(&mut self) {
        let base = match self.config.theme {
            Theme::Omarchy => self.omarchy_theme.refresh().unwrap_or(Palette::BUILTIN),
            Theme::Default => Palette::BUILTIN,
        };
        let palette = self.config.colors.apply(base);
        if self.renderer.palette != palette {
            self.renderer.palette = palette;
            self.last_drawn = None;
        }
    }

    pub fn set_app_disabled(&mut self, disabled: bool) {
        if self.app_disabled != disabled {
            self.app_disabled = disabled;
            if disabled {
                self.hide();
                self.state_machine.reset();
                self.state_machine.invalidate_suggestions();
                self.surrounding = None;
            }
        }
    }

    pub fn refresh_sensitivity(&mut self) -> bool {
        // 1. Wayland ContentType (GUI passwords, PINs, hidden text)
        if self.is_sensitive_wayland {
            self.set_sensitive(true);
            return true;
        }

        // 2. Terminal sudo / auth pending state
        if self.terminal_sudo_pending {
            if let Some(started) = self.terminal_sudo_started
                && started.elapsed() > std::time::Duration::from_secs(60)
            {
                self.terminal_sudo_pending = false;
                self.terminal_sudo_started = None;
            } else {
                self.set_sensitive(true);
                return true;
            }
        }

        // 3. Query Hyprland active window & inspect process tree
        // Rate-limit IPC checks to avoid excessive socket traffic (every 40ms)
        let now = std::time::Instant::now();
        if now.duration_since(self.last_window_check) >= std::time::Duration::from_millis(40) {
            self.last_window_check = now;
            if let Some(win) = get_hyprland_active_window() {
                self.active_window_pid = Some(win.pid);
                // Focus can move between windows without a new activation
                self.set_app_disabled(self.config.is_disabled_app(&win.class));

                // Layer 3: Title & Class check
                if is_sensitive_window(&win.title, &win.class) {
                    self.set_sensitive(true);
                    return true;
                }

                // Layer 1: Descendant process tree check
                if has_sensitive_descendant(win.pid) {
                    self.set_sensitive(true);
                    return true;
                }
            } else if let Some(pid) = self.active_window_pid
                && has_sensitive_descendant(pid)
            {
                self.set_sensitive(true);
                return true;
            }
        } else if self.is_sensitive {
            return true;
        }

        self.set_sensitive(false);
        false
    }

    pub fn set_sensitive(&mut self, sensitive: bool) {
        if self.is_sensitive != sensitive {
            self.is_sensitive = sensitive;
            if sensitive {
                self.hide();
                self.state_machine.reset();
                self.state_machine.invalidate_suggestions();
                self.surrounding = None;
                self.swallowed_keys.clear();
            }
        }
    }

    pub fn render_and_show(
        &mut self,
        qh: &QueueHandle<Self>,
        candidates: &[String],
        selected_index: Option<usize>,
    ) {
        // GUI apps echo each keystroke back as surrounding text; the compositor moves the bar
        // with the caret on its own, so an identical redraw would only cost time.
        if self.is_popup_visible
            && self
                .last_drawn
                .as_ref()
                .is_some_and(|(c, s)| c == candidates && *s == selected_index)
        {
            return;
        }

        if let (Some(surface), Some(shm)) = (&self.popup_surface, &self.shm)
            && let Some(pixmap) = self.renderer.render_bar(candidates, selected_index)
        {
            let buffer_height = match self.config.bar_position {
                BarPosition::Below => pixmap.height(),
                // Hyprland places a popup that does not fit below the caret above it instead. A
                // transparent surface taller than any monitor triggers that every time, and the
                // bar drawn at its bottom edge then sits right above the text.
                BarPosition::Above => (self.monitor_extent * 2 + pixmap.height())
                    .next_multiple_of(2)
                    .min(MAX_BUFFER_HEIGHT),
            };
            if let Err(e) = draw_pixmap_to_surface(surface, shm, qh, &pixmap, buffer_height) {
                eprintln!("Failed to draw pixmap: {}", e);
            } else {
                self.is_popup_visible = true;
                self.last_drawn = Some((candidates.to_vec(), selected_index));
            }
        }
    }

    pub fn hide(&mut self) {
        if self.is_popup_visible
            && let Some(surface) = &self.popup_surface
        {
            hide_surface(surface);
            self.is_popup_visible = false;
        }
    }

    /// Re-show the suggestions the state machine currently holds, if any
    pub fn redraw_current(&mut self, qh: &QueueHandle<Self>) {
        let candidates = match &self.state_machine.mode {
            InputMode::Idle => return,
            InputMode::Suggesting { candidates, .. } => candidates.clone(),
            InputMode::Navigating { candidates, .. } => candidates.clone(),
        };
        let selected = self.state_machine.mode.selected_index();
        self.render_and_show(qh, &candidates, selected);
    }

    /// Act on the text around the caret that the app reported
    fn apply_surrounding_text(
        &mut self,
        qh: &QueueHandle<Self>,
        text: String,
        cursor: u32,
        anchor: u32,
    ) {
        // Never keep the contents of password / sensitive fields (or disabled apps) around
        let tracking = self.active && !self.is_sensitive && !self.app_disabled;
        if !tracking {
            self.surrounding = None;
            return;
        }

        let action = self.state_machine.handle_surrounding_text(
            &text,
            cursor as usize,
            anchor as usize,
            &self.dict,
        );
        self.surrounding = Some((text, cursor as usize));
        match action {
            KeyAction::ShowSuggestions { candidates, .. } => {
                let selected = self.state_machine.mode.selected_index();
                self.render_and_show(qh, &candidates, selected);
            }
            KeyAction::HideSuggestions => {
                self.hide();
            }
            KeyAction::UpdateSelection { index } => {
                if let InputMode::Navigating { candidates, .. } = &self.state_machine.mode {
                    let c = candidates.clone();
                    self.render_and_show(qh, &c, Some(index));
                }
            }
            _ => {
                // A hidden bar must not keep a selection that Enter could still commit
                self.state_machine.mode = InputMode::Idle;
                self.hide();
            }
        }
    }

    /// The held key started repeating in the app: the shown word is going stale, so hide the
    /// bar until the key is released
    pub fn on_repeat_deadline(&mut self) {
        self.repeat_deadline = None;
        if self.held_key.is_some() {
            self.state_machine.mode = InputMode::Idle;
            self.hide();
        }
    }

    /// Apps repeat a held key themselves, so typesuggest sees a single press. Replay the repeats
    /// the app produced between the press and `until` (compositor timestamps), so the
    /// remembered line matches what the app did. Apps that report surrounding text resync on
    /// their own and are left to that.
    fn apply_key_repeats(&mut self, held: HeldKey, until: u32, qh: &QueueHandle<Self>) {
        let held_ms = until.wrapping_sub(held.pressed_at);
        let repeats = repeats_while_held(self.repeat_rate, self.repeat_delay, held_ms)
            .min(MAX_REPLAYED_REPEATS);
        if repeats == 0
            || !self.active
            || self.is_sensitive
            || self.app_disabled
            || self.surrounding.is_some()
        {
            return;
        }
        for _ in 0..repeats {
            let _ =
                self.state_machine
                    .handle_key_press(held.keysym, held.ch, held.ctrl, &self.dict);
        }
        match self.state_machine.mode {
            InputMode::Idle => self.hide(),
            _ => self.redraw_current(qh),
        }
    }

    /// Drop everything typed in the field that just lost focus
    fn forget_typed_text(&mut self) {
        self.state_machine.reset();
        self.state_machine.invalidate_suggestions();
        self.surrounding = None;
        self.pending_surrounding = None;
        self.last_drawn = None;
    }

    /// Write the learned phrases on a background thread; only the newest snapshot is written
    fn save_learned_phrases(&mut self) {
        let Some(path) = self.user_bigrams_path.clone() else {
            return;
        };
        let content = self.dict.user_bigrams_tsv();
        let saver = self.learned_saver.get_or_insert_with(|| {
            let (tx, rx) = std::sync::mpsc::channel::<(std::path::PathBuf, String)>();
            std::thread::spawn(move || {
                while let Ok(mut job) = rx.recv() {
                    while let Ok(newer) = rx.try_recv() {
                        job = newer;
                    }
                    write_user_bigrams_file(&job.0, &job.1);
                }
            });
            tx
        });
        let _ = saver.send((path, content));
    }
}

/// Password, PIN, hidden-text and sensitive-data fields. Hint bits this build does not know
/// (newer protocol versions) must not hide the ones it does.
fn is_secret_content(hint: WEnum<ContentHint>, purpose: WEnum<ContentPurpose>) -> bool {
    let secret_hints = (ContentHint::HiddenText | ContentHint::SensitiveData).bits();
    let hint_bits = match hint {
        WEnum::Value(h) => h.bits(),
        WEnum::Unknown(raw) => raw,
    };
    matches!(
        purpose,
        WEnum::Value(ContentPurpose::Password | ContentPurpose::Pin)
    ) || hint_bits & secret_hints != 0
}

// Registry Dispatch
impl Dispatch<wl_registry::WlRegistry, ()> for Engine {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            match interface.as_str() {
                "wl_shm" => {
                    state.shm = Some(registry.bind(name, version.min(1), qh, ()));
                }
                "wl_compositor" => {
                    state.compositor = Some(registry.bind(name, version.min(4), qh, ()));
                }
                "wl_seat" => {
                    if state.seat.is_none() {
                        state.seat = Some(registry.bind(name, version.min(2), qh, ()));
                    }
                }
                "zwp_input_method_manager_v2" => {
                    state.im_manager = Some(registry.bind(name, version.min(1), qh, ()));
                }
                "zwp_virtual_keyboard_manager_v1" => {
                    state.vk_manager = Some(registry.bind(name, version.min(1), qh, ()));
                }
                _ => {}
            }
        }
    }
}

// Compositor & Surface Dispatches
impl Dispatch<wl_compositor::WlCompositor, ()> for Engine {
    fn event(
        _: &mut Self,
        _: &wl_compositor::WlCompositor,
        _: wl_compositor::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_surface::WlSurface, ()> for Engine {
    fn event(
        _: &mut Self,
        _: &wl_surface::WlSurface,
        _: wl_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

// Seat Dispatch
impl Dispatch<wl_seat::WlSeat, ()> for Engine {
    fn event(
        state: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(caps),
        } = event
            && caps.contains(wl_seat::Capability::Pointer)
            && state.pointer.is_none()
        {
            state.pointer = Some(seat.get_pointer(qh, ()));
        }
    }
}

// Pointer Dispatch: events only arrive while the pointer is over our own popup
impl Dispatch<wl_pointer::WlPointer, ()> for Engine {
    fn event(
        state: &mut Self,
        _: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // Above the caret, the popup's transparent part covers the text above it and would take
        // the mouse; get out of the way as soon as the mouse moves there
        if let wl_pointer::Event::Enter { surface, .. } = event
            && state.config.bar_position == BarPosition::Above
            && state.popup_surface.as_ref() == Some(&surface)
        {
            state.state_machine.mode = InputMode::Idle;
            state.hide();
        }
    }
}

// SHM Dispatches
impl Dispatch<wl_shm::WlShm, ()> for Engine {
    fn event(
        _: &mut Self,
        _: &wl_shm::WlShm,
        _: wl_shm::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_shm_pool::WlShmPool, ()> for Engine {
    fn event(
        _: &mut Self,
        _: &wl_shm_pool::WlShmPool,
        _: wl_shm_pool::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_buffer::WlBuffer, ()> for Engine {
    fn event(
        _: &mut Self,
        buffer: &wl_buffer::WlBuffer,
        event: wl_buffer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_buffer::Event::Release = event {
            buffer.destroy();
        }
    }
}

// Virtual Keyboard Dispatches
impl Dispatch<zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1, ()> for Engine {
    fn event(
        _: &mut Self,
        _: &zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1,
        _: zwp_virtual_keyboard_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1, ()> for Engine {
    fn event(
        _: &mut Self,
        _: &zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1,
        _: zwp_virtual_keyboard_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

// Input Method Manager & Popup Surface Dispatches
impl Dispatch<zwp_input_method_manager_v2::ZwpInputMethodManagerV2, ()> for Engine {
    fn event(
        _: &mut Self,
        _: &zwp_input_method_manager_v2::ZwpInputMethodManagerV2,
        _: zwp_input_method_manager_v2::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<zwp_input_popup_surface_v2::ZwpInputPopupSurfaceV2, ()> for Engine {
    fn event(
        state: &mut Self,
        _: &zwp_input_popup_surface_v2::ZwpInputPopupSurfaceV2,
        event: zwp_input_popup_surface_v2::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwp_input_popup_surface_v2::Event::TextInputRectangle {
            x,
            y,
            width,
            height,
        } = event
        {
            state.cursor_rect = Some((x, y, width, height));

            // Hyprland's no-caret placeholder: hide instead of showing the bar in the middle of
            // the window, and redraw once the app commits its caret rectangle (next done).
            if (width, height) == PLACEHOLDER_CARET_SIZE
                && state.placeholder_deferrals < MAX_PLACEHOLDER_DEFERRALS
            {
                state.placeholder_deferrals += 1;
                state.awaiting_caret_rect = true;
                state.hide();
            } else {
                state.awaiting_caret_rect = false;
            }
        }
    }
}

// Input Method V2 Dispatch
impl Dispatch<zwp_input_method_v2::ZwpInputMethodV2, ()> for Engine {
    fn event(
        state: &mut Self,
        _: &zwp_input_method_v2::ZwpInputMethodV2,
        event: zwp_input_method_v2::Event,
        _: &(),
        conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            zwp_input_method_v2::Event::ContentType { hint, purpose } => {
                // Applied on done, before the surrounding text of the same update
                state.pending_secret = Some(is_secret_content(hint, purpose));
            }
            zwp_input_method_v2::Event::Activate => {
                state.active = true;
                state.cursor_rect = None;
                state.surrounding = None;
                state.pending_secret = None;
                state.pending_surrounding = None;
                state.held_key = None;
                state.repeat_deadline = None;
                state.awaiting_caret_rect = false;
                state.placeholder_deferrals = 0;
                state.state_machine.reset();
                state.swallowed_keys.clear();
                state.hide();
                // Config edits and theme switches apply from the next focused text field
                state.reload_settings();
                if state.config.bar_position == BarPosition::Above {
                    state.monitor_extent =
                        get_hyprland_max_monitor_extent().unwrap_or(FALLBACK_MONITOR_EXTENT);
                }
                let mut disabled = false;
                if let Some(win) = get_hyprland_active_window() {
                    state.active_window_pid = Some(win.pid);
                    disabled = state.config.is_disabled_app(&win.class);
                }
                state.set_app_disabled(disabled);
                state.refresh_sensitivity();
            }
            zwp_input_method_v2::Event::Deactivate => {
                state.active = false;
                state.held_key = None;
                state.repeat_deadline = None;
                state.cursor_rect = None;
                state.pending_secret = None;
                state.awaiting_caret_rect = false;
                state.is_sensitive_wayland = false;
                state.terminal_sudo_pending = false;
                state.terminal_sudo_started = None;
                state.set_sensitive(false);
                state.forget_typed_text();
                state.swallowed_keys.clear();
                state.hide();
            }

            zwp_input_method_v2::Event::SurroundingText {
                text,
                cursor,
                anchor,
            } => {
                // Applied on done, after any content type of the same update
                state.pending_surrounding = Some((text, cursor, anchor));
            }
            zwp_input_method_v2::Event::Done => {
                state.active_serial = state.active_serial.wrapping_add(1);

                // The update is complete: whether the field is secret first, then its text
                if let Some(secret) = state.pending_secret.take() {
                    state.is_sensitive_wayland = secret;
                    state.refresh_sensitivity();
                }
                if let Some((text, cursor, anchor)) = state.pending_surrounding.take() {
                    state.apply_surrounding_text(qh, text, cursor, anchor);
                }

                // Disabled apps are left alone entirely, as if no input method were running
                if state.active && !state.app_disabled {
                    // Acknowledge every done. Hyprland only sends zwp_text_input_v3.done to the app
                    // when the IME commits, and Chromium/Electron hold back set_cursor_rectangle and
                    // set_surrounding_text until a done arrives whose serial matches their commit
                    // count. Without this ack their caret updates lag a keystroke behind, and the
                    // popup gets placed against a stale or missing caret rectangle.
                    if let Some(im) = &state.im {
                        im.commit(state.active_serial);
                    }
                    let _ = conn.flush();

                    if state.awaiting_caret_rect && !state.is_popup_visible && !state.is_sensitive {
                        state.redraw_current(qh);
                    }
                }
            }
            zwp_input_method_v2::Event::Unavailable => {
                eprintln!(
                    "[typesuggest] Error: Input method unavailable. Ensure another IME (such as Fcitx5) is not active on this seat."
                );
                state.unavailable = true;
            }
            _ => {}
        }
    }
}

// Keyboard Grab Dispatch
impl Dispatch<zwp_input_method_keyboard_grab_v2::ZwpInputMethodKeyboardGrabV2, ()> for Engine {
    fn event(
        state: &mut Self,
        _: &zwp_input_method_keyboard_grab_v2::ZwpInputMethodKeyboardGrabV2,
        event: zwp_input_method_keyboard_grab_v2::Event,
        _: &(),
        conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            zwp_input_method_keyboard_grab_v2::Event::Keymap { format, fd, size } => {
                let vkb_fd = if state.vk.is_some() {
                    fd.as_fd().try_clone_to_owned().ok()
                } else {
                    None
                };

                if matches!(format, WEnum::Value(wl_keyboard::KeymapFormat::XkbV1)) {
                    match unsafe {
                        xkb::Keymap::new_from_fd(
                            &state.xkb_ctx,
                            fd,
                            size as usize,
                            xkb::KEYMAP_FORMAT_TEXT_V1,
                            xkb::KEYMAP_COMPILE_NO_FLAGS,
                        )
                    } {
                        Ok(Some(keymap)) => {
                            state.xkb_state = Some(xkb::State::new(&keymap));
                        }
                        _ => {
                            eprintln!("[typesuggest] Failed to parse compositor XKB keymap");
                        }
                    }
                }

                if let (Some(vk), Some(vkb_fd)) = (&state.vk, vkb_fd) {
                    vk.keymap(1, vkb_fd.as_fd(), size);
                    state.keymap_sent_to_vk = true;
                }
            }

            zwp_input_method_keyboard_grab_v2::Event::Modifiers {
                serial: _,
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
            } => {
                if let Some(xkb_state) = &mut state.xkb_state {
                    xkb_state.update_mask(mods_depressed, mods_latched, mods_locked, 0, 0, group);
                }

                if let Some(vk) = &state.vk
                    && state.keymap_sent_to_vk
                {
                    vk.modifiers(mods_depressed, mods_latched, mods_locked, group);
                }
            }

            zwp_input_method_keyboard_grab_v2::Event::Key {
                serial: _,
                time,
                key,
                state: key_state,
            } => {
                let is_pressed = matches!(key_state, WEnum::Value(wl_keyboard::KeyState::Pressed));

                if is_pressed {
                    // Forward the keystroke exactly once, from whichever step decides it
                    let mut forwarded = false;
                    macro_rules! forward {
                        () => {
                            if !forwarded {
                                // Not read again on paths that return right after forwarding
                                #[allow(unused_assignments)]
                                {
                                    forwarded = true;
                                }
                                if let Some(vk) = &state.vk {
                                    vk.key(time, key, 1);
                                }
                            }
                        };
                    }

                    // A new key ends the previous key's repeat in the app; catch up on what it did
                    if let Some(held) = state.held_key.take() {
                        state.repeat_deadline = None;
                        state.apply_key_repeats(held, time, qh);
                    }

                    // The grab receives every key, even when the focused app has no enabled
                    // text input (XWayland, games, Qt with QT_IM_MODULE=fcitx). Our commits are
                    // dropped there, so only forward: acting on suggestions would swallow
                    // Up/Enter/Space/Tab and inject Backspaces into the app.
                    if !state.active {
                        forward!();
                        return;
                    }

                    let mut keysym_raw = 0u32;
                    let mut char_opt = None;
                    let mut ctrl_active = false;
                    let mut alt_or_super = false;

                    if let Some(xkb_state) = &state.xkb_state {
                        let keycode = xkb::Keycode::new(key + 8);
                        keysym_raw = xkb_state.key_get_one_sym(keycode).raw();
                        let utf8 = xkb_state.key_get_utf8(keycode);
                        if !utf8.is_empty() {
                            char_opt = utf8.chars().next();
                        }
                        let active = |m| xkb_state.mod_name_is_active(m, xkb::STATE_MODS_EFFECTIVE);
                        ctrl_active = active(xkb::MOD_NAME_CTRL);
                        alt_or_super = active(xkb::MOD_NAME_ALT) || active(xkb::MOD_NAME_LOGO);
                    }

                    // While the bar is held back waiting for a caret rectangle the user cannot see
                    // it, so Up/Enter must not navigate or commit a hidden candidate.
                    if state.awaiting_caret_rect && !state.is_popup_visible {
                        state.state_machine.mode = InputMode::Idle;
                    }

                    // Keys that cannot be swallowed go to the app before any slower work (window
                    // and process checks, dictionary lookups, drawing), so typing never waits on it
                    if !state
                        .state_machine
                        .may_swallow(keysym_raw, char_opt, ctrl_active)
                    {
                        forward!();
                        let _ = conn.flush();
                    }

                    // 1. Refresh sensitivity from Wayland ContentType, Hyprland window, and /proc
                    // (the rate-limited window query also notices focus moving to a disabled app)
                    state.refresh_sensitivity();

                    // Apps in disabled_apps get every key untouched, just like the inactive path
                    if state.app_disabled {
                        forward!();
                        return;
                    }

                    // 2. If sensitive (password/PIN/sudo prompt), forward raw key and do not touch buffer or suggestions
                    if state.is_sensitive {
                        state.hide();
                        if keysym_raw == 0xff0d || keysym_raw == 0xff8d {
                            // User pressed Enter (submitting password). Clear pending flag and trigger re-check
                            state.terminal_sudo_pending = false;
                            state.terminal_sudo_started = None;
                            state.last_window_check = std::time::Instant::now()
                                .checked_sub(std::time::Duration::from_secs(1))
                                .unwrap_or_else(std::time::Instant::now);
                        } else if (ctrl_active
                            && (keysym_raw == 0x0063
                                || keysym_raw == 0x0043
                                || keysym_raw == 0x0064
                                || keysym_raw == 0x0044))
                            || keysym_raw == 0xff1b
                        {
                            // Ctrl+C, Ctrl+D, or Escape (user cancelled prompt). A password field
                            // stays sensitive; only the terminal prompt tracking ends.
                            state.terminal_sudo_pending = false;
                            state.terminal_sudo_started = None;
                            state.set_sensitive(state.is_sensitive_wayland);
                        }

                        forward!();
                        return;
                    }

                    // Alt / Super shortcuts (word jumps, window manager keys) move the caret in
                    // ways the buffer cannot follow: forget the word and let the key through
                    if alt_or_super {
                        state.state_machine.reset();
                        state.hide();
                        forward!();
                        return;
                    }

                    let action = state.state_machine.handle_key_press(
                        keysym_raw,
                        char_opt,
                        ctrl_active,
                        &state.dict,
                    );

                    // If Enter was pressed and not committing a candidate, unconditionally dismiss suggestions
                    if (keysym_raw == 0xff0d || keysym_raw == 0xff8d)
                        && !matches!(action, KeyAction::CommitCandidate { .. })
                    {
                        state.hide();
                    }

                    match action {
                        KeyAction::PassThrough => {
                            forward!();
                        }

                        // Typed-command detection is for terminals; apps that report surrounding
                        // text are GUI fields, where "su..." or "pass..." is ordinary text
                        KeyAction::SensitiveCommandSubmitted if state.surrounding.is_none() => {
                            state.hide();
                            state.terminal_sudo_pending = true;
                            state.terminal_sudo_started = Some(std::time::Instant::now());
                            state.set_sensitive(true);
                            forward!();
                        }

                        KeyAction::SensitiveCommandSubmitted | KeyAction::HideSuggestions => {
                            forward!();
                            state.hide();
                        }

                        KeyAction::ShowSuggestions { candidates, .. } => {
                            forward!();
                            // Deliver the keystroke before spending time drawing the bar
                            let _ = conn.flush();
                            let selected = state.state_machine.mode.selected_index();
                            state.render_and_show(qh, &candidates, selected);
                        }

                        KeyAction::CancelNavigation => {
                            // SWALLOW Down / Escape key when exiting navigation!
                            state.swallowed_keys.insert(key);
                            state.hide();
                        }

                        KeyAction::UpdateSelection { index } => {
                            // SWALLOW key (Up / Left / Right)
                            state.swallowed_keys.insert(key);
                            if let InputMode::Navigating { candidates, .. } =
                                &state.state_machine.mode
                            {
                                let c = candidates.clone();
                                state.render_and_show(qh, &c, Some(index));
                            }
                        }

                        KeyAction::CommitCandidate {
                            deleted_before,
                            deleted_after,
                            replacement,
                            prev_word,
                            chosen_word,
                        } => {
                            // SWALLOW Enter / Space / Tab key (zero accidental chat sends or extra spaces!)
                            state.swallowed_keys.insert(key);

                            // Committed text is typed into the app, terminals included: never
                            // let a control character through (e.g. a newline would run a command)
                            if replacement.chars().any(char::is_control) {
                                state.hide();
                                return;
                            }

                            // 1. Remove the partial word. Apps that report surrounding text (GTK,
                            // Qt, Chromium) get delete_surrounding_text, applied atomically with
                            // the commit. Simulated Backspaces race the commit there: GTK and Qt
                            // queue key events but insert committed text at once, so the new word
                            // landed first and lost its tail ("hel" -> "helhe"). Only use it when
                            // the app's last reported text matches what we are about to delete.
                            let app_text_matches =
                                state.surrounding.as_ref().is_some_and(|(text, cursor)| {
                                    text.get(..*cursor)
                                        .is_some_and(|before| before.ends_with(&deleted_before))
                                        && text
                                            .get(*cursor..)
                                            .is_some_and(|after| after.starts_with(&deleted_after))
                                });
                            if let Some(im) = &state.im
                                && app_text_matches
                            {
                                im.delete_surrounding_text(
                                    deleted_before.len() as u32,
                                    deleted_after.len() as u32,
                                );
                            } else if let Some(vk) = &state.vk {
                                // Terminals: Backspace (evdev 14) and Delete (evdev 111)
                                for _ in deleted_before.chars() {
                                    vk.key(time, 14, 1);
                                    vk.key(time, 14, 0);
                                }
                                for _ in deleted_after.chars() {
                                    vk.key(time, 111, 1);
                                    vk.key(time, 111, 0);
                                }
                            }

                            // 2. Commit replacement string via InputMethod
                            if let Some(im) = &state.im {
                                im.commit_string(replacement);
                                im.commit(state.active_serial);
                            }

                            let _ = conn.flush();
                            state.hide();

                            // 3. Learn the phrase (dictionary words only) and save it in the
                            // background, after the text is on its way
                            if state.dict.is_learning_enabled()
                                && let Some(pw) = &prev_word
                            {
                                state.dict.record_user_bigram(pw, &chosen_word);
                                state.state_machine.invalidate_suggestions();
                                state.save_learned_phrases();
                            }
                        }

                        KeyAction::Consume => {
                            state.swallowed_keys.insert(key);
                        }
                    }

                    // The app repeats a key that stays down; remember it to keep up with that
                    if forwarded && !is_modifier_keysym(keysym_raw) {
                        state.held_key = Some(HeldKey {
                            key,
                            pressed_at: time,
                            keysym: keysym_raw,
                            ch: char_opt,
                            ctrl: ctrl_active,
                        });
                        if state.repeat_rate > 0 {
                            state.repeat_deadline = Some(
                                std::time::Instant::now()
                                    + std::time::Duration::from_millis(
                                        state.repeat_delay.max(0) as u64
                                    ),
                            );
                        }
                    }
                } else {
                    // Key released
                    if state.swallowed_keys.remove(&key) {
                        // Swallow release of consumed keys
                    } else if let Some(vk) = &state.vk {
                        vk.key(time, key, 0);
                    }
                    if let Some(held) = state.held_key.filter(|h| h.key == key) {
                        state.held_key = None;
                        state.repeat_deadline = None;
                        state.apply_key_repeats(held, time, qh);
                    }
                }
            }

            zwp_input_method_keyboard_grab_v2::Event::RepeatInfo { rate, delay } => {
                state.repeat_rate = rate;
                state.repeat_delay = delay;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_repeats_while_held() {
        // Hyprland's defaults: 25 keys per second after 600 ms
        assert_eq!(repeats_while_held(25, 600, 599), 0);
        assert_eq!(repeats_while_held(25, 600, 600), 1);
        assert_eq!(repeats_while_held(25, 600, 639), 1);
        assert_eq!(repeats_while_held(25, 600, 640), 2);
        assert_eq!(repeats_while_held(25, 600, 2500), 48);
        // Repeat turned off, or nonsense values from the compositor
        assert_eq!(repeats_while_held(0, 600, 5000), 0);
        assert_eq!(repeats_while_held(-1, 600, 5000), 0);
        assert_eq!(repeats_while_held(25, -5, 5000), 0);
    }

    #[test]
    fn test_modifiers_are_not_repeated() {
        assert!(is_modifier_keysym(0xffe1)); // Shift_L
        assert!(is_modifier_keysym(0xffe3)); // Control_L
        assert!(is_modifier_keysym(0xfe03)); // ISO_Level3_Shift (AltGr)
        assert!(!is_modifier_keysym(0xff08)); // BackSpace
        assert!(!is_modifier_keysym(0x0061)); // a
    }
}
