use crate::config::{AcceptKeys, Config};
use crate::dict::{Context, Dictionary};

// Standard XKB keysym constants
const KEY_UP: u32 = 0xff52;
const KEY_DOWN: u32 = 0xff54;
const KEY_LEFT: u32 = 0xff51;
const KEY_RIGHT: u32 = 0xff53;
const KEY_HOME: u32 = 0xff50;
const KEY_END: u32 = 0xff57;
const KEY_RETURN: u32 = 0xff0d;
const KEY_KP_ENTER: u32 = 0xff8d;
const KEY_TAB: u32 = 0xff09;
const KEY_ESCAPE: u32 = 0xff1b;
const KEY_BACKSPACE: u32 = 0xff08;
const KEY_DELETE: u32 = 0xffff;
const KEY_SPACE: u32 = 0x0020;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputMode {
    Idle,
    Suggesting {
        prefix: String,
        suffix: String,
        candidates: Vec<String>,
    },
    Navigating {
        prefix: String,
        suffix: String,
        candidates: Vec<String>,
        selected_index: usize,
    },
}

impl InputMode {
    /// The candidate the bar highlights. The best suggestion counts as selected from the
    /// moment the bar appears, so taking it needs no separate keypress, and the two modes
    /// differ only in how keys act rather than in what is highlighted.
    pub fn selected_index(&self) -> Option<usize> {
        match self {
            InputMode::Idle => None,
            InputMode::Suggesting { candidates, .. } => (!candidates.is_empty()).then_some(0),
            InputMode::Navigating { selected_index, .. } => Some(*selected_index),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyAction {
    PassThrough,
    /// Consume key without forwarding
    Consume,
    /// Show or update suggestions bar
    ShowSuggestions {
        prefix: String,
        candidates: Vec<String>,
    },
    /// Dismiss suggestions bar
    HideSuggestions,
    /// Cancel navigation and swallow key (Down/Esc/Up while navigating)
    CancelNavigation,
    /// Update highlighted candidate index in suggestions bar
    UpdateSelection {
        index: usize,
    },
    /// Commit candidate: replace `deleted_before` (text just before the cursor) and `deleted_after`
    /// (text just after it) with `replacement`
    CommitCandidate {
        deleted_before: String,
        deleted_after: String,
        replacement: String,
        prev_word: Option<String>,
        chosen_word: String,
    },
    /// A command known to prompt for credentials/passwords was submitted (e.g. sudo, passwd)
    SensitiveCommandSubmitted,
}

pub fn is_word_char(c: char) -> bool {
    // ’ is the apostrophe apps with typographic quotes insert
    c.is_alphanumeric() || c == '\'' || c == '\u{2019}'
}

pub fn extract_word_prefix(before_cursor: &str) -> &str {
    let mut start_byte = before_cursor.len();
    for (idx, ch) in before_cursor.char_indices().rev() {
        if is_word_char(ch) {
            start_byte = idx;
        } else {
            break;
        }
    }
    &before_cursor[start_byte..]
}

pub fn extract_word_suffix(after_cursor: &str) -> &str {
    let mut end_byte = 0;
    for (idx, ch) in after_cursor.char_indices() {
        if is_word_char(ch) {
            end_byte = idx + ch.len_utf8();
        } else {
            break;
        }
    }
    &after_cursor[..end_byte]
}

/// Walk back from `end` over whitespace and punctuation to the word before it.
/// Returns None when a sentence boundary (`.`, `!`, `?` or a newline) sits in
/// between, since the word before a full stop says nothing about the one after it.
fn prev_word_span(before: &[char], end: usize) -> Option<(usize, usize)> {
    let mut word_end = end;
    while word_end > 0 && before[word_end - 1].is_whitespace() {
        word_end -= 1;
    }
    while word_end > 0 && !is_word_char(before[word_end - 1]) {
        let ch = before[word_end - 1];
        if ch == '.' || ch == '!' || ch == '?' || ch == '\n' {
            return None;
        }
        word_end -= 1;
    }
    let mut word_start = word_end;
    for (i, &c) in before[..word_end].iter().enumerate().rev() {
        if is_word_char(c) {
            word_start = i;
        } else {
            break;
        }
    }
    (word_start < word_end).then_some((word_start, word_end))
}

/// Whether a word can condition a completion: real letters only, so digits and
/// codes stay out of the n-gram tables' keys, and at least two characters.
fn clean_context_word(word: &str) -> Option<String> {
    let clean = word.trim().to_lowercase().replace('\u{2019}', "'");
    (clean.chars().count() >= 2 && clean.chars().all(|c| c.is_alphabetic() || c == '\''))
        .then_some(clean)
}

/// Where the identifier segment ending at `end` begins, for a word that starts at
/// `word_start`. Segments break where a capital letter follows a lower-case letter or a
/// digit ("myProg"), and where a run of capitals ends before a lower-case letter, so that
/// "HTTPServer" reads as "HTTP" plus "Server".
fn segment_start(chars: &[char], word_start: usize, end: usize) -> usize {
    // Walking back from the caret, the first boundary found is the one nearest it, which
    // is where the segment being typed begins
    for i in (word_start + 1..end).rev() {
        let c = chars[i];
        let prev = chars[i - 1];
        let capital_after_lower = c.is_uppercase() && (prev.is_lowercase() || prev.is_numeric());
        let end_of_capital_run = c.is_uppercase()
            && prev.is_uppercase()
            && chars.get(i + 1).is_some_and(|next| next.is_lowercase());
        if capital_after_lower || end_of_capital_run {
            return i;
        }
    }
    word_start
}

/// The two words preceding the word that starts at `word_start`, nearest first
fn context_before(before: &[char], word_start: usize) -> Context {
    let span = prev_word_span(before, word_start);
    let word_of = |(start, end): (usize, usize)| {
        clean_context_word(&before[start..end].iter().collect::<String>())
    };
    let prev = span.and_then(word_of);
    let prev_prev = span
        .and_then(|(start, _)| prev_word_span(before, start))
        .and_then(word_of);
    Context::new(prev, prev_prev)
}

/// The word preceding `before_word` (a slice that ends where a word starts)
pub fn extract_prev_word_from_slice(before_word: &str) -> Option<String> {
    extract_context_from_slice(before_word).prev
}

/// The two words preceding `before_word`, for the text an application reported
pub fn extract_context_from_slice(before_word: &str) -> Context {
    let chars: Vec<char> = before_word.chars().collect();
    context_before(&chars, chars.len())
}

/// Detect whether a command line invokes a sensitive command that prompts for credentials/passwords.
pub fn is_sensitive_command_line(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }
    // Check if line or piped/chained segment starts with a sensitive command
    // e.g. "sudo pacman -S foo", "sudo su", "su -", "passwd", "ssh user@host"
    for segment in trimmed.split(&[';', '|', '&'][..]) {
        let seg = segment.trim();
        let cmd = seg.split_whitespace().next().unwrap_or("");
        let cmd_name = cmd.rsplit('/').next().unwrap_or(cmd);
        if matches!(
            cmd_name,
            "sudo"
                | "su"
                | "doas"
                | "passwd"
                | "pkexec"
                | "ssh"
                | "login"
                | "cryptsetup"
                | "op"
                | "bw"
                | "vault"
                | "pass"
                | "openconnect"
                | "openvpn"
                | "keepassxc-cli"
        ) || cmd_name.starts_with("pinentry")
        {
            return true;
        }
    }
    false
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InputBuffer {
    pub chars: Vec<char>,
    pub cursor: usize, // cursor index between 0 and chars.len()
}

impl InputBuffer {
    pub fn new() -> Self {
        Self {
            chars: Vec::new(),
            cursor: 0,
        }
    }

    pub fn clear(&mut self) {
        self.chars.clear();
        self.cursor = 0;
    }

    pub fn insert_char(&mut self, c: char) {
        let cur = self.cursor.min(self.chars.len());
        self.chars.insert(cur, c);
        self.cursor = cur + 1;
        // Bounded capacity to prevent unbounded memory growth
        if self.chars.len() > 4096 {
            let drop_count = self.chars.len() - 4096;
            self.chars.drain(0..drop_count);
            self.cursor = self.cursor.saturating_sub(drop_count);
        }
    }

    pub fn backspace(&mut self) -> bool {
        if self.cursor > 0 && !self.chars.is_empty() {
            let idx = (self.cursor - 1).min(self.chars.len() - 1);
            self.chars.remove(idx);
            self.cursor = idx;
            true
        } else {
            false
        }
    }

    pub fn delete(&mut self) -> bool {
        if self.cursor < self.chars.len() {
            self.chars.remove(self.cursor);
            true
        } else {
            false
        }
    }

    pub fn delete_word_backwards(&mut self) {
        // 1. Delete trailing spaces before cursor
        while self.cursor > 0 && self.chars[self.cursor - 1].is_whitespace() {
            self.backspace();
        }
        // 2. Delete word characters before cursor
        while self.cursor > 0 && is_word_char(self.chars[self.cursor - 1]) {
            self.backspace();
        }
    }

    pub fn delete_to_start(&mut self) {
        let cur = self.cursor.min(self.chars.len());
        self.chars.drain(0..cur);
        self.cursor = 0;
    }

    pub fn delete_to_end(&mut self) {
        let cur = self.cursor.min(self.chars.len());
        self.chars.truncate(cur);
    }

    pub fn move_left(&mut self) -> bool {
        if self.cursor > 0 {
            self.cursor -= 1;
            true
        } else {
            false
        }
    }

    pub fn move_right(&mut self) -> bool {
        if self.cursor < self.chars.len() {
            self.cursor += 1;
            true
        } else {
            false
        }
    }

    pub fn move_home(&mut self) {
        self.cursor = 0;
    }

    pub fn move_end(&mut self) {
        self.cursor = self.chars.len();
    }

    /// Extract the current word prefix immediately preceding the cursor, and the
    /// two words before it that condition the completion
    pub fn current_word_context(&self) -> (String, Context) {
        let cur = self.cursor.min(self.chars.len());
        let before = &self.chars[..cur];

        // Find the start of the word being typed
        let mut cur_word_start = cur;
        for (i, &c) in before.iter().enumerate().rev() {
            if is_word_char(c) {
                cur_word_start = i;
            } else {
                break;
            }
        }

        // Inside an identifier only the segment being typed is completed, so "myProg"
        // offers "myProgram" instead of looking for a word starting with "myProg". The
        // context still comes from before the whole word, since "my" is part of this word
        // rather than a word of its own.
        let segment_begin = segment_start(before, cur_word_start, cur);
        let prefix: String = before[segment_begin..cur].iter().collect();

        (prefix.clone(), context_before(before, cur_word_start))
    }

    /// The prefix, and only the word right before it
    pub fn current_word_prefix_and_prev(&self) -> (String, Option<String>) {
        let (prefix, ctx) = self.current_word_context();
        (prefix, ctx.prev)
    }

    /// Extract the current word suffix immediately following the cursor
    pub fn current_word_suffix(&self) -> String {
        let cur = self.cursor.min(self.chars.len());
        let after = &self.chars[cur..];
        let mut end = 0;
        for (i, &c) in after.iter().enumerate() {
            if is_word_char(c) {
                end = i + 1;
            } else {
                break;
            }
        }
        after[..end].iter().collect()
    }

    /// Replace the current word around the cursor with the replacement string
    pub fn apply_replacement(
        &mut self,
        delete_before: usize,
        delete_after: usize,
        replacement: &str,
    ) {
        let cur = self.cursor.min(self.chars.len());
        let start = cur.saturating_sub(delete_before);
        let end = (cur + delete_after).min(self.chars.len());
        self.chars.drain(start..end);
        let repl_chars: Vec<char> = replacement.chars().collect();
        let repl_len = repl_chars.len();
        self.chars.splice(start..start, repl_chars);
        self.cursor = start + repl_len;
    }

    /// Synchronize from compositor surrounding text (windowed to prevent unbounded memory spikes)
    pub fn sync_from_surrounding(&mut self, before_cursor: &str, after_cursor: &str) {
        self.chars.clear();
        let before_chars: Vec<char> = before_cursor.chars().collect();
        let start = before_chars.len().saturating_sub(512);
        self.chars.extend(&before_chars[start..]);
        self.cursor = self.chars.len();
        self.chars.extend(after_cursor.chars().take(128));
    }
}

pub struct StateMachine {
    pub mode: InputMode,
    pub buffer: InputBuffer,
    pub max_candidates: usize,
    pub min_prefix_length: usize,
    /// Keys that commit the highlighted candidate while navigating
    pub accept_keys: AcceptKeys,
    /// Append a space after the committed word
    pub trailing_space: bool,
    /// Last (prefix, context) lookup and its result. GUI apps echo each keystroke back
    /// as surrounding text, which would otherwise repeat the same lookup two or three times.
    last_suggestion: Option<(String, Context, Vec<String>)>,
}

impl Default for StateMachine {
    fn default() -> Self {
        Self::new(3)
    }
}

impl StateMachine {
    pub fn new(max_candidates: usize) -> Self {
        Self {
            mode: InputMode::Idle,
            buffer: InputBuffer::new(),
            max_candidates,
            min_prefix_length: 1,
            accept_keys: AcceptKeys::default(),
            trailing_space: true,
            last_suggestion: None,
        }
    }

    /// Take the suggestion settings of a (re)loaded config, keeping the current mode and buffer
    pub fn apply_config(&mut self, config: &Config) {
        self.max_candidates = config.max_candidates;
        self.min_prefix_length = config.min_prefix_length.max(1);
        self.accept_keys = config.accept_keys;
        self.trailing_space = config.trailing_space;
        self.invalidate_suggestions();
    }

    /// Memoized `Dictionary::suggest` for the current prefix and context
    fn suggest(&mut self, dict: &Dictionary, prefix: &str, ctx: &Context) -> Vec<String> {
        if let Some((p, c, candidates)) = &self.last_suggestion
            && p == prefix
            && c == ctx
        {
            return candidates.clone();
        }
        let candidates = dict.suggest(prefix, ctx, self.max_candidates);
        self.last_suggestion = Some((prefix.to_string(), ctx.clone(), candidates.clone()));
        candidates
    }

    /// Whether `handle_key_press` might swallow this key instead of letting it reach the app.
    /// Only keys pressed while navigating, and Up while suggestions are shown (it starts
    /// navigating), can be swallowed; every other key is forwarded whatever it does here.
    pub fn may_swallow(&self, keysym: u32, ctrl_active: bool) -> bool {
        match self.mode {
            InputMode::Navigating { .. } => true,
            // The bar preselects its best candidate, so Tab takes it without an Up press
            InputMode::Suggesting { .. } => {
                !ctrl_active && (keysym == KEY_UP || (keysym == KEY_TAB && self.accept_keys.tab))
            }
            InputMode::Idle => false,
        }
    }

    /// Drop the memoized lookup after the dictionary changes (e.g. a learned phrase)
    pub fn invalidate_suggestions(&mut self) {
        self.last_suggestion = None;
    }

    pub fn with_min_prefix(mut self, min_prefix: usize) -> Self {
        self.min_prefix_length = min_prefix.max(1);
        self
    }

    /// Reset state to Idle
    pub fn reset(&mut self) -> KeyAction {
        self.mode = InputMode::Idle;
        self.buffer.clear();
        KeyAction::HideSuggestions
    }

    /// Handle text cursor context update from compositor (surrounding text)
    pub fn handle_surrounding_text(
        &mut self,
        text: &str,
        cursor_bytes: usize,
        anchor_bytes: usize,
        dict: &Dictionary,
    ) -> KeyAction {
        // If text is actively highlighted/selected (cursor != anchor), dismiss suggestions
        if cursor_bytes != anchor_bytes {
            self.buffer.clear();
            if self.mode != InputMode::Idle {
                self.mode = InputMode::Idle;
                return KeyAction::HideSuggestions;
            }
            return KeyAction::PassThrough;
        }

        // Only inspect if cursor is within valid UTF-8 slice boundaries
        if cursor_bytes > text.len() || !text.is_char_boundary(cursor_bytes) {
            return KeyAction::PassThrough;
        }

        let before_cursor = &text[..cursor_bytes];
        let after_cursor = &text[cursor_bytes..];

        let word_prefix = extract_word_prefix(before_cursor);
        let word_suffix = extract_word_suffix(after_cursor);
        let prev_slice = &before_cursor[..before_cursor.len().saturating_sub(word_prefix.len())];
        let context = extract_context_from_slice(prev_slice);

        // Always sync the internal buffer with compositor surrounding text
        self.buffer
            .sync_from_surrounding(before_cursor, after_cursor);

        if word_prefix.chars().count() >= self.min_prefix_length
            && word_prefix.chars().any(|c| c.is_alphabetic())
        {
            let candidates = self.suggest(dict, word_prefix, &context);
            if !candidates.is_empty() {
                match &self.mode {
                    InputMode::Navigating { selected_index, .. } => {
                        let sel = (*selected_index).min(candidates.len() - 1);
                        self.mode = InputMode::Navigating {
                            prefix: word_prefix.to_string(),
                            suffix: word_suffix.to_string(),
                            candidates: candidates.clone(),
                            selected_index: sel,
                        };
                        return KeyAction::UpdateSelection { index: sel };
                    }
                    _ => {
                        self.mode = InputMode::Suggesting {
                            prefix: word_prefix.to_string(),
                            suffix: word_suffix.to_string(),
                            candidates: candidates.clone(),
                        };
                        return KeyAction::ShowSuggestions {
                            prefix: word_prefix.to_string(),
                            candidates,
                        };
                    }
                }
            }
        }

        if self.mode != InputMode::Idle {
            self.mode = InputMode::Idle;
            return KeyAction::HideSuggestions;
        }

        KeyAction::PassThrough
    }

    /// Handle key press event
    /// keysym: XKB keysym
    /// ch: optional character representation
    /// ctrl_active: whether Control modifier is held
    pub fn handle_key_press(
        &mut self,
        keysym: u32,
        ch: Option<char>,
        ctrl_active: bool,
        dict: &Dictionary,
    ) -> KeyAction {
        // 1. Control key shortcuts (e.g. in bash/terminals/editors)
        if ctrl_active {
            match keysym {
                KEY_BACKSPACE | 0x0077 /* 'w' */ | 0x0057 /* 'W' */ => {
                    self.buffer.delete_word_backwards();
                    self.mode = InputMode::Idle;
                    return KeyAction::HideSuggestions;
                }
                0x0075 /* 'u' */ | 0x0055 /* 'U' */ => {
                    self.buffer.delete_to_start();
                    self.mode = InputMode::Idle;
                    return KeyAction::HideSuggestions;
                }
                0x006b /* 'k' */ | 0x004b /* 'K' */ => {
                    self.buffer.delete_to_end();
                    self.mode = InputMode::Idle;
                    return KeyAction::HideSuggestions;
                }
                0x0063 /* 'c' */ | 0x0043 /* 'C' */ => {
                    // Terminals throw the line away; GUI apps (copy) resync from surrounding text
                    self.buffer.clear();
                    self.mode = InputMode::Idle;
                    return KeyAction::HideSuggestions;
                }
                0x0061 /* 'a' */ | 0x0041 /* 'A' */ => {
                    self.buffer.move_home();
                    self.mode = InputMode::Idle;
                    return KeyAction::HideSuggestions;
                }
                0x0065 /* 'e' */ | 0x0045 /* 'E' */ => {
                    self.buffer.move_end();
                    self.mode = InputMode::Idle;
                    return KeyAction::HideSuggestions;
                }
                _ => {
                    if self.mode != InputMode::Idle {
                        self.mode = InputMode::Idle;
                        return KeyAction::HideSuggestions;
                    }
                    return KeyAction::PassThrough;
                }
            }
        }

        // 2. If currently in Navigating mode, handle candidate selection keys
        if let InputMode::Navigating {
            candidates,
            selected_index,
            ..
        } = &self.mode
        {
            let candidates = candidates.clone();
            let current_idx = *selected_index;
            let count = candidates.len();

            if count == 0 {
                self.mode = InputMode::Idle;
                return KeyAction::HideSuggestions;
            }

            let is_accept_key = match keysym {
                KEY_RETURN | KEY_KP_ENTER => self.accept_keys.enter,
                KEY_SPACE => self.accept_keys.space,
                KEY_TAB => self.accept_keys.tab,
                _ => false,
            };

            match keysym {
                KEY_RIGHT => {
                    let next_idx = (current_idx + 1) % count;
                    return self.select_candidate(next_idx, candidates);
                }

                KEY_LEFT => {
                    let prev_idx = (current_idx + count - 1) % count;
                    return self.select_candidate(prev_idx, candidates);
                }

                KEY_UP | KEY_DOWN | KEY_ESCAPE => {
                    // Up, Down, or Escape cancels navigation back to document and swallows key
                    self.mode = InputMode::Idle;
                    return KeyAction::CancelNavigation;
                }

                _ if is_accept_key => {
                    // Tab walks the list rather than taking the top word, so the alternatives
                    // can be compared before committing. Tab on the last candidate takes it,
                    // which is how you commit once the highlight has moved off the top one.
                    if self.accept_keys.tab && keysym == KEY_TAB {
                        if current_idx + 1 == count {
                            return self.commit_candidate(current_idx, candidates);
                        }
                        return self.select_candidate(current_idx + 1, candidates);
                    }
                    // Enter and Space take the candidate the bar is pointing at
                    return self.commit_candidate(current_idx, candidates);
                }

                _ => {
                    // Any other key (including Enter / Space / Tab when not in accept_keys) exits
                    // navigation and is handled exactly like a keypress while suggesting, so it
                    // still reaches the app
                    let (prefix, _) = self.buffer.current_word_prefix_and_prev();
                    self.mode = InputMode::Suggesting {
                        prefix,
                        suffix: self.buffer.current_word_suffix(),
                        candidates,
                    };
                    let action = self.handle_typing_key(keysym, ch, dict);
                    // Keys that leave the suggestions up still need the highlight removed
                    if action == KeyAction::PassThrough
                        && let InputMode::Suggesting {
                            prefix, candidates, ..
                        } = &self.mode
                    {
                        return KeyAction::ShowSuggestions {
                            prefix: prefix.clone(),
                            candidates: candidates.clone(),
                        };
                    }
                    return action;
                }
            }
        }

        self.handle_typing_key(keysym, ch, dict)
    }

    /// 3. Normal typing & navigation keys (anything not handled as a shortcut or navigation key)
    ///
    /// Replace the word around the caret with `candidates[index]` and hand the app the
    /// edit. Shared by both modes, so accepting the preselected candidate and accepting
    /// one picked while navigating produce the same commit.
    fn commit_candidate(&mut self, index: usize, candidates: Vec<String>) -> KeyAction {
        let chosen = candidates[index].clone();
        let (prefix, context) = self.buffer.current_word_context();
        let prev_word = context.prev.clone();
        let suffix = self.buffer.current_word_suffix();
        let replacement = if self.trailing_space {
            format!("{} ", chosen)
        } else {
            chosen.clone()
        };
        self.buffer
            .apply_replacement(prefix.chars().count(), suffix.chars().count(), &replacement);
        self.mode = InputMode::Idle;
        KeyAction::CommitCandidate {
            deleted_before: prefix,
            deleted_after: suffix,
            replacement,
            prev_word,
            chosen_word: chosen,
        }
    }

    /// Move the highlight to `index`, keeping the bar's word and suffix as they are
    fn select_candidate(&mut self, index: usize, candidates: Vec<String>) -> KeyAction {
        let (prefix, _) = self.buffer.current_word_prefix_and_prev();
        self.mode = InputMode::Navigating {
            prefix,
            suffix: self.buffer.current_word_suffix(),
            candidates,
            selected_index: index,
        };
        KeyAction::UpdateSelection { index }
    }

    fn handle_typing_key(&mut self, keysym: u32, ch: Option<char>, dict: &Dictionary) -> KeyAction {
        // The top candidate is highlighted as soon as the bar appears, so Tab can take it right
        // away. Without this the key would reach the app, and taking the suggestion would
        // need a separate Up press to select it first.
        //
        // Only Tab. Space and Enter keep their normal meaning while the bar is merely up:
        // every word in the dictionary is a prefix of some longer entry ("work" of "work-",
        // "set" of "set-up"), so accepting on Space would rewrite the word the user just
        // finished typing. Accepting with those needs the bar engaged first, via Up or an
        // arrow key.
        if self.accept_keys.tab
            && keysym == KEY_TAB
            && let InputMode::Suggesting { candidates, .. } = &self.mode
            && !candidates.is_empty()
        {
            let candidates = candidates.clone();
            return self.commit_candidate(0, candidates);
        }
        match keysym {
            KEY_UP => {
                // If in Suggesting mode with candidates, Up enters navigation!
                if let InputMode::Suggesting { candidates, .. } = &self.mode
                    && !candidates.is_empty()
                {
                    let c = candidates.clone();
                    let (prefix, _) = self.buffer.current_word_prefix_and_prev();
                    self.mode = InputMode::Navigating {
                        prefix,
                        suffix: self.buffer.current_word_suffix(),
                        candidates: c,
                        selected_index: 0,
                    };
                    return KeyAction::UpdateSelection { index: 0 };
                }
                // Otherwise user pressed Up in document: clear line buffer and pass through
                self.buffer.clear();
                self.mode = InputMode::Idle;
                KeyAction::PassThrough
            }

            KEY_DOWN => {
                self.buffer.clear();
                self.mode = InputMode::Idle;
                KeyAction::HideSuggestions
            }

            KEY_LEFT => {
                self.buffer.move_left();
                self.mode = InputMode::Idle;
                KeyAction::HideSuggestions
            }

            KEY_RIGHT => {
                self.buffer.move_right();
                self.mode = InputMode::Idle;
                KeyAction::HideSuggestions
            }

            KEY_HOME => {
                self.buffer.move_home();
                self.mode = InputMode::Idle;
                KeyAction::HideSuggestions
            }

            KEY_END => {
                self.buffer.move_end();
                self.mode = InputMode::Idle;
                KeyAction::HideSuggestions
            }

            KEY_ESCAPE => {
                self.buffer.clear();
                self.mode = InputMode::Idle;
                KeyAction::HideSuggestions
            }

            KEY_SPACE => {
                // Normal space typed in document is inserted into buffer so deleting back into previous word works!
                self.buffer.insert_char(' ');
                self.mode = InputMode::Idle;
                KeyAction::HideSuggestions
            }

            KEY_RETURN | KEY_KP_ENTER => {
                let line: String = self.buffer.chars.iter().collect();
                let is_sensitive = is_sensitive_command_line(&line);
                let was_suggesting = self.mode != InputMode::Idle;
                self.buffer.clear();
                self.mode = InputMode::Idle;
                if is_sensitive {
                    KeyAction::SensitiveCommandSubmitted
                } else if was_suggesting {
                    KeyAction::HideSuggestions
                } else {
                    KeyAction::PassThrough
                }
            }

            KEY_BACKSPACE => {
                let had_char = self.buffer.backspace();
                if had_char {
                    let (prefix, context) = self.buffer.current_word_context();
                    let suffix = self.buffer.current_word_suffix();
                    if prefix.chars().count() >= self.min_prefix_length
                        && prefix.chars().any(|c| c.is_alphabetic())
                    {
                        let candidates = self.suggest(dict, &prefix, &context);
                        if !candidates.is_empty() {
                            self.mode = InputMode::Suggesting {
                                prefix: prefix.clone(),
                                suffix,
                                candidates: candidates.clone(),
                            };
                            return KeyAction::ShowSuggestions { prefix, candidates };
                        }
                    }
                }
                self.mode = InputMode::Idle;
                KeyAction::HideSuggestions
            }

            KEY_DELETE => {
                self.buffer.delete();
                let (prefix, context) = self.buffer.current_word_context();
                let suffix = self.buffer.current_word_suffix();
                if prefix.chars().count() >= self.min_prefix_length
                    && prefix.chars().any(|c| c.is_alphabetic())
                {
                    let candidates = self.suggest(dict, &prefix, &context);
                    if !candidates.is_empty() {
                        self.mode = InputMode::Suggesting {
                            prefix: prefix.clone(),
                            suffix,
                            candidates: candidates.clone(),
                        };
                        return KeyAction::ShowSuggestions { prefix, candidates };
                    }
                }
                self.mode = InputMode::Idle;
                KeyAction::HideSuggestions
            }

            _ => {
                if let Some(c) = ch {
                    self.buffer.insert_char(c);
                    if is_word_char(c) {
                        let (prefix, context) = self.buffer.current_word_context();
                        let suffix = self.buffer.current_word_suffix();
                        if prefix.chars().count() >= self.min_prefix_length
                            && prefix.chars().any(|ch| ch.is_alphabetic())
                        {
                            let candidates = self.suggest(dict, &prefix, &context);
                            if !candidates.is_empty() {
                                self.mode = InputMode::Suggesting {
                                    prefix: prefix.clone(),
                                    suffix,
                                    candidates: candidates.clone(),
                                };
                                return KeyAction::ShowSuggestions { prefix, candidates };
                            }
                        }
                        if self.mode != InputMode::Idle {
                            self.mode = InputMode::Idle;
                            return KeyAction::HideSuggestions;
                        }
                    } else {
                        // Non-word character (punctuation, symbols, etc.)
                        if self.mode != InputMode::Idle {
                            self.mode = InputMode::Idle;
                            return KeyAction::HideSuggestions;
                        }
                    }
                }
                KeyAction::PassThrough
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_dict() -> Dictionary {
        let text = "program 1000\nprogress 800\nproject 900\nbeginning 950\nbegan 800\nbeggar 700\nscreen 900\nscript 800\nscrap 700\nhello 900\nhelp 800\nmorning 100\nmuch 500";
        let mut dict = Dictionary::from_frequency_text(text);
        dict.load_bigrams_tsv("good\tmorning\t1000000");
        dict
    }

    #[test]
    fn test_contextual_sentence_suggestions() {
        let dict = setup_dict();
        let mut sm = StateMachine::new(3);

        // Type "good "
        for c in "good ".chars() {
            sm.handle_key_press(c as u32, Some(c), false, &dict);
        }

        // Type "m" -> "good m" (1-letter prefix triggers contextual suggestions immediately!)
        let act = sm.handle_key_press('m' as u32, Some('m'), false, &dict);

        assert!(matches!(act, KeyAction::ShowSuggestions { .. }));
        if let KeyAction::ShowSuggestions { candidates, .. } = act {
            // "morning" should be #1 because of "good" context!
            assert_eq!(candidates[0], "morning");
        } else {
            panic!("Expected ShowSuggestions");
        }
    }

    #[test]
    fn test_normal_typing_and_arrow_freedom() {
        let dict = setup_dict();
        let mut sm = StateMachine::new(3);

        // Type 'p' -> triggers suggestions from 1st letter!
        let act = sm.handle_key_press(0x0070, Some('p'), false, &dict);
        assert!(matches!(act, KeyAction::ShowSuggestions { .. }));

        // Type 'r' -> refines suggestions
        let act = sm.handle_key_press(0x0072, Some('r'), false, &dict);
        assert!(matches!(act, KeyAction::ShowSuggestions { .. }));

        // Arrow Right should NOT be trapped, should pass through and hide suggestions
        let act = sm.handle_key_press(0xff53, None, false, &dict);
        assert_eq!(act, KeyAction::HideSuggestions);
        assert_eq!(sm.mode, InputMode::Idle);
    }

    #[test]
    fn test_windows_up_navigation_and_enter_swallow() {
        let dict = setup_dict();
        let mut sm = StateMachine::new(3);

        sm.handle_key_press(0x0070, Some('p'), false, &dict);
        sm.handle_key_press(0x0072, Some('r'), false, &dict);
        sm.handle_key_press(0x006f, Some('o'), false, &dict);

        // Press Up to enter navigation
        let act = sm.handle_key_press(0xff52, None, false, &dict);
        assert_eq!(act, KeyAction::UpdateSelection { index: 0 });

        // Press Right to cycle to index 1
        let act = sm.handle_key_press(0xff53, None, false, &dict);
        assert_eq!(act, KeyAction::UpdateSelection { index: 1 });

        // Press Enter: Must commit candidate and swallow key
        let act = sm.handle_key_press(0xff0d, None, false, &dict);
        assert_eq!(
            act,
            KeyAction::CommitCandidate {
                deleted_before: "pro".to_string(),
                deleted_after: String::new(),
                replacement: "project ".to_string(),
                prev_word: None,
                chosen_word: "project".to_string(),
            }
        );
        assert_eq!(sm.mode, InputMode::Idle);
    }

    #[test]
    fn test_space_selection_in_navigating() {
        let dict = setup_dict();
        let mut sm = StateMachine::new(3);

        sm.handle_key_press(0x0070, Some('p'), false, &dict);
        sm.handle_key_press(0x0072, Some('r'), false, &dict);
        sm.handle_key_press(0x006f, Some('o'), false, &dict);

        // Press Up to enter navigation
        let act = sm.handle_key_press(0xff52, None, false, &dict);
        assert_eq!(act, KeyAction::UpdateSelection { index: 0 });

        // Press Space: Must commit candidate and swallow Space key!
        let act = sm.handle_key_press(0x0020, Some(' '), false, &dict);
        assert_eq!(
            act,
            KeyAction::CommitCandidate {
                deleted_before: "pro".to_string(),
                deleted_after: String::new(),
                replacement: "program ".to_string(),
                prev_word: None,
                chosen_word: "program".to_string(),
            }
        );
        assert_eq!(sm.mode, InputMode::Idle);
    }

    /// The bar preselects its best candidate, so Tab walks the alternatives and commits
    /// the last one, which is the only way to commit once the highlight has moved.
    #[test]
    fn test_tab_walks_candidates_then_commits_the_last() {
        let dict = setup_dict();
        let mut sm = StateMachine::new(3);

        for c in "pro".chars() {
            sm.handle_key_press(c as u32, Some(c), false, &dict);
        }
        // Candidates are ["program", "project", "progress"]

        // Tab alone, with no Up press first, takes the preselected candidate
        let act = sm.handle_key_press(0xff09, None, false, &dict);
        assert_eq!(
            act,
            KeyAction::CommitCandidate {
                deleted_before: "pro".to_string(),
                deleted_after: String::new(),
                replacement: "program ".to_string(),
                prev_word: None,
                chosen_word: "program".to_string(),
            }
        );
        assert_eq!(sm.mode, InputMode::Idle);

        // With the bar engaged, Tab walks the list instead of taking the top word
        sm.reset();
        for c in "pro".chars() {
            sm.handle_key_press(c as u32, Some(c), false, &dict);
        }
        let act = sm.handle_key_press(0xff52, None, false, &dict);
        assert_eq!(act, KeyAction::UpdateSelection { index: 0 });
        assert_eq!(
            sm.handle_key_press(0xff09, None, false, &dict),
            KeyAction::UpdateSelection { index: 1 }
        );
        assert_eq!(
            sm.handle_key_press(0xff09, None, false, &dict),
            KeyAction::UpdateSelection { index: 2 }
        );
        // Tab on the last candidate takes it
        assert_eq!(
            sm.handle_key_press(0xff09, None, false, &dict),
            KeyAction::CommitCandidate {
                deleted_before: "pro".to_string(),
                deleted_after: String::new(),
                replacement: "progress ".to_string(),
                prev_word: None,
                chosen_word: "progress".to_string(),
            }
        );
        assert_eq!(sm.mode, InputMode::Idle);
    }

    /// Space and Enter keep their ordinary meaning while the bar is merely up: every
    /// dictionary word is a prefix of some longer entry, so accepting on Space would
    /// rewrite the word that was just finished.
    #[test]
    fn test_space_and_enter_do_not_accept_until_the_bar_is_engaged() {
        let dict = setup_dict();
        let mut sm = StateMachine::new(3);

        for c in "pro".chars() {
            sm.handle_key_press(c as u32, Some(c), false, &dict);
        }
        assert!(matches!(sm.mode, InputMode::Suggesting { .. }));

        // Space reaches the app rather than committing "program"
        let act = sm.handle_key_press(0x0020, Some(' '), false, &dict);
        assert!(!matches!(act, KeyAction::CommitCandidate { .. }), "{act:?}");
        assert_eq!(sm.buffer.chars.iter().collect::<String>(), "pro ");

        // So does Enter
        sm.reset();
        for c in "pro".chars() {
            sm.handle_key_press(c as u32, Some(c), false, &dict);
        }
        let act = sm.handle_key_press(0xff0d, None, false, &dict);
        assert!(!matches!(act, KeyAction::CommitCandidate { .. }), "{act:?}");

        // Once engaged with Up, both commit the highlighted candidate
        sm.reset();
        for c in "pro".chars() {
            sm.handle_key_press(c as u32, Some(c), false, &dict);
        }
        sm.handle_key_press(0xff52, None, false, &dict);
        assert!(matches!(
            sm.handle_key_press(0x0020, Some(' '), false, &dict),
            KeyAction::CommitCandidate { .. }
        ));
    }

    #[test]
    fn test_identifier_segments_are_completed_on_their_own() {
        let dict = setup_dict();
        // Only the segment being typed is completed; the rest of the identifier stands
        for (typed, segment, result) in [
            ("myProg", "Prog", "myProgram "),
            ("getProg", "Prog", "getProgram "),
            ("utf8Prog", "Prog", "utf8Program "),
            // A run of capitals ends before the capitalised word that follows it
            ("HTTPServ", "Serv", "HTTPScreen "),
            // Separators already split the word, and stay untouched
            ("my_prog", "prog", "my_program "),
            ("my-prog", "prog", "my-program "),
        ] {
            let mut sm = StateMachine::new(2);
            for c in typed.chars() {
                sm.handle_key_press(c as u32, Some(c), false, &dict);
            }
            assert_eq!(
                sm.buffer.current_word_context().0,
                segment,
                "segment of {typed:?}"
            );
            sm.handle_key_press(KEY_TAB, None, false, &dict);
            assert_eq!(
                sm.buffer.chars.iter().collect::<String>(),
                result,
                "completing {typed:?}"
            );
        }
    }

    #[test]
    fn test_identifier_needs_a_letter_in_the_last_segment() {
        let dict = setup_dict();
        // A trailing separator starts an empty segment, and there is nothing to complete
        // until a letter is typed
        for typed in ["my_", "my-", "my "] {
            let mut sm = StateMachine::new(2);
            for c in typed.chars() {
                sm.handle_key_press(c as u32, Some(c), false, &dict);
            }
            assert_eq!(
                sm.buffer.current_word_context().0,
                "",
                "segment of {typed:?}"
            );
            assert!(
                matches!(sm.mode, InputMode::Idle),
                "{typed:?} offered {:?}",
                sm.mode
            );
        }
    }

    #[test]
    fn test_camel_case_word_is_still_one_word_for_context() {
        let dict = setup_dict();
        let mut sm = StateMachine::new(3);
        // "good morning" is a phrase; "myMorning" is one identifier, so the earlier
        // segment must not be offered to the n-gram tables as the previous word
        for c in "myMor".chars() {
            sm.handle_key_press(c as u32, Some(c), false, &dict);
        }
        let (_, ctx) = sm.buffer.current_word_context();
        assert_eq!(ctx.prev, None);
        assert_eq!(ctx.prev_prev, None);
    }

    /// Whether the engine swallows the keystroke for this action instead of forwarding it
    fn is_swallowed(act: &KeyAction) -> bool {
        matches!(
            act,
            KeyAction::Consume
                | KeyAction::CancelNavigation
                | KeyAction::UpdateSelection { .. }
                | KeyAction::CommitCandidate { .. }
        )
    }

    /// Type `word` and press Up to highlight the first suggestion
    fn type_and_navigate(sm: &mut StateMachine, dict: &Dictionary, word: &str) {
        for c in word.chars() {
            sm.handle_key_press(c as u32, Some(c), false, dict);
        }
        let act = sm.handle_key_press(0xff52, None, false, dict);
        assert_eq!(act, KeyAction::UpdateSelection { index: 0 });
    }

    #[test]
    fn test_enter_outside_accept_keys_sends_instead_of_committing() {
        let dict = setup_dict();
        let mut sm = StateMachine::new(3);
        sm.accept_keys = AcceptKeys {
            enter: false,
            space: true,
            tab: true,
        };
        type_and_navigate(&mut sm, &dict, "pro");

        // Enter leaves navigation and reaches the app (e.g. sends the chat message)
        let act = sm.handle_key_press(0xff0d, None, false, &dict);
        assert!(!is_swallowed(&act), "{act:?}");
        assert_eq!(act, KeyAction::HideSuggestions);
        assert_eq!(sm.mode, InputMode::Idle);
        assert!(sm.buffer.chars.is_empty());

        // Keypad Enter counts as Enter
        type_and_navigate(&mut sm, &dict, "pro");
        let act = sm.handle_key_press(0xff8d, None, false, &dict);
        assert!(!is_swallowed(&act), "{act:?}");
    }

    #[test]
    fn test_space_outside_accept_keys_types_a_space() {
        let dict = setup_dict();
        let mut sm = StateMachine::new(3);
        sm.accept_keys = AcceptKeys {
            enter: true,
            space: false,
            tab: false,
        };
        type_and_navigate(&mut sm, &dict, "pro");

        let act = sm.handle_key_press(0x0020, Some(' '), false, &dict);
        assert!(!is_swallowed(&act), "{act:?}");
        assert_eq!(act, KeyAction::HideSuggestions);
        assert_eq!(sm.mode, InputMode::Idle);
        assert_eq!(sm.buffer.chars.iter().collect::<String>(), "pro ");

        // Tab is not an accept key either: it is forwarded and hides the bar
        type_and_navigate(&mut sm, &dict, "pro");
        let act = sm.handle_key_press(0xff09, Some('\t'), false, &dict);
        assert!(!is_swallowed(&act), "{act:?}");
        assert_eq!(act, KeyAction::HideSuggestions);

        // Enter still commits
        type_and_navigate(&mut sm, &dict, "pro");
        let act = sm.handle_key_press(0xff0d, None, false, &dict);
        assert!(matches!(act, KeyAction::CommitCandidate { .. }));
    }

    #[test]
    fn test_other_keys_leave_navigation_without_being_swallowed() {
        let dict = setup_dict();
        let mut sm = StateMachine::new(3);

        // Punctuation is typed and hides the bar
        type_and_navigate(&mut sm, &dict, "pro");
        let act = sm.handle_key_press('.' as u32, Some('.'), false, &dict);
        assert_eq!(act, KeyAction::HideSuggestions);
        assert_eq!(sm.mode, InputMode::Idle);
        assert_eq!(sm.buffer.chars.iter().collect::<String>(), "pro.");

        // A key without text (Shift) is forwarded; the bar stays up without the highlight
        sm.reset();
        type_and_navigate(&mut sm, &dict, "pro");
        let act = sm.handle_key_press(0xffe1, None, false, &dict);
        assert!(!is_swallowed(&act), "{act:?}");
        assert!(matches!(act, KeyAction::ShowSuggestions { .. }));
        assert!(matches!(sm.mode, InputMode::Suggesting { .. }));

        // Navigation is over, so Enter no longer commits
        let act = sm.handle_key_press(0xff0d, None, false, &dict);
        assert_eq!(act, KeyAction::HideSuggestions);
    }

    #[test]
    fn test_enter_outside_accept_keys_still_detects_sensitive_commands() {
        let dict = setup_dict();
        let mut sm = StateMachine::new(3);
        sm.accept_keys = AcceptKeys {
            enter: false,
            space: true,
            tab: true,
        };
        type_and_navigate(&mut sm, &dict, "sudo pro");

        let act = sm.handle_key_press(0xff0d, None, false, &dict);
        assert_eq!(act, KeyAction::SensitiveCommandSubmitted);
    }

    #[test]
    fn test_commit_without_trailing_space() {
        let dict = setup_dict();
        let mut sm = StateMachine::new(3);
        sm.trailing_space = false;
        type_and_navigate(&mut sm, &dict, "pro");

        // The accept key is still swallowed, and no space follows the word
        let act = sm.handle_key_press(0x0020, Some(' '), false, &dict);
        assert_eq!(
            act,
            KeyAction::CommitCandidate {
                deleted_before: "pro".to_string(),
                deleted_after: String::new(),
                replacement: "program".to_string(),
                prev_word: None,
                chosen_word: "program".to_string(),
            }
        );
        assert_eq!(sm.buffer.chars.iter().collect::<String>(), "program");
    }

    #[test]
    fn test_apply_config_keeps_buffer_and_refreshes_suggestions() {
        let dict = setup_dict();
        let mut sm = StateMachine::new(3);
        let act = sm.handle_surrounding_text("pr", 2, 2, &dict);
        assert!(
            matches!(act, KeyAction::ShowSuggestions { ref candidates, .. } if candidates.len() == 3)
        );

        let config = Config {
            max_candidates: 1,
            min_prefix_length: 2,
            accept_keys: AcceptKeys {
                enter: false,
                space: false,
                tab: true,
            },
            trailing_space: false,
            ..Config::default()
        };
        sm.apply_config(&config);
        assert_eq!(sm.buffer.chars.iter().collect::<String>(), "pr");
        assert!(matches!(sm.mode, InputMode::Suggesting { .. }));
        assert_eq!(sm.accept_keys, config.accept_keys);
        assert!(!sm.trailing_space);

        // Same prefix again: the memoized 3-candidate lookup must not be reused
        let act = sm.handle_surrounding_text("pr", 2, 2, &dict);
        assert!(
            matches!(act, KeyAction::ShowSuggestions { ref candidates, .. } if candidates.len() == 1)
        );

        // New min prefix length applies
        let act = sm.handle_surrounding_text("p", 1, 1, &dict);
        assert_eq!(act, KeyAction::HideSuggestions);
    }

    #[test]
    fn test_windows_down_cancel_navigation() {
        let dict = setup_dict();
        let mut sm = StateMachine::new(3);

        sm.handle_key_press(0x0070, Some('p'), false, &dict);
        sm.handle_key_press(0x0072, Some('r'), false, &dict);

        // Enter navigation
        sm.handle_key_press(0xff52, None, false, &dict);

        // Press Down: cancels selection and swallows key
        let act = sm.handle_key_press(0xff54, None, false, &dict);
        assert_eq!(act, KeyAction::CancelNavigation);
        assert_eq!(sm.mode, InputMode::Idle);
    }

    #[test]
    fn test_windows_up_toggle_cancels_navigation() {
        let dict = setup_dict();
        let mut sm = StateMachine::new(3);

        sm.handle_key_press(0x0070, Some('p'), false, &dict);
        sm.handle_key_press(0x0072, Some('r'), false, &dict);

        // Enter navigation with Up
        sm.handle_key_press(0xff52, None, false, &dict);

        // Press Up again: cancels navigation and swallows key
        let act = sm.handle_key_press(0xff52, None, false, &dict);
        assert_eq!(act, KeyAction::CancelNavigation);
        assert_eq!(sm.mode, InputMode::Idle);
    }

    #[test]
    fn test_internal_buffer_retro_edit_without_surrounding_text() {
        let dict = setup_dict();
        let mut sm = StateMachine::new(3);

        // Simulate typing "screen" in a terminal (no surrounding text events)
        for c in "screen".chars() {
            sm.handle_key_press(c as u32, Some(c), false, &dict);
        }
        let (prefix, _) = sm.buffer.current_word_prefix_and_prev();
        assert_eq!(prefix, "screen");
        assert_eq!(sm.buffer.current_word_suffix(), "");

        // User presses Left arrow 3 times to get to "scr|een"
        sm.handle_key_press(0xff51, None, false, &dict); // cursor at 5 ("scree|n")
        sm.handle_key_press(0xff51, None, false, &dict); // cursor at 4 ("scre|en")
        sm.handle_key_press(0xff51, None, false, &dict); // cursor at 3 ("scr|een")

        let (prefix, _) = sm.buffer.current_word_prefix_and_prev();
        assert_eq!(prefix, "scr");
        assert_eq!(sm.buffer.current_word_suffix(), "een");

        // User types 'a' -> buffer becomes "scra|een"
        let act = sm.handle_key_press('a' as u32, Some('a'), false, &dict);
        assert!(matches!(act, KeyAction::ShowSuggestions { .. }));
        let (prefix, _) = sm.buffer.current_word_prefix_and_prev();
        assert_eq!(prefix, "scra");
        assert_eq!(sm.buffer.current_word_suffix(), "een");

        // Press Up to navigate to suggestions for "scra" (e.g. "scrap")
        let act = sm.handle_key_press(0xff52, None, false, &dict);
        assert_eq!(act, KeyAction::UpdateSelection { index: 0 });

        // Commit with Space: must delete 4 before ("scra") and 3 after ("een")!
        let act = sm.handle_key_press(0x0020, Some(' '), false, &dict);
        assert_eq!(
            act,
            KeyAction::CommitCandidate {
                deleted_before: "scra".to_string(),
                deleted_after: "een".to_string(),
                replacement: "scrap ".to_string(),
                prev_word: None,
                chosen_word: "scrap".to_string(),
            }
        );
        assert_eq!(sm.mode, InputMode::Idle);
    }

    #[test]
    fn test_backspace_preserves_prefix_tracking() {
        let dict = setup_dict();
        let mut sm = StateMachine::new(3);

        sm.handle_key_press('p' as u32, Some('p'), false, &dict);
        sm.handle_key_press('r' as u32, Some('r'), false, &dict);
        sm.handle_key_press('o' as u32, Some('o'), false, &dict);

        // Backspace to 'pr'
        let act = sm.handle_key_press(0xff08, None, false, &dict);
        assert!(matches!(act, KeyAction::ShowSuggestions { .. }));
        let (prefix, _) = sm.buffer.current_word_prefix_and_prev();
        assert_eq!(prefix, "pr");

        // Backspace to 'p' (1 char >= min_prefix_length of 1, suggestions remain visible!)
        let act = sm.handle_key_press(0xff08, None, false, &dict);
        assert!(matches!(act, KeyAction::ShowSuggestions { .. }));
        let (prefix, _) = sm.buffer.current_word_prefix_and_prev();
        assert_eq!(prefix, "p");

        // Backspace again -> empty buffer -> hides suggestions
        let act = sm.handle_key_press(0xff08, None, false, &dict);
        assert_eq!(act, KeyAction::HideSuggestions);
        let (prefix, _) = sm.buffer.current_word_prefix_and_prev();
        assert_eq!(prefix, "");

        // Type 'p' again -> "p" -> suggestions reappear!
        let act = sm.handle_key_press('p' as u32, Some('p'), false, &dict);
        assert!(matches!(act, KeyAction::ShowSuggestions { .. }));
        let (prefix, _) = sm.buffer.current_word_prefix_and_prev();
        assert_eq!(prefix, "p");
    }

    #[test]
    fn test_delete_back_across_spaces_detects_beginning_of_word() {
        let dict = setup_dict();
        let mut sm = StateMachine::new(3);

        // Simulate typing "echo program testing"
        for c in "echo program testing".chars() {
            sm.handle_key_press(c as u32, Some(c), false, &dict);
        }

        // Backspace 7 times to delete "testing"
        for _ in 0..7 {
            sm.handle_key_press(0xff08, None, false, &dict);
        }

        // Backspace the space between "program" and "testing"
        sm.handle_key_press(0xff08, None, false, &dict);
        let (prefix, _) = sm.buffer.current_word_prefix_and_prev();
        assert_eq!(prefix, "program");

        // Backspace 'm' from "program" -> "progra"
        let act = sm.handle_key_press(0xff08, None, false, &dict);
        assert!(matches!(act, KeyAction::ShowSuggestions { .. }));
        let (prefix, _) = sm.buffer.current_word_prefix_and_prev();
        assert_eq!(prefix, "progra");

        // Backspace 'a' from "progra" -> "progr"
        let act = sm.handle_key_press(0xff08, None, false, &dict);
        assert!(matches!(act, KeyAction::ShowSuggestions { .. }));
        let (prefix, _) = sm.buffer.current_word_prefix_and_prev();
        assert_eq!(prefix, "progr");

        // Select candidate with Up and commit with Space
        sm.handle_key_press(0xff52, None, false, &dict); // Up
        let act = sm.handle_key_press(0x0020, Some(' '), false, &dict); // Space
        assert_eq!(
            act,
            KeyAction::CommitCandidate {
                deleted_before: "progr".to_string(),
                deleted_after: String::new(),
                replacement: "program ".to_string(),
                prev_word: Some("echo".to_string()),
                chosen_word: "program".to_string(),
            }
        );
        assert_eq!(sm.buffer.chars.iter().collect::<String>(), "echo program ");
    }

    #[test]
    fn test_ctrl_w_word_deletion_and_backspacing() {
        let dict = setup_dict();
        let mut sm = StateMachine::new(3);

        for c in "hello world".chars() {
            sm.handle_key_press(c as u32, Some(c), false, &dict);
        }

        // Ctrl+W deletes "world", leaving "hello "
        sm.handle_key_press(0x0077, Some('w'), true, &dict);
        assert_eq!(sm.buffer.chars.iter().collect::<String>(), "hello ");

        // Backspace deletes space -> "hello"
        sm.handle_key_press(0xff08, None, false, &dict);
        assert_eq!(sm.buffer.chars.iter().collect::<String>(), "hello");

        // Backspace 'o' -> "hell"
        let act = sm.handle_key_press(0xff08, None, false, &dict);
        assert!(matches!(act, KeyAction::ShowSuggestions { .. }));
        let (prefix, _) = sm.buffer.current_word_prefix_and_prev();
        assert_eq!(prefix, "hell");
    }

    #[test]
    fn test_word_boundary_retro_edit_with_surrounding_text() {
        let dict = setup_dict();
        let mut sm = StateMachine::new(3);

        // Document has "I was beginning to see"
        // Caret is after 'beg' (index 9)
        let text = "I was beginning to see";
        let act = sm.handle_surrounding_text(text, 9, 9, &dict);

        assert!(matches!(act, KeyAction::ShowSuggestions { .. }));
        if let InputMode::Suggesting { prefix, suffix, .. } = &sm.mode {
            assert_eq!(prefix, "beg");
            assert_eq!(suffix, "inning");
        } else {
            panic!("Expected Suggesting mode");
        }

        // Enter navigation
        sm.handle_key_press(0xff52, None, false, &dict);

        // Commit candidate with Space
        let act = sm.handle_key_press(0x0020, Some(' '), false, &dict);
        assert_eq!(
            act,
            KeyAction::CommitCandidate {
                deleted_before: "beg".to_string(),
                deleted_after: "inning".to_string(),
                replacement: "beginning ".to_string(),
                prev_word: Some("was".to_string()),
                chosen_word: "beginning".to_string(),
            }
        );
        assert_eq!(sm.mode, InputMode::Idle);
    }

    #[test]
    fn test_anchor_selection_hides_suggestions() {
        let dict = setup_dict();
        let mut sm = StateMachine::new(3);

        // Active text selection (cursor 10, anchor 15)
        let text = "I was beginning to see";
        let act = sm.handle_surrounding_text(text, 10, 15, &dict);
        assert_eq!(act, KeyAction::PassThrough);
        assert_eq!(sm.mode, InputMode::Idle);
    }

    #[test]
    fn test_custom_min_prefix_length() {
        let dict = setup_dict();
        let mut sm = StateMachine::new(3).with_min_prefix(2);

        // Type 'p' -> since min_prefix_length is 2, it passes through!
        let act = sm.handle_key_press(0x0070, Some('p'), false, &dict);
        assert_eq!(act, KeyAction::PassThrough);

        // Type 'r' -> prefix is "pr" (len 2 >= 2) -> shows suggestions!
        let act = sm.handle_key_press(0x0072, Some('r'), false, &dict);
        assert!(matches!(act, KeyAction::ShowSuggestions { .. }));

        // Backspace to 'p' (len 1 < 2) -> hides suggestions
        let act = sm.handle_key_press(0xff08, None, false, &dict);
        assert_eq!(act, KeyAction::HideSuggestions);
    }

    #[test]
    fn test_ctrl_c_forgets_the_line() {
        let dict = setup_dict();
        let mut sm = StateMachine::new(3);
        for c in "git commit".chars() {
            sm.handle_key_press(c as u32, Some(c), false, &dict);
        }
        assert_eq!(
            sm.handle_key_press(0x0063, None, true, &dict),
            KeyAction::HideSuggestions
        );
        assert!(sm.buffer.chars.is_empty());
        // The next line starts fresh, with no previous word from the cancelled one
        sm.handle_key_press('p' as u32, Some('p'), false, &dict);
        assert_eq!(
            sm.buffer.current_word_prefix_and_prev(),
            ("p".to_string(), None)
        );
    }

    #[test]
    fn test_apostrophe_continues_the_word() {
        let mut dict = Dictionary::from_frequency_text("don 900\ndone 800\nknow 500");
        dict.add_english_contractions();
        let mut sm = StateMachine::new(3);
        for c in "don'".chars() {
            sm.handle_key_press(c as u32, Some(c), false, &dict);
        }
        let (prefix, _) = sm.buffer.current_word_prefix_and_prev();
        assert_eq!(prefix, "don'");
        assert!(
            matches!(&sm.mode, InputMode::Suggesting { candidates, .. } if candidates[0] == "don't")
        );

        // Apps with typographic quotes report ’ in the surrounding text
        let text = "I don\u{2019}";
        let act = sm.handle_surrounding_text(text, text.len(), text.len(), &dict);
        assert!(
            matches!(act, KeyAction::ShowSuggestions { ref candidates, .. } if candidates[0] == "don't")
        );
        assert_eq!(
            extract_prev_word_from_slice("don\u{2019}t "),
            Some("don't".to_string())
        );
    }

    #[test]
    fn test_utf8_prev_word_slice() {
        assert_eq!(
            extract_prev_word_from_slice("café ").as_deref(),
            Some("café")
        );
        assert_eq!(
            extract_prev_word_from_slice("naïve\u{3000}").as_deref(),
            Some("naïve")
        );
    }

    #[test]
    fn test_context_reaches_two_words_back() {
        let ctx = extract_context_from_slice("as soon as ");
        assert_eq!(ctx.prev.as_deref(), Some("as"));
        assert_eq!(ctx.prev_prev.as_deref(), Some("soon"));

        // Punctuation between the words is fine
        let ctx = extract_context_from_slice("thank you, ");
        assert_eq!(ctx.prev.as_deref(), Some("you"));
        assert_eq!(ctx.prev_prev.as_deref(), Some("thank"));

        // Only one word before the caret: no trigram context
        let ctx = extract_context_from_slice("soon ");
        assert_eq!(ctx.prev.as_deref(), Some("soon"));
        assert_eq!(ctx.prev_prev, None);
        assert_eq!(extract_context_from_slice("").prev, None);
    }

    #[test]
    fn test_context_stops_at_a_sentence_boundary() {
        // The word before a full stop says nothing about the one after it, so the
        // trigram context has to start a fresh sentence rather than reach across
        let ctx = extract_context_from_slice("I am fine. ");
        assert_eq!(ctx.prev, None);
        assert_eq!(ctx.prev_prev, None);

        // Two words into the new sentence, both are available again
        let ctx = extract_context_from_slice("I am fine. as soon ");
        assert_eq!(ctx.prev.as_deref(), Some("soon"));
        assert_eq!(ctx.prev_prev.as_deref(), Some("as"));

        let ctx = extract_context_from_slice("done! as ");
        assert_eq!(ctx.prev.as_deref(), Some("as"));
        assert_eq!(ctx.prev_prev, None);
    }

    #[test]
    fn test_buffer_reports_two_words_of_context() {
        let mut b = InputBuffer::new();
        for c in "as soon as ".chars() {
            b.insert_char(c);
        }
        let (prefix, ctx) = b.current_word_context();
        assert_eq!(prefix, "");
        assert_eq!(ctx.prev.as_deref(), Some("as"));
        assert_eq!(ctx.prev_prev.as_deref(), Some("soon"));

        // The caret in the middle of a word: the prefix is what precedes it, and
        // the words behind it are still the ones around the caret
        for c in "mor".chars() {
            b.insert_char(c);
        }
        b.move_left();
        let (prefix, ctx) = b.current_word_context();
        assert_eq!(prefix, "mo");
        assert_eq!(ctx.prev.as_deref(), Some("as"));
        assert_eq!(ctx.prev_prev.as_deref(), Some("soon"));
    }

    /// The engine forwards keys that `may_swallow` rules out before running the state machine,
    /// so the state machine must never swallow such a key (the app would get a press without a
    /// release, i.e. a stuck key)
    #[test]
    fn test_may_swallow_covers_every_swallowing_key() {
        let dict = setup_dict();
        let keys: Vec<(u32, Option<char>)> = [
            KEY_UP,
            KEY_DOWN,
            KEY_LEFT,
            KEY_RIGHT,
            KEY_HOME,
            KEY_END,
            KEY_RETURN,
            KEY_KP_ENTER,
            KEY_TAB,
            KEY_ESCAPE,
            KEY_BACKSPACE,
            KEY_DELETE,
            0xffe1,
            0xffe3,
            0xffbe,
        ]
        .into_iter()
        .map(|k| (k, None))
        .chain([
            (KEY_SPACE, Some(' ')),
            (0x0067, Some('g')),
            (0x002e, Some('.')),
        ])
        .collect();

        for ctrl in [false, true] {
            for &(key, ch) in &keys {
                // Idle, Suggesting and Navigating, all reached by typing "pro"
                for setup in 0..3 {
                    let mut sm = StateMachine::new(3);
                    if setup >= 1 {
                        for c in "pro".chars() {
                            sm.handle_key_press(c as u32, Some(c), false, &dict);
                        }
                    }
                    if setup == 2 {
                        sm.handle_key_press(KEY_UP, None, false, &dict);
                    }
                    let may = sm.may_swallow(key, ctrl);
                    let act = sm.handle_key_press(key, ch, ctrl, &dict);
                    assert!(
                        may || !is_swallowed(&act),
                        "key {key:#x} ctrl={ctrl} setup={setup} swallowed via {act:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn test_is_sensitive_command_line() {
        assert!(is_sensitive_command_line("sudo pacman -S neovim"));
        assert!(is_sensitive_command_line("sudo -i"));
        assert!(is_sensitive_command_line("sudo su"));
        assert!(is_sensitive_command_line("su -"));
        assert!(is_sensitive_command_line("su"));
        assert!(is_sensitive_command_line("passwd"));
        assert!(is_sensitive_command_line("doas apt update"));
        assert!(is_sensitive_command_line("pkexec bash"));
        assert!(is_sensitive_command_line("ssh user@remote"));
        assert!(is_sensitive_command_line("echo hello && sudo tee file"));
        assert!(is_sensitive_command_line("/usr/bin/sudo vim /etc/hosts"));
        assert!(is_sensitive_command_line("pinentry-curses"));
        assert!(is_sensitive_command_line("op item get"));

        assert!(!is_sensitive_command_line("ls -la"));
        assert!(!is_sensitive_command_line("cargo test"));
        assert!(!is_sensitive_command_line("echo sudo"));
        assert!(!is_sensitive_command_line("cat sudo.txt"));
        assert!(!is_sensitive_command_line(""));
    }

    #[test]
    fn test_sensitive_command_submitted_key_action() {
        let dict = setup_dict();
        let mut sm = StateMachine::new(3);

        // Type "sudo pacman"
        for c in "sudo pacman".chars() {
            sm.handle_key_press(c as u32, Some(c), false, &dict);
        }

        // Press Enter (0xff0d)
        let act = sm.handle_key_press(0xff0d, None, false, &dict);
        assert_eq!(act, KeyAction::SensitiveCommandSubmitted);
        assert_eq!(sm.mode, InputMode::Idle);
        assert!(sm.buffer.chars.is_empty());
    }
}
