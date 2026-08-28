//! GNU mc(1) Options → Learn keys: terminal sequences for hard-to-send keys.
//!
//! Public behavior is taken from mc(1) “Learn keys”, the FAQ (F11–F20 /
//! Shift-Fn), ArchWiki `[terminal:TERM]` examples, and ticket #3134 (key
//! order and labels). Sequences use the GNU ini escapes (`\e`, `^i`, `\;`).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Incoming key identity (code + modifiers). `KeyEvent` also carries kind/state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeySig {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

impl KeySig {
    pub fn from_event(ev: &KeyEvent) -> Self {
        Self {
            code: ev.code,
            modifiers: ev.modifiers,
        }
    }

    pub fn to_event(self) -> KeyEvent {
        KeyEvent::new(self.code, self.modifiers)
    }

    pub fn matches(self, ev: &KeyEvent) -> bool {
        self.code == ev.code && self.modifiers == ev.modifiers
    }
}

/// Keys the Learn keys dialog can test and redefine (mc(1) functional keys,
/// arrows, editing keys, keypad `/*-+`, Completion/M-tab, Back Tab).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LearnKey {
    Space,
    Backspace,
    Insert,
    Delete,
    Home,
    End,
    PageUp,
    PageDown,
    Left,
    Right,
    Up,
    Down,
    F(u8),
    KpSlash,
    KpAsterisk,
    KpMinus,
    KpPlus,
    Complete,
    BackTab,
}

/// Dialog order: three columns (editing/arrows, F1–F12, F13–F20 + keypad + tabs).
/// Column lengths match [`COL_LENS`].
pub const LEARNABLE_KEYS: [LearnKey; 38] = [
    LearnKey::Space,
    LearnKey::Backspace,
    LearnKey::Insert,
    LearnKey::Delete,
    LearnKey::Home,
    LearnKey::End,
    LearnKey::PageUp,
    LearnKey::PageDown,
    LearnKey::Left,
    LearnKey::Right,
    LearnKey::Up,
    LearnKey::Down,
    LearnKey::F(1),
    LearnKey::F(2),
    LearnKey::F(3),
    LearnKey::F(4),
    LearnKey::F(5),
    LearnKey::F(6),
    LearnKey::F(7),
    LearnKey::F(8),
    LearnKey::F(9),
    LearnKey::F(10),
    LearnKey::F(11),
    LearnKey::F(12),
    LearnKey::F(13),
    LearnKey::F(14),
    LearnKey::F(15),
    LearnKey::F(16),
    LearnKey::F(17),
    LearnKey::F(18),
    LearnKey::F(19),
    LearnKey::F(20),
    LearnKey::KpSlash,
    LearnKey::KpAsterisk,
    LearnKey::KpMinus,
    LearnKey::KpPlus,
    LearnKey::Complete,
    LearnKey::BackTab,
];

/// Rows per column in the Learn keys grid (editing/arrows, F1–F12, rest).
pub const COL_LENS: [usize; 3] = [12, 12, 14];

const FKEY_INI: [&str; 21] = [
    "f0", "f1", "f2", "f3", "f4", "f5", "f6", "f7", "f8", "f9", "f10", "f11", "f12", "f13", "f14",
    "f15", "f16", "f17", "f18", "f19", "f20",
];
const FKEY_LABEL: [&str; 21] = [
    "F?", "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12", "F13", "F14",
    "F15", "F16", "F17", "F18", "F19", "F20",
];

fn fkey_index(n: u8) -> usize {
    if (1..=20).contains(&n) {
        n as usize
    } else {
        0
    }
}

#[derive(Debug, Clone)]
pub struct LearnKeyRow {
    pub key: LearnKey,
    /// First successful test (mc(1): “OK should appear next to the name”).
    pub ok: bool,
    /// Sequence captured via the key’s button (redefined; saved on Save).
    pub captured: Option<KeySig>,
}

impl LearnKey {
    pub fn ini_name(self) -> &'static str {
        match self {
            Self::Space => "space",
            Self::Backspace => "backspace",
            Self::Insert => "insert",
            Self::Delete => "delete",
            Self::Home => "home",
            Self::End => "end",
            Self::PageUp => "pageup",
            Self::PageDown => "pagedown",
            Self::Left => "left",
            Self::Right => "right",
            Self::Up => "up",
            Self::Down => "down",
            Self::F(n) => FKEY_INI[fkey_index(n)],
            Self::KpSlash => "kpslash",
            Self::KpAsterisk => "kpasterisk",
            Self::KpMinus => "kpminus",
            Self::KpPlus => "kpplus",
            Self::Complete => "complete",
            Self::BackTab => "backtab",
        }
    }

    pub fn from_ini_name(name: &str) -> Option<Self> {
        let n = name.trim().to_ascii_lowercase();
        match n.as_str() {
            "space" => Some(Self::Space),
            "backspace" => Some(Self::Backspace),
            "insert" => Some(Self::Insert),
            "delete" => Some(Self::Delete),
            "home" => Some(Self::Home),
            "end" => Some(Self::End),
            "pageup" | "pgup" | "pg_up" => Some(Self::PageUp),
            "pagedown" | "pgdn" | "pg_dn" => Some(Self::PageDown),
            "left" => Some(Self::Left),
            "right" => Some(Self::Right),
            "up" => Some(Self::Up),
            "down" => Some(Self::Down),
            "complete" => Some(Self::Complete),
            "backtab" | "back_tab" => Some(Self::BackTab),
            "kpslash" | "slash" => Some(Self::KpSlash),
            "kpasterisk" | "asterisk" | "kpmultiply" => Some(Self::KpAsterisk),
            "kpminus" | "minus" => Some(Self::KpMinus),
            "kpplus" | "plus" => Some(Self::KpPlus),
            other => {
                if let Some(rest) = other.strip_prefix('f') {
                    if let Ok(n) = rest.parse::<u8>() {
                        if (1..=20).contains(&n) {
                            return Some(Self::F(n));
                        }
                    }
                }
                None
            }
        }
    }

    /// Button label in the dialog (ticket #3134: no “key” suffix; “Back Tab”;
    /// keypad as `/ * - +`; Completion/M-tab).
    pub fn label(self) -> &'static str {
        match self {
            Self::Space => "Space",
            Self::Backspace => "Backspace",
            Self::Insert => "Insert",
            Self::Delete => "Delete",
            Self::Home => "Home",
            Self::End => "End",
            Self::PageUp => "PageUp",
            Self::PageDown => "PageDown",
            Self::Left => "Left",
            Self::Right => "Right",
            Self::Up => "Up",
            Self::Down => "Down",
            Self::F(n) => FKEY_LABEL[fkey_index(n)],
            Self::KpSlash => "/",
            Self::KpAsterisk => "*",
            Self::KpMinus => "-",
            Self::KpPlus => "+",
            Self::Complete => "Completion/M-tab",
            Self::BackTab => "Back Tab",
        }
    }

    pub fn canonical_event(self) -> KeyEvent {
        match self {
            Self::Space => KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
            Self::Backspace => KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
            Self::Insert => KeyEvent::new(KeyCode::Insert, KeyModifiers::NONE),
            Self::Delete => KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE),
            Self::Home => KeyEvent::new(KeyCode::Home, KeyModifiers::NONE),
            Self::End => KeyEvent::new(KeyCode::End, KeyModifiers::NONE),
            Self::PageUp => KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE),
            Self::PageDown => KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE),
            Self::Left => KeyEvent::new(KeyCode::Left, KeyModifiers::NONE),
            Self::Right => KeyEvent::new(KeyCode::Right, KeyModifiers::NONE),
            Self::Up => KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
            Self::Down => KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
            Self::F(n) => KeyEvent::new(KeyCode::F(n), KeyModifiers::NONE),
            Self::KpSlash => KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE),
            Self::KpAsterisk => KeyEvent::new(KeyCode::Char('*'), KeyModifiers::NONE),
            Self::KpMinus => KeyEvent::new(KeyCode::Char('-'), KeyModifiers::NONE),
            Self::KpPlus => KeyEvent::new(KeyCode::Char('+'), KeyModifiers::NONE),
            Self::Complete => KeyEvent::new(KeyCode::Tab, KeyModifiers::ALT),
            Self::BackTab => KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE),
        }
    }

    /// Map a recognized event onto a dialog key (including Shift-F3…F10 as F13–F20).
    pub fn from_event(ev: &KeyEvent) -> Option<Self> {
        let shift_only = ev.modifiers.contains(KeyModifiers::SHIFT)
            && !ev.modifiers.contains(KeyModifiers::CONTROL)
            && !ev.modifiers.contains(KeyModifiers::ALT);
        match ev.code {
            KeyCode::Char(' ') if ev.modifiers.is_empty() => Some(Self::Space),
            KeyCode::Backspace if ev.modifiers.is_empty() => Some(Self::Backspace),
            KeyCode::Insert if ev.modifiers.is_empty() => Some(Self::Insert),
            KeyCode::Delete if ev.modifiers.is_empty() => Some(Self::Delete),
            KeyCode::Home if ev.modifiers.is_empty() => Some(Self::Home),
            KeyCode::End if ev.modifiers.is_empty() => Some(Self::End),
            KeyCode::PageUp if ev.modifiers.is_empty() => Some(Self::PageUp),
            KeyCode::PageDown if ev.modifiers.is_empty() => Some(Self::PageDown),
            KeyCode::Left if ev.modifiers.is_empty() => Some(Self::Left),
            KeyCode::Right if ev.modifiers.is_empty() => Some(Self::Right),
            KeyCode::Up if ev.modifiers.is_empty() => Some(Self::Up),
            KeyCode::Down if ev.modifiers.is_empty() => Some(Self::Down),
            KeyCode::F(n @ 1..=20) if ev.modifiers.is_empty() => Some(Self::F(n)),
            KeyCode::F(n @ 3..=10) if shift_only => Some(Self::F(n.saturating_add(10))),
            KeyCode::Char('/') if ev.modifiers.is_empty() => Some(Self::KpSlash),
            KeyCode::Char('*') if ev.modifiers.is_empty() => Some(Self::KpAsterisk),
            KeyCode::Char('-') if ev.modifiers.is_empty() => Some(Self::KpMinus),
            KeyCode::Char('+') if ev.modifiers.is_empty() => Some(Self::KpPlus),
            KeyCode::Tab if ev.modifiers.contains(KeyModifiers::ALT) => Some(Self::Complete),
            KeyCode::BackTab => Some(Self::BackTab),
            KeyCode::Tab if shift_only => Some(Self::BackTab),
            _ => None,
        }
    }
}

pub fn dialog_rows() -> Vec<LearnKeyRow> {
    LEARNABLE_KEYS
        .iter()
        .map(|&key| LearnKeyRow {
            key,
            ok: false,
            captured: None,
        })
        .collect()
}

pub fn grid_col_row(index: usize) -> Option<(usize, usize)> {
    let mut i = index;
    for (col, &len) in COL_LENS.iter().enumerate() {
        if i < len {
            return Some((col, i));
        }
        i -= len;
    }
    None
}

pub fn grid_index(col: usize, row: usize) -> Option<usize> {
    if col >= COL_LENS.len() || row >= COL_LENS[col] {
        return None;
    }
    Some(COL_LENS.iter().take(col).sum::<usize>() + row)
}

pub fn current_term() -> String {
    std::env::var("TERM").unwrap_or_else(|_| "dumb".to_string())
}

pub type TermBindings = Vec<(LearnKey, Vec<u8>)>;
#[derive(Debug, Clone)]
pub struct LearnedKeyStore {
    pub term: String,
    remaps: Vec<(KeySig, LearnKey)>,
    saved: TermBindings,
    others: Vec<(String, TermBindings)>,
}

impl Default for LearnedKeyStore {
    fn default() -> Self {
        Self::for_term(current_term())
    }
}

impl LearnedKeyStore {
    pub fn for_term(term: String) -> Self {
        Self {
            term,
            remaps: Vec::new(),
            saved: Vec::new(),
            others: Vec::new(),
        }
    }

    /// Rewrite `ev` when it matches a learned incoming sequence.
    pub fn remap(&self, ev: KeyEvent) -> KeyEvent {
        for (sig, logical) in &self.remaps {
            if sig.matches(&ev) {
                let mut out = logical.canonical_event();
                out.kind = ev.kind;
                out.state = ev.state;
                return out;
            }
        }
        ev
    }

    pub fn binding_for(&self, key: LearnKey) -> Option<&[u8]> {
        self.saved
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, seq)| seq.as_slice())
    }

    pub fn set_binding(&mut self, logical: LearnKey, incoming: KeySig) {
        let canon = KeySig::from_event(&logical.canonical_event());
        if incoming == canon {
            self.saved.retain(|(k, _)| *k != logical);
            self.remaps.retain(|(_, k)| *k != logical);
            return;
        }
        let seq = keyevent_to_sequence(&incoming.to_event());
        self.saved.retain(|(k, _)| *k != logical);
        self.saved.push((logical, seq));
        self.remaps.retain(|(s, k)| *k != logical && *s != incoming);
        self.remaps.push((incoming, logical));
    }

    /// Merge dialog captures into the current TERM (already-working keys omitted).
    pub fn apply_dialog(&mut self, rows: &[LearnKeyRow]) {
        for row in rows {
            if let Some(sig) = row.captured {
                self.set_binding(row.key, sig);
            }
        }
    }

    pub fn load_ini_pair(&mut self, term: &str, name: &str, value: &str) {
        let Some(logical) = LearnKey::from_ini_name(name) else {
            return;
        };
        let seq = parse_gnu_sequence(value);
        if seq.is_empty() {
            return;
        }
        if term.eq_ignore_ascii_case(&self.term) {
            if let Some(ev) = sequence_to_keyevent(&seq) {
                self.set_binding(logical, KeySig::from_event(&ev));
            } else {
                self.saved.retain(|(k, _)| *k != logical);
                self.saved.push((logical, seq));
            }
        } else {
            match self
                .others
                .iter_mut()
                .find(|(t, _)| t.eq_ignore_ascii_case(term))
            {
                Some((_, pairs)) => {
                    pairs.retain(|(k, _)| *k != logical);
                    pairs.push((logical, seq));
                }
                None => self.others.push((term.to_string(), vec![(logical, seq)])),
            }
        }
    }

    /// `(term, pairs)` for every `[terminal:TERM]` section we should write.
    pub fn sections_to_write(&self) -> Vec<(String, TermBindings)> {
        let mut out = self.others.clone();
        if !self.saved.is_empty() {
            if let Some(existing) = out
                .iter_mut()
                .find(|(t, _)| t.eq_ignore_ascii_case(&self.term))
            {
                existing.1 = self.saved.clone();
            } else {
                out.push((self.term.clone(), self.saved.clone()));
            }
        }
        out
    }
}

/// Encode a captured key as a GNU `[terminal:TERM]` sequence.
pub fn keyevent_to_sequence(ev: &KeyEvent) -> Vec<u8> {
    let alt = ev.modifiers.contains(KeyModifiers::ALT);
    let ctrl = ev.modifiers.contains(KeyModifiers::CONTROL);
    let shift = ev.modifiers.contains(KeyModifiers::SHIFT);
    let mut seq = Vec::new();
    match ev.code {
        KeyCode::F(n) => return fkey_sequence(n, shift),
        KeyCode::Up => seq.extend(arrow_seq(b'A', shift)),
        KeyCode::Down => seq.extend(arrow_seq(b'B', shift)),
        KeyCode::Right => seq.extend(arrow_seq(b'C', shift)),
        KeyCode::Left => seq.extend(arrow_seq(b'D', shift)),
        KeyCode::Home => seq.extend(if shift {
            b"\x1b[1;2H".to_vec()
        } else {
            b"\x1b[H".to_vec()
        }),
        KeyCode::End => seq.extend(if shift {
            b"\x1b[1;2F".to_vec()
        } else {
            b"\x1b[F".to_vec()
        }),
        KeyCode::Insert => seq.extend(tilde_seq(2, shift)),
        KeyCode::Delete => seq.extend(tilde_seq(3, shift)),
        KeyCode::PageUp => seq.extend(tilde_seq(5, shift)),
        KeyCode::PageDown => seq.extend(tilde_seq(6, shift)),
        KeyCode::BackTab => seq.extend(b"\x1b[Z".to_vec()),
        KeyCode::Tab => {
            if alt {
                return b"\x1b\t".to_vec();
            }
            seq.push(b'\t');
        }
        KeyCode::Backspace => seq.push(0x7f),
        KeyCode::Esc => seq.push(0x1b),
        KeyCode::Enter => seq.push(b'\r'),
        KeyCode::Char(c) => {
            if alt {
                seq.push(0x1b);
            }
            if ctrl {
                let uc = (c as u8).to_ascii_uppercase();
                seq.push(uc & 0x1f);
            } else {
                let mut buf = [0u8; 4];
                seq.extend(c.encode_utf8(&mut buf).as_bytes());
            }
            return seq;
        }
        _ => {}
    }
    if alt && !seq.starts_with(&[0x1b]) {
        let mut v = vec![0x1b];
        v.extend(seq);
        return v;
    }
    seq
}

fn arrow_seq(final_byte: u8, shift: bool) -> Vec<u8> {
    if shift {
        vec![0x1b, b'[', b'1', b';', b'2', final_byte]
    } else {
        vec![0x1b, b'[', final_byte]
    }
}

fn tilde_seq(n: u8, shift: bool) -> Vec<u8> {
    if shift {
        format!("\x1b[{n};2~").into_bytes()
    } else {
        format!("\x1b[{n}~").into_bytes()
    }
}

fn fkey_sequence(n: u8, shift: bool) -> Vec<u8> {
    // FAQ / this project: F13–F20 are Shift-F3…Shift-F10 (not Shift-F1).
    let (base, use_shift) = if (13..=20).contains(&n) && !shift {
        (n - 10, true)
    } else {
        (n, shift)
    };
    match (base, use_shift) {
        (1, false) => b"\x1bOP".to_vec(),
        (2, false) => b"\x1bOQ".to_vec(),
        (3, false) => b"\x1bOR".to_vec(),
        (4, false) => b"\x1bOS".to_vec(),
        (1, true) => b"\x1b[1;2P".to_vec(),
        (2, true) => b"\x1b[1;2Q".to_vec(),
        (3, true) => b"\x1b[1;2R".to_vec(),
        (4, true) => b"\x1b[1;2S".to_vec(),
        (n @ 5..=12, sh) => {
            let code = fkey_tilde_code(n);
            if sh {
                format!("\x1b[{code};2~").into_bytes()
            } else {
                format!("\x1b[{code}~").into_bytes()
            }
        }
        (n, _) => format!("\x1b[{}~", 10u8.saturating_add(n)).into_bytes(),
    }
}

fn fkey_tilde_code(n: u8) -> u8 {
    match n {
        5 => 15,
        6 => 17,
        7 => 18,
        8 => 19,
        9 => 20,
        10 => 21,
        11 => 23,
        12 => 24,
        other => other,
    }
}

fn fkey_from_tilde_code(code: u16) -> Option<u8> {
    Some(match code {
        11 => 1,
        12 => 2,
        13 => 3,
        14 => 4,
        15 => 5,
        17 => 6,
        18 => 7,
        19 => 8,
        20 => 9,
        21 => 10,
        23 => 11,
        24 => 12,
        25 => 13,
        26 => 14,
        28 => 15,
        29 => 16,
        31 => 17,
        32 => 18,
        33 => 19,
        34 => 20,
        _ => return None,
    })
}

/// Decode a stored sequence back to the KeyEvent we would remap from.
pub fn sequence_to_keyevent(seq: &[u8]) -> Option<KeyEvent> {
    if seq.is_empty() {
        return None;
    }
    if seq == b"\x1b[Z" {
        return Some(KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE));
    }
    if seq == b"\x1b\t" {
        return Some(KeyEvent::new(KeyCode::Tab, KeyModifiers::ALT));
    }
    if seq == b"\x7f" || seq == b"\x08" {
        return Some(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
    }
    if seq == b" " {
        return Some(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
    }
    if seq == b"/" {
        return Some(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
    }
    if seq == b"*" {
        return Some(KeyEvent::new(KeyCode::Char('*'), KeyModifiers::NONE));
    }
    if seq == b"-" {
        return Some(KeyEvent::new(KeyCode::Char('-'), KeyModifiers::NONE));
    }
    if seq == b"+" {
        return Some(KeyEvent::new(KeyCode::Char('+'), KeyModifiers::NONE));
    }
    if seq == b"\r" || seq == b"\n" {
        return Some(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    }
    if seq == b"\t" {
        return Some(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    }
    // SS3 F1–F4 / application keypad
    if seq.len() == 3 && seq[0] == 0x1b && seq[1] == b'O' {
        return match seq[2] {
            b'P' => Some(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE)),
            b'Q' => Some(KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE)),
            b'R' => Some(KeyEvent::new(KeyCode::F(3), KeyModifiers::NONE)),
            b'S' => Some(KeyEvent::new(KeyCode::F(4), KeyModifiers::NONE)),
            b'o' => Some(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE)),
            b'j' => Some(KeyEvent::new(KeyCode::Char('*'), KeyModifiers::NONE)),
            b'm' => Some(KeyEvent::new(KeyCode::Char('-'), KeyModifiers::NONE)),
            b'k' => Some(KeyEvent::new(KeyCode::Char('+'), KeyModifiers::NONE)),
            b'A' => Some(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)),
            b'B' => Some(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)),
            b'C' => Some(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE)),
            b'D' => Some(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE)),
            b'H' => Some(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE)),
            b'F' => Some(KeyEvent::new(KeyCode::End, KeyModifiers::NONE)),
            _ => None,
        };
    }
    if seq.len() >= 3 && seq[0] == 0x1b && seq[1] == b'[' {
        return decode_csi(&seq[2..]);
    }
    if seq.len() == 2 && seq[0] == 0x1b {
        if seq[1] == b'\t' {
            return Some(KeyEvent::new(KeyCode::Tab, KeyModifiers::ALT));
        }
        if seq[1].is_ascii_graphic() || seq[1] == b' ' {
            return Some(KeyEvent::new(
                KeyCode::Char(seq[1] as char),
                KeyModifiers::ALT,
            ));
        }
    }
    if seq.len() == 1 {
        let c = seq[0];
        if (1..=26).contains(&c) {
            return Some(KeyEvent::new(
                KeyCode::Char((b'a' + (c - 1)) as char),
                KeyModifiers::CONTROL,
            ));
        }
        if c.is_ascii_graphic() {
            return Some(KeyEvent::new(KeyCode::Char(c as char), KeyModifiers::NONE));
        }
    }
    None
}

fn decode_csi(body: &[u8]) -> Option<KeyEvent> {
    if body.is_empty() {
        return None;
    }
    let final_byte = *body.last()?;
    let params = &body[..body.len() - 1];
    let mut nums: Vec<u16> = Vec::new();
    if !params.is_empty() {
        for part in params.split(|&b| b == b';') {
            if part.is_empty() {
                nums.push(0);
            } else {
                let s = std::str::from_utf8(part).ok()?;
                nums.push(s.parse().ok()?);
            }
        }
    }
    let first = nums.first().copied().unwrap_or(0);
    let second = nums.get(1).copied().unwrap_or(0);
    match final_byte {
        b'A' => Some(arrow_or_shift(KeyCode::Up, second)),
        b'B' => Some(arrow_or_shift(KeyCode::Down, second)),
        b'C' => Some(arrow_or_shift(KeyCode::Right, second)),
        b'D' => Some(arrow_or_shift(KeyCode::Left, second)),
        b'H' => Some(arrow_or_shift(KeyCode::Home, second)),
        b'F' => Some(arrow_or_shift(KeyCode::End, second)),
        b'Z' => Some(KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE)),
        b'P' => Some(shifted_f(1, second)),
        b'Q' => Some(shifted_f(2, second)),
        b'R' => Some(shifted_f(3, second)),
        b'S' => Some(shifted_f(4, second)),
        b'~' => {
            let n = if first == 0 { 1 } else { first };
            match n {
                2 => Some(arrow_or_shift(KeyCode::Insert, second)),
                3 => Some(arrow_or_shift(KeyCode::Delete, second)),
                5 => Some(arrow_or_shift(KeyCode::PageUp, second)),
                6 => Some(arrow_or_shift(KeyCode::PageDown, second)),
                code => {
                    let f = fkey_from_tilde_code(code)?;
                    Some(shifted_f(f, second))
                }
            }
        }
        _ => None,
    }
}

fn arrow_or_shift(code: KeyCode, mod_param: u16) -> KeyEvent {
    if mod_param == 2 {
        KeyEvent::new(code, KeyModifiers::SHIFT)
    } else {
        KeyEvent::new(code, KeyModifiers::NONE)
    }
}

fn shifted_f(base: u8, mod_param: u16) -> KeyEvent {
    if mod_param == 2 {
        // Shift-F3…F10 → F13–F20 (mc FAQ / this project’s File menu).
        if (3..=10).contains(&base) {
            KeyEvent::new(KeyCode::F(base + 10), KeyModifiers::NONE)
        } else {
            KeyEvent::new(KeyCode::F(base), KeyModifiers::SHIFT)
        }
    } else {
        KeyEvent::new(KeyCode::F(base), KeyModifiers::NONE)
    }
}

/// GNU mc.ini sequence escapes: `\e`, `\;`, `^i`, `^?`.
pub fn parse_gnu_sequence(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 1 < b.len() {
            match b[i + 1] {
                b'e' | b'E' => out.push(0x1b),
                b'n' => out.push(b'\n'),
                b'r' => out.push(b'\r'),
                b't' => out.push(b'\t'),
                b'b' => out.push(0x08),
                b'a' => out.push(0x07),
                b'\\' => out.push(b'\\'),
                b';' => out.push(b';'),
                other => out.push(other),
            }
            i += 2;
            continue;
        }
        if b[i] == b'^' && i + 1 < b.len() {
            let c = b[i + 1];
            if c == b'?' {
                out.push(0x7f);
            } else {
                out.push(c.to_ascii_uppercase() & 0x1f);
            }
            i += 2;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

pub fn format_gnu_sequence(bytes: &[u8]) -> String {
    let mut out = String::new();
    for &c in bytes {
        match c {
            0x1b => out.push_str("\\e"),
            b';' => out.push_str("\\;"),
            b'\\' => out.push_str("\\\\"),
            0x7f => out.push_str("^?"),
            0x09 => out.push_str("^i"),
            c if c < 0x20 => {
                out.push('^');
                out.push(((c + b'@') as char).to_ascii_lowercase());
            }
            c if c >= 0x80 => {
                // Keep as latin-1 so round-trips of 8-bit sequences stay intact.
                out.push(char::from(c));
            }
            c => out.push(c as char),
        }
    }
    out
}

/// Replace or append `[section]` in an ini document. `body` is `key=value` lines.
pub fn upsert_ini_section(contents: &str, section: &str, body: &str) -> String {
    let header = format!("[{section}]");
    let header_l = header.to_ascii_lowercase();
    let lines: Vec<&str> = contents.lines().collect();
    let mut start = None;
    let mut end = lines.len();
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim();
        if t.starts_with('[') && t.ends_with(']') && t.to_ascii_lowercase() == header_l {
            start = Some(i);
            end = lines.len();
            for (j, l2) in lines.iter().enumerate().skip(i + 1) {
                let t2 = l2.trim();
                if t2.starts_with('[') && t2.ends_with(']') {
                    end = j;
                    break;
                }
            }
            break;
        }
    }
    let mut block = vec![header];
    for l in body.lines() {
        if !l.is_empty() {
            block.push(l.to_string());
        }
    }
    let mut out: Vec<String> = Vec::new();
    match start {
        Some(s) => {
            out.extend(lines[..s].iter().map(|x| (*x).to_string()));
            out.extend(block);
            if end < lines.len() {
                if out.last().is_some_and(|x| !x.is_empty()) {
                    out.push(String::new());
                }
                out.extend(lines[end..].iter().map(|x| (*x).to_string()));
            }
        }
        None => {
            out.extend(lines.iter().map(|x| (*x).to_string()));
            if !out.is_empty() && out.last().is_some_and(|x| !x.is_empty()) {
                out.push(String::new());
            }
            out.extend(block);
        }
    }
    let mut s = out.join("\n");
    if !s.ends_with('\n') {
        s.push('\n');
    }
    s
}

pub fn format_terminal_section_body(pairs: &TermBindings) -> String {
    let mut body = String::new();
    for (key, seq) in pairs {
        body.push_str(key.ini_name());
        body.push('=');
        body.push_str(&format_gnu_sequence(seq));
        body.push('\n');
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gnu_sequence_roundtrip_escapes() {
        let raw = parse_gnu_sequence("\\e[1\\;2R");
        assert_eq!(raw, b"\x1b[1;2R");
        assert_eq!(format_gnu_sequence(&raw), "\\e[1\\;2R");
        assert_eq!(parse_gnu_sequence("^?"), b"\x7f");
        assert_eq!(parse_gnu_sequence("\\e^i"), b"\x1b\t");
    }

    #[test]
    fn f13_shift_f3_sequence_decodes() {
        let ev = sequence_to_keyevent(b"\x1b[1;2R").expect("decode");
        assert_eq!(ev.code, KeyCode::F(13));
        assert!(LearnKey::from_event(&ev) == Some(LearnKey::F(13)));
        let enc = keyevent_to_sequence(&KeyEvent::new(KeyCode::F(13), KeyModifiers::NONE));
        assert_eq!(enc, b"\x1b[1;2R");
    }

    #[test]
    fn complete_and_backtab_sequences() {
        let complete = keyevent_to_sequence(&LearnKey::Complete.canonical_event());
        assert_eq!(complete, b"\x1b\t");
        assert_eq!(
            sequence_to_keyevent(&complete).unwrap().modifiers,
            KeyModifiers::ALT
        );
        let bt = keyevent_to_sequence(&LearnKey::BackTab.canonical_event());
        assert_eq!(bt, b"\x1b[Z");
    }

    #[test]
    fn store_remaps_captured_f15_to_f13() {
        let mut store = LearnedKeyStore::for_term("xterm".into());
        let incoming = KeySig::from_event(&KeyEvent::new(KeyCode::F(15), KeyModifiers::NONE));
        store.set_binding(LearnKey::F(13), incoming);
        let got = store.remap(KeyEvent::new(KeyCode::F(15), KeyModifiers::NONE));
        assert_eq!(got.code, KeyCode::F(13));
        assert_eq!(
            store.binding_for(LearnKey::F(13)),
            Some(b"\x1b[15;2~".as_slice())
        );
    }

    #[test]
    fn load_wiki_f13_sequence() {
        let mut store = LearnedKeyStore::for_term("screen-256color".into());
        store.load_ini_pair("screen-256color", "f13", "\\e[1\\;2R");
        let got = store.remap(KeyEvent::new(KeyCode::F(13), KeyModifiers::NONE));
        // Identity sequence for F13 is not a remap.
        assert_eq!(got.code, KeyCode::F(13));
        store.load_ini_pair("screen-256color", "up", "\\e[A");
        // Canonical up is also identity.
        let up = store.remap(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(up.code, KeyCode::Up);
    }

    #[test]
    fn load_non_identity_remap() {
        let mut store = LearnedKeyStore::for_term("xterm".into());
        store.load_ini_pair("xterm", "f13", "\\e[15\\;2~");
        let got = store.remap(KeyEvent::new(KeyCode::F(15), KeyModifiers::NONE));
        assert_eq!(got.code, KeyCode::F(13));
    }

    #[test]
    fn upsert_preserves_other_sections() {
        let ini = "[layout]\nmenubar_visible=true\n\n[confirm]\ndelete=true\n";
        let out = upsert_ini_section(ini, "terminal:xterm", "f13=\\e[15\\;2~\n");
        assert!(out.contains("[layout]"));
        assert!(out.contains("[confirm]"));
        assert!(out.contains("[terminal:xterm]"));
        assert!(out.contains("f13=\\e[15\\;2~"));
        let out2 = upsert_ini_section(&out, "terminal:xterm", "up=\\e[A\n");
        assert_eq!(out2.matches("[terminal:xterm]").count(), 1);
        assert!(out2.contains("up=\\e[A"));
        assert!(!out2.contains("f13="));
    }

    #[test]
    fn grid_covers_all_keys() {
        assert_eq!(COL_LENS.iter().sum::<usize>(), LEARNABLE_KEYS.len());
        assert_eq!(grid_index(0, 0), Some(0));
        assert_eq!(grid_index(1, 0), Some(12));
        assert_eq!(grid_col_row(12), Some((1, 0)));
        assert_eq!(LearnKey::from_ini_name("F13"), Some(LearnKey::F(13)));
    }

    #[test]
    fn identity_binding_is_not_saved() {
        let mut store = LearnedKeyStore::for_term("xterm".into());
        store.set_binding(
            LearnKey::Up,
            KeySig::from_event(&LearnKey::Up.canonical_event()),
        );
        assert!(store.binding_for(LearnKey::Up).is_none());
        assert!(store.sections_to_write().is_empty());
    }
}
