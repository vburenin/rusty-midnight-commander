use crate::panel::TreeEntry;
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use walkdir::WalkDir;

/// Dialog height lower bound so GNU checkboxes, ignore-dirs field, and the results list fit.
pub const FIND_DIALOG_MIN_H: u16 = 23;
/// Dialog height upper bound (terminal-relative, same slack as before plus new rows).
pub const FIND_DIALOG_MAX_H: u16 = 29;
/// Rows from the top of the dialog to the results list (fields, checkboxes, ignore field, status).
pub const FIND_DIALOG_LIST_TOP: u16 = 17;
/// Results list starts on the row under the title (live GNU 4.8.30).
pub const FIND_RESULTS_LIST_TOP: u16 = 1;
/// `list_h = dialog_h - FIND_DIALOG_LIST_CHROME` (list top + button/border chrome).
pub const FIND_DIALOG_LIST_CHROME: u16 = 19;
/// Results chrome below the title: hline + Found + state + hline + 2 button rows + bottom.
pub const FIND_RESULTS_LIST_CHROME: u16 = 8;
/// Live GNU 4.8.30 results: 9-cell left/right margins (`COLS − 18`).
pub const FIND_RESULTS_H_MARGIN: u16 = 18;
/// Live GNU 4.8.30 results: 3-cell top/bottom margins (`LINES − 6`).
pub const FIND_RESULTS_V_MARGIN: u16 = 6;

/// Live GNU mc 4.8.30 Find File **setup** dialog (Alt-?): 66×17, two columns.
pub const FIND_SETUP_W: u16 = 66;
pub const FIND_SETUP_H: u16 = 17;
/// Interior left column (0-based from the left border, including the `│`).
pub const FIND_SETUP_X1: u16 = 2;
/// Interior right column (`Content:` / content checks).
pub const FIND_SETUP_X2: u16 = 34;
/// `[ Tree ]` origin, dialog-relative.
pub const FIND_SETUP_TREE_X: u16 = 56;
/// `[< OK >]` origin, dialog-relative.
pub const FIND_SETUP_OK_X: u16 = 23;
/// `[ Cancel ]` origin, dialog-relative.
pub const FIND_SETUP_CANCEL_X: u16 = 32;
/// Name / content input width (cells from the column origin to the other column).
pub const FIND_SETUP_FIELD_W: u16 = 32;

pub fn find_dialog_height(rows: u16) -> u16 {
    find_results_height(rows)
}

/// Live GNU 4.8.30 Find File results width: `COLS − 18`.
pub fn find_results_width(cols: u16) -> u16 {
    cols.saturating_sub(FIND_RESULTS_H_MARGIN).min(cols)
}

/// Live GNU 4.8.30 Find File results height: `LINES − 6`.
pub fn find_results_height(rows: u16) -> u16 {
    rows.saturating_sub(FIND_RESULTS_V_MARGIN).min(rows)
}

pub fn find_dialog_list_rows(dialog_h: u16) -> usize {
    dialog_h.saturating_sub(FIND_DIALOG_LIST_CHROME) as usize
}

pub fn find_results_list_rows(dialog_h: u16) -> usize {
    dialog_h.saturating_sub(FIND_RESULTS_LIST_CHROME) as usize
}

/// Live GNU 4.8.30 results: centered `COLS−18` × `LINES−6` (origin (9,3) on 80×24).
pub fn find_results_origin(cols: u16, rows: u16) -> (u16, u16) {
    let w = find_results_width(cols);
    let h = find_results_height(rows);
    let x = cols.saturating_add(1).saturating_sub(w) / 2;
    let y = rows.saturating_sub(h) / 2;
    (x, y)
}

/// Live GNU 4.8.30: ceil((cols − 66) / 2) left, full-screen vertical center.
pub fn find_setup_origin(cols: u16, rows: u16) -> (u16, u16) {
    let w = FIND_SETUP_W.min(cols);
    let h = FIND_SETUP_H.min(rows);
    let x = cols.saturating_add(1).saturating_sub(w) / 2;
    let y = rows.saturating_sub(h) / 2;
    (x, y)
}

/// Overlay height for the Find File directory-tree figure (mc(1) Tree button).
pub fn find_tree_picker_height(rows: u16) -> u16 {
    let cap = rows.min(16);
    cap.max(rows.min(10))
}

/// List rows inside the directory-tree overlay (title/border chrome).
pub fn find_tree_picker_list_rows(overlay_h: u16) -> usize {
    overlay_h.saturating_sub(4) as usize
}

/// `/` first, then each ancestor down to `start`, so the tree figure can walk up.
pub fn find_tree_ancestor_chain(start: &Path) -> Vec<PathBuf> {
    let start = if start.as_os_str().is_empty() {
        PathBuf::from("/")
    } else {
        start.to_path_buf()
    };
    let mut chain: Vec<PathBuf> = start.ancestors().map(|p| p.to_path_buf()).collect();
    chain.reverse();
    chain.retain(|p| !p.as_os_str().is_empty());
    if chain.is_empty() {
        chain.push(PathBuf::from("/"));
    }
    chain
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum NamePattern {
    Glob(String),
}

fn serde_default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FindParams {
    pub start_dir: PathBuf,
    pub name_pattern: NamePattern,
    pub content_substring: Option<String>,
    /// File-name column **Case sensitive** (GNU default on).
    pub case_sensitive: bool,
    /// Content-column **Regular expression**. Filename regex is `!using_shell_patterns`.
    pub regular_expression: bool,
    /// When false, only immediate children of `start_dir` are searched.
    pub find_recursively: bool,
    pub follow_symlinks: bool,
    /// Skip names starting with `.` (except `..`); hidden dirs are not descended.
    pub skip_hidden: bool,
    /// Content matches must form whole words (like grep -w). Filename pattern is unchanged.
    #[serde(default)]
    pub whole_words: bool,
    /// When set, skip directories listed in `ignore_dirs` during the walk.
    /// Live GNU 4.8.30 paints this checked on a clean profile.
    #[serde(default = "serde_default_true")]
    pub enable_ignore_dirs: bool,
    /// Colon-separated directory names or absolute paths; unused when the checkbox is off.
    #[serde(default)]
    pub ignore_dirs: String,
    /// File-name column **Using shell patterns** (GNU default on). Off → filename is a regex.
    #[serde(default = "serde_default_true")]
    pub using_shell_patterns: bool,
    /// Content-column **Case sensitive** (GNU default on).
    #[serde(default = "serde_default_true")]
    pub content_case_sensitive: bool,
    /// File-name column **All charsets** (stored; search stays UTF-8).
    #[serde(default)]
    pub file_all_charsets: bool,
    /// Content-column **All charsets** (stored; search stays UTF-8).
    #[serde(default)]
    pub content_all_charsets: bool,
    /// Content-column **First hit**: stop scanning a file after the first content match.
    #[serde(default)]
    pub first_hit: bool,
}

impl Default for FindParams {
    fn default() -> Self {
        Self {
            start_dir: PathBuf::new(),
            name_pattern: NamePattern::Glob("*".into()),
            content_substring: None,
            case_sensitive: true,
            regular_expression: false,
            find_recursively: true,
            follow_symlinks: false,
            skip_hidden: false,
            whole_words: false,
            enable_ignore_dirs: true,
            ignore_dirs: String::new(),
            using_shell_patterns: true,
            content_case_sensitive: true,
            file_all_charsets: false,
            content_all_charsets: false,
            first_hit: false,
        }
    }
}

/// Alt-? opens GNU's two-column setup; OK switches to the results list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindDialogPhase {
    Setup,
    Results,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindDialogFocus {
    StartDir,
    /// GNU mc(1) Find File **Tree** button (directory-tree figure).
    Tree,
    EnableIgnoreDirs,
    IgnoreDirs,
    NamePattern,
    Content,
    FindRecursively,
    FollowSymlinks,
    UsingShellPatterns,
    /// File-name column **Case sensitive**.
    CaseSensitive,
    FileAllCharsets,
    SkipHidden,
    WholeWords,
    /// Content-column **Regular expression**.
    RegularExpression,
    ContentCaseSensitive,
    ContentAllCharsets,
    FirstHit,
    /// GNU mc(1) **OK**: start a new search (setup only).
    ButtonOk,
    /// Setup-phase **Cancel** (closes without searching).
    ButtonCancel,
    /// Results listbox (live GNU default widget; Enter still fires Chdir).
    ResultsList,
    /// GNU 4.8.30 **Suspend** / **Continue**: pause a running or finished find, resume a stopped one.
    ButtonSuspend,
    ButtonAgain,
    ButtonChdir,
    ButtonPanelize,
    ButtonQuit,
    /// Results **View - F3**.
    ButtonView,
    /// Results **Edit - F4**.
    ButtonEdit,
}

impl FindDialogFocus {
    /// GNU widget-add Tab order on the two-column setup form.
    pub fn next_setup(self) -> Self {
        match self {
            Self::StartDir => Self::Tree,
            Self::Tree => Self::EnableIgnoreDirs,
            Self::EnableIgnoreDirs => Self::IgnoreDirs,
            Self::IgnoreDirs => Self::NamePattern,
            Self::NamePattern => Self::Content,
            Self::Content => Self::FindRecursively,
            Self::FindRecursively => Self::FollowSymlinks,
            Self::FollowSymlinks => Self::UsingShellPatterns,
            Self::UsingShellPatterns => Self::CaseSensitive,
            Self::CaseSensitive => Self::FileAllCharsets,
            Self::FileAllCharsets => Self::SkipHidden,
            Self::SkipHidden => Self::WholeWords,
            Self::WholeWords => Self::RegularExpression,
            Self::RegularExpression => Self::ContentCaseSensitive,
            Self::ContentCaseSensitive => Self::ContentAllCharsets,
            Self::ContentAllCharsets => Self::FirstHit,
            Self::FirstHit => Self::ButtonOk,
            Self::ButtonOk => Self::ButtonCancel,
            Self::ButtonCancel => Self::StartDir,
            // Results-only widgets wrap into the setup cycle from OK.
            Self::ResultsList
            | Self::ButtonSuspend
            | Self::ButtonAgain
            | Self::ButtonChdir
            | Self::ButtonPanelize
            | Self::ButtonQuit
            | Self::ButtonView
            | Self::ButtonEdit => Self::ButtonOk,
        }
    }

    pub fn prev_setup(self) -> Self {
        match self {
            Self::StartDir => Self::ButtonCancel,
            Self::Tree => Self::StartDir,
            Self::EnableIgnoreDirs => Self::Tree,
            Self::IgnoreDirs => Self::EnableIgnoreDirs,
            Self::NamePattern => Self::IgnoreDirs,
            Self::Content => Self::NamePattern,
            Self::FindRecursively => Self::Content,
            Self::FollowSymlinks => Self::FindRecursively,
            Self::UsingShellPatterns => Self::FollowSymlinks,
            Self::CaseSensitive => Self::UsingShellPatterns,
            Self::FileAllCharsets => Self::CaseSensitive,
            Self::SkipHidden => Self::FileAllCharsets,
            Self::WholeWords => Self::SkipHidden,
            Self::RegularExpression => Self::WholeWords,
            Self::ContentCaseSensitive => Self::RegularExpression,
            Self::ContentAllCharsets => Self::ContentCaseSensitive,
            Self::FirstHit => Self::ContentAllCharsets,
            Self::ButtonOk => Self::FirstHit,
            Self::ButtonCancel => Self::ButtonOk,
            Self::ResultsList
            | Self::ButtonSuspend
            | Self::ButtonAgain
            | Self::ButtonChdir
            | Self::ButtonPanelize
            | Self::ButtonQuit
            | Self::ButtonView
            | Self::ButtonEdit => Self::ButtonCancel,
        }
    }

    /// Results-phase widgets (list → Chdir → Again → Suspend → Quit → Panelize → View → Edit).
    pub fn next_results(self) -> Self {
        match self {
            Self::ResultsList => Self::ButtonChdir,
            Self::ButtonChdir => Self::ButtonAgain,
            Self::ButtonAgain => Self::ButtonSuspend,
            Self::ButtonSuspend => Self::ButtonQuit,
            Self::ButtonQuit => Self::ButtonPanelize,
            Self::ButtonPanelize => Self::ButtonView,
            Self::ButtonView => Self::ButtonEdit,
            Self::ButtonEdit => Self::ResultsList,
            _ => Self::ResultsList,
        }
    }

    pub fn prev_results(self) -> Self {
        match self {
            Self::ResultsList => Self::ButtonEdit,
            Self::ButtonChdir => Self::ResultsList,
            Self::ButtonAgain => Self::ButtonChdir,
            Self::ButtonSuspend => Self::ButtonAgain,
            Self::ButtonQuit => Self::ButtonSuspend,
            Self::ButtonPanelize => Self::ButtonQuit,
            Self::ButtonView => Self::ButtonPanelize,
            Self::ButtonEdit => Self::ButtonView,
            _ => Self::ResultsList,
        }
    }

    pub fn next_in(self, phase: FindDialogPhase) -> Self {
        match phase {
            FindDialogPhase::Setup => self.next_setup(),
            FindDialogPhase::Results => self.next_results(),
        }
    }

    pub fn prev_in(self, phase: FindDialogPhase) -> Self {
        match phase {
            FindDialogPhase::Setup => self.prev_setup(),
            FindDialogPhase::Results => self.prev_results(),
        }
    }

    /// Legacy combined-dialog cycle (results buttons after the form).
    pub fn next(self) -> Self {
        self.next_setup()
    }

    pub fn prev(self) -> Self {
        self.prev_setup()
    }

    pub fn is_checkbox(self) -> bool {
        matches!(
            self,
            Self::WholeWords
                | Self::CaseSensitive
                | Self::RegularExpression
                | Self::FindRecursively
                | Self::FollowSymlinks
                | Self::SkipHidden
                | Self::EnableIgnoreDirs
                | Self::UsingShellPatterns
                | Self::FileAllCharsets
                | Self::ContentCaseSensitive
                | Self::ContentAllCharsets
                | Self::FirstHit
        )
    }

    /// Tree plus setup/results action buttons.
    pub fn is_button(self) -> bool {
        self == Self::Tree || self.is_action_button()
    }

    /// Bottom-row GNU actions, including setup Cancel.
    pub fn is_action_button(self) -> bool {
        matches!(
            self,
            Self::ButtonOk
                | Self::ButtonCancel
                | Self::ButtonSuspend
                | Self::ButtonAgain
                | Self::ButtonChdir
                | Self::ButtonPanelize
                | Self::ButtonQuit
                | Self::ButtonView
                | Self::ButtonEdit
        )
    }

    /// Fields, Tree, and checkboxes: Up/Down walk these instead of the results list.
    pub fn is_form_widget(self) -> bool {
        self.is_checkbox()
            || matches!(
                self,
                Self::StartDir | Self::Tree | Self::NamePattern | Self::Content | Self::IgnoreDirs
            )
    }

    /// GNU spatial Down on the two-column setup (stay in column).
    pub fn setup_down(self) -> Self {
        match self {
            Self::StartDir | Self::Tree => Self::EnableIgnoreDirs,
            Self::EnableIgnoreDirs => Self::IgnoreDirs,
            Self::IgnoreDirs => Self::NamePattern,
            Self::NamePattern => Self::FindRecursively,
            Self::Content => Self::WholeWords,
            Self::FindRecursively => Self::FollowSymlinks,
            Self::FollowSymlinks => Self::UsingShellPatterns,
            Self::UsingShellPatterns => Self::CaseSensitive,
            Self::CaseSensitive => Self::FileAllCharsets,
            Self::FileAllCharsets => Self::SkipHidden,
            Self::SkipHidden => Self::ButtonOk,
            Self::WholeWords => Self::RegularExpression,
            Self::RegularExpression => Self::ContentCaseSensitive,
            Self::ContentCaseSensitive => Self::ContentAllCharsets,
            Self::ContentAllCharsets => Self::FirstHit,
            Self::FirstHit => Self::ButtonCancel,
            Self::ButtonOk | Self::ButtonCancel => self,
            other => other.next_setup(),
        }
    }

    /// GNU spatial Up on the two-column setup (stay in column).
    pub fn setup_up(self) -> Self {
        match self {
            Self::StartDir | Self::Tree => self,
            Self::EnableIgnoreDirs => Self::StartDir,
            Self::IgnoreDirs => Self::EnableIgnoreDirs,
            Self::NamePattern | Self::Content => Self::IgnoreDirs,
            Self::FindRecursively => Self::NamePattern,
            Self::FollowSymlinks => Self::FindRecursively,
            Self::UsingShellPatterns => Self::FollowSymlinks,
            Self::CaseSensitive => Self::UsingShellPatterns,
            Self::FileAllCharsets => Self::CaseSensitive,
            Self::SkipHidden => Self::FileAllCharsets,
            Self::WholeWords => Self::Content,
            Self::RegularExpression => Self::WholeWords,
            Self::ContentCaseSensitive => Self::RegularExpression,
            Self::ContentAllCharsets => Self::ContentCaseSensitive,
            Self::FirstHit => Self::ContentAllCharsets,
            Self::ButtonOk => Self::SkipHidden,
            Self::ButtonCancel => Self::FirstHit,
            other => other.prev_setup(),
        }
    }

    /// GNU spatial Left/Right: jump between the two columns on the same row.
    pub fn setup_across(self) -> Self {
        match self {
            Self::StartDir => Self::Tree,
            Self::Tree => Self::StartDir,
            Self::NamePattern => Self::Content,
            Self::Content => Self::NamePattern,
            Self::FindRecursively => Self::WholeWords,
            Self::WholeWords => Self::FindRecursively,
            Self::FollowSymlinks => Self::RegularExpression,
            Self::RegularExpression => Self::FollowSymlinks,
            Self::UsingShellPatterns => Self::ContentCaseSensitive,
            Self::ContentCaseSensitive => Self::UsingShellPatterns,
            Self::CaseSensitive => Self::ContentAllCharsets,
            Self::ContentAllCharsets => Self::CaseSensitive,
            Self::FileAllCharsets => Self::FirstHit,
            Self::FirstHit => Self::FileAllCharsets,
            Self::ButtonOk => Self::ButtonCancel,
            Self::ButtonCancel => Self::ButtonOk,
            other => other,
        }
    }
}

/// Flattened directory-tree figure opened from Find File's Tree button.
/// Lives on [`FindDialogState`] so Find File does not switch `UiMode`.
#[derive(Debug, Clone)]
pub struct FindTreePicker {
    pub entries: Vec<TreeEntry>,
    pub selected_index: usize,
    pub scroll_top: usize,
}

impl FindTreePicker {
    pub fn ensure_visible(&mut self, list_rows: usize) {
        if list_rows == 0 || self.entries.is_empty() {
            self.scroll_top = 0;
            return;
        }
        if self.selected_index < self.scroll_top {
            self.scroll_top = self.selected_index;
        } else if self.selected_index >= self.scroll_top + list_rows {
            self.scroll_top = self
                .selected_index
                .saturating_sub(list_rows.saturating_sub(1));
        }
        let max_top = self.entries.len().saturating_sub(list_rows);
        if self.scroll_top > max_top {
            self.scroll_top = max_top;
        }
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct FindResults {
    pub paths: Vec<PathBuf>,
}

/// Worker → UI events for the results dialog (hits plus the directory GNU shows).
#[derive(Debug, Clone)]
pub enum FindEvent {
    Hit(PathBuf),
    Progress(PathBuf),
}

/// One painted row in the GNU results listbox (directory header or a hit name).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FindDisplayRow {
    Header(PathBuf),
    File {
        name: String,
        path: PathBuf,
        hit_index: usize,
    },
}

impl FindDisplayRow {
    pub fn display_text(&self) -> String {
        match self {
            Self::Header(dir) => format!(" {}", dir.display()),
            Self::File { name, .. } => format!("    {name}"),
        }
    }

    pub fn hit_index(&self) -> Option<usize> {
        match self {
            Self::File { hit_index, .. } => Some(*hit_index),
            Self::Header(_) => None,
        }
    }
}

/// Group hits the way live GNU 4.8.30 does: a directory line, then 4-space names.
pub fn find_display_rows(paths: &[PathBuf]) -> Vec<FindDisplayRow> {
    let mut out = Vec::new();
    let mut last_parent: Option<PathBuf> = None;
    for (i, p) in paths.iter().enumerate() {
        let parent = p.parent().unwrap_or(p).to_path_buf();
        if last_parent.as_ref() != Some(&parent) {
            out.push(FindDisplayRow::Header(parent.clone()));
            last_parent = Some(parent);
        }
        let name = p
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| p.display().to_string());
        out.push(FindDisplayRow::File {
            name,
            path: p.clone(),
            hit_index: i,
        });
    }
    out
}

/// Display-row index of `hit_index`, or 0 if that hit is missing.
pub fn find_display_index_for_hit(rows: &[FindDisplayRow], hit_index: usize) -> usize {
    rows.iter()
        .position(|r| r.hit_index() == Some(hit_index))
        .unwrap_or(0)
}

#[derive(Debug, Clone)]
pub struct CancelHandle {
    cancel: Arc<AtomicBool>,
    pause: Arc<AtomicBool>,
}

impl CancelHandle {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {
            cancel: Arc::new(AtomicBool::new(false)),
            pause: Arc::new(AtomicBool::new(false)),
        }
    }
    pub fn flag(&self) -> Arc<AtomicBool> {
        self.cancel.clone()
    }
    pub fn pause_flag(&self) -> Arc<AtomicBool> {
        self.pause.clone()
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
    pub fn is_canceled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
    /// GNU Find File **Stop**: pause without discarding the walk.
    pub fn pause(&self) {
        self.pause.store(true, Ordering::Relaxed);
    }
    /// GNU Find File **Start**: continue a stopped search.
    pub fn resume(&self) {
        self.pause.store(false, Ordering::Relaxed);
    }
    pub fn is_paused(&self) -> bool {
        self.pause.load(Ordering::Relaxed)
    }
}

impl Default for CancelHandle {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
pub struct FindDialogState {
    pub params: FindParams,
    pub start_dir_edit: String,
    pub focus: FindDialogFocus,
    /// Alt-? opens [`FindDialogPhase::Setup`]; OK switches to results.
    pub phase: FindDialogPhase,
    pub running: bool,
    pub results: FindResults,
    pub cancel: Option<CancelHandle>,
    pub results_rx: Option<Receiver<FindEvent>>,
    pub selected_index: usize,
    pub scroll_top: usize,
    /// Directory-tree figure overlay (mc(1) Tree). `None` while closed.
    pub tree_picker: Option<FindTreePicker>,
    /// Directory GNU paints on the results status line while the walk is live.
    pub progress_dir: Option<PathBuf>,
    /// GNU **Suspend** on a running *or* finished find (status `Stopped`).
    pub stopped: bool,
    /// Found-line spinner (`\\|/−`) while a search is in progress.
    pub spin: u8,
}

impl FindDialogState {
    pub fn new(start_dir: PathBuf) -> Self {
        Self {
            params: FindParams {
                start_dir: start_dir.clone(),
                ..FindParams::default()
            },
            start_dir_edit: start_dir.display().to_string(),
            focus: FindDialogFocus::NamePattern,
            phase: FindDialogPhase::Setup,
            running: false,
            results: FindResults::default(),
            cancel: None,
            results_rx: None,
            selected_index: 0,
            scroll_top: 0,
            tree_picker: None,
            progress_dir: None,
            stopped: false,
            spin: 0,
        }
    }

    pub fn selected_path(&self) -> Option<&Path> {
        self.results
            .paths
            .get(self.selected_index)
            .map(|p| p.as_path())
    }

    /// Pull queued hits / progress without blocking. Returns true if the UI should redraw.
    pub fn drain_search(&mut self) -> bool {
        let mut evs = Vec::new();
        let mut disconnect = false;
        if let Some(rx) = &self.results_rx {
            loop {
                match rx.try_recv() {
                    Ok(ev) => evs.push(ev),
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        disconnect = true;
                        break;
                    }
                }
            }
        }
        let dirty = !evs.is_empty() || disconnect;
        for ev in evs {
            match ev {
                FindEvent::Hit(p) => self.results.paths.push(p),
                FindEvent::Progress(dir) => {
                    self.progress_dir = Some(dir);
                    self.spin = self.spin.wrapping_add(1);
                }
            }
        }
        if disconnect {
            self.running = false;
            self.cancel = None;
            self.results_rx = None;
        }
        dirty
    }

    /// Toggle the focused GNU checkbox. Returns true if a checkbox was focused.
    pub fn toggle_focused_checkbox(&mut self) -> bool {
        match self.focus {
            FindDialogFocus::CaseSensitive => {
                self.params.case_sensitive = !self.params.case_sensitive;
                true
            }
            FindDialogFocus::RegularExpression => {
                self.params.regular_expression = !self.params.regular_expression;
                true
            }
            FindDialogFocus::FindRecursively => {
                self.params.find_recursively = !self.params.find_recursively;
                true
            }
            FindDialogFocus::FollowSymlinks => {
                self.params.follow_symlinks = !self.params.follow_symlinks;
                true
            }
            FindDialogFocus::SkipHidden => {
                self.params.skip_hidden = !self.params.skip_hidden;
                true
            }
            FindDialogFocus::WholeWords => {
                self.params.whole_words = !self.params.whole_words;
                true
            }
            FindDialogFocus::UsingShellPatterns => {
                self.params.using_shell_patterns = !self.params.using_shell_patterns;
                true
            }
            FindDialogFocus::FileAllCharsets => {
                self.params.file_all_charsets = !self.params.file_all_charsets;
                true
            }
            FindDialogFocus::ContentCaseSensitive => {
                self.params.content_case_sensitive = !self.params.content_case_sensitive;
                true
            }
            FindDialogFocus::ContentAllCharsets => {
                self.params.content_all_charsets = !self.params.content_all_charsets;
                true
            }
            FindDialogFocus::FirstHit => {
                self.params.first_hit = !self.params.first_hit;
                true
            }
            FindDialogFocus::EnableIgnoreDirs => {
                self.params.enable_ignore_dirs = !self.params.enable_ignore_dirs;
                true
            }
            _ => false,
        }
    }

    /// GNU mc(1) **Suspend** is in effect: the worker is alive but waiting.
    pub fn is_paused(&self) -> bool {
        self.running && self.cancel.as_ref().is_some_and(|c| c.is_paused())
    }

    /// Live GNU: Suspend on a running *or* finished find shows `Stopped` / Continue.
    pub fn is_stopped(&self) -> bool {
        self.stopped || self.is_paused()
    }

    /// Drop the worker channel and request cancel so a paused walk cannot hang.
    pub fn abort_search(&mut self) {
        if let Some(ch) = &self.cancel {
            ch.cancel();
            ch.resume();
        }
        self.running = false;
        self.stopped = false;
        self.cancel = None;
        self.results_rx = None;
    }

    /// GNU **OK**: start a new search from the current fields.
    pub fn start_new_search(&mut self, fallback_cwd: PathBuf) {
        self.abort_search();
        let start_str = self.start_dir_edit.trim().to_string();
        self.params.start_dir = if start_str.is_empty() {
            fallback_cwd
        } else {
            PathBuf::from(start_str)
        };
        self.results.paths.clear();
        self.selected_index = 0;
        self.scroll_top = 0;
        self.progress_dir = Some(self.params.start_dir.clone());
        self.stopped = false;
        self.spin = 0;
        let params = self.params.clone();
        let cancel = CancelHandle::new();
        let flag = cancel.flag();
        let pause = cancel.pause_flag();
        let (tx, rx) = std::sync::mpsc::channel();
        self.cancel = Some(cancel);
        self.results_rx = Some(rx);
        self.running = true;
        self.phase = FindDialogPhase::Results;
        self.focus = FindDialogFocus::ResultsList;
        std::thread::spawn(move || {
            search_files_streaming_events(&params, &flag, &pause, |ev| {
                let _ = tx.send(ev);
            });
        });
    }

    /// GNU **Suspend**: pause a running walk and mark the find Stopped (also when finished).
    pub fn suspend_search(&mut self) {
        if self.running {
            if let Some(ch) = &self.cancel {
                ch.pause();
            }
        }
        self.stopped = true;
    }

    /// GNU **Continue**: resume a suspended walk; if the walk already finished, just un-stop.
    pub fn continue_search(&mut self) {
        self.stopped = false;
        if self.is_paused() {
            if let Some(ch) = &self.cancel {
                ch.resume();
            }
        }
    }

    /// GNU **Stop**: pause a running search. No-op if idle or already stopped.
    pub fn pause_search(&mut self) {
        self.suspend_search();
    }

    /// GNU **Start**: continue a stopped search. Does not start a new one.
    pub fn resume_search(&mut self) {
        self.continue_search();
    }

    /// GNU **Again**: ask for new parameters (focus the filename field; do not search).
    pub fn again_parameters(&mut self) {
        self.abort_search();
        self.phase = FindDialogPhase::Setup;
        self.focus = FindDialogFocus::NamePattern;
    }

    /// Keep the selected hit inside the visible GNU listbox (display rows include headers).
    pub fn ensure_hit_visible(&mut self, list_rows: usize) {
        let rows = find_display_rows(&self.results.paths);
        let idx = find_display_index_for_hit(&rows, self.selected_index);
        if list_rows == 0 {
            self.scroll_top = 0;
            return;
        }
        if idx < self.scroll_top {
            self.scroll_top = idx;
        } else if idx >= self.scroll_top + list_rows {
            self.scroll_top = idx.saturating_sub(list_rows.saturating_sub(1));
        }
        let max_top = rows.len().saturating_sub(list_rows);
        if self.scroll_top > max_top {
            self.scroll_top = max_top;
        }
    }
}

pub fn search_files(params: &FindParams, cancel: &Arc<AtomicBool>) -> Vec<PathBuf> {
    let pause = Arc::new(AtomicBool::new(false));
    let mut out = Vec::new();
    search_files_streaming(params, cancel, &pause, |p| out.push(p));
    out
}

pub fn search_files_streaming<F: FnMut(PathBuf)>(
    params: &FindParams,
    cancel: &Arc<AtomicBool>,
    pause: &Arc<AtomicBool>,
    mut on_hit: F,
) {
    search_files_streaming_events(params, cancel, pause, |ev| {
        if let FindEvent::Hit(p) = ev {
            on_hit(p);
        }
    });
}

pub fn search_files_streaming_events<F: FnMut(FindEvent)>(
    params: &FindParams,
    cancel: &Arc<AtomicBool>,
    pause: &Arc<AtomicBool>,
    mut on_event: F,
) {
    let name_pat = match &params.name_pattern {
        NamePattern::Glob(s) => s.as_str(),
    };

    let name_is_regex = !params.using_shell_patterns;
    let name_re = if name_is_regex {
        match RegexBuilder::new(name_pat)
            .case_insensitive(!params.case_sensitive)
            .build()
        {
            Ok(re) => Some(re),
            Err(_) => return,
        }
    } else {
        None
    };
    let glob = if name_is_regex {
        None
    } else {
        Some(GlobMatcher::new(name_pat, params.case_sensitive))
    };

    let content_filter = compile_content_filter(params);
    let whole_words = params.whole_words;

    let root = params.start_dir.clone();
    let skip_hidden = params.skip_hidden;
    let ignore = IgnoreSpec::from_params(params);
    let mut walker = WalkDir::new(&root).follow_links(params.follow_symlinks);
    if !params.find_recursively {
        walker = walker.max_depth(1);
    }
    let mut last_progress: Option<PathBuf> = None;
    on_event(FindEvent::Progress(root.clone()));
    for entry in walker
        .into_iter()
        .filter_entry(|e| keep_walk_entry(e, skip_hidden, ignore.as_ref()))
        .filter_map(Result::ok)
    {
        if wait_while_paused(cancel, pause) {
            break;
        }
        let p = entry.path();
        let progress = if entry.file_type().is_dir() {
            p.to_path_buf()
        } else {
            p.parent().unwrap_or(p).to_path_buf()
        };
        if last_progress.as_ref() != Some(&progress) {
            last_progress = Some(progress.clone());
            on_event(FindEvent::Progress(progress));
        }
        // Do not include the search root directory itself as a hit
        if p == root {
            continue;
        }
        let name = entry.file_name().to_string_lossy();
        let name_ok = if let Some(re) = &name_re {
            re.is_match(&name)
        } else if let Some(g) = &glob {
            g.is_match(&name)
        } else {
            false
        };
        if !name_ok {
            continue;
        }
        match &content_filter {
            ContentFilter::None => on_event(FindEvent::Hit(p.to_path_buf())),
            ContentFilter::InvalidRegex => {}
            ContentFilter::Substring(q) => {
                if entry.file_type().is_file()
                    && file_contains(p, q, params.content_case_sensitive, whole_words)
                {
                    on_event(FindEvent::Hit(p.to_path_buf()));
                }
            }
            ContentFilter::Regex(re) => {
                if entry.file_type().is_file() && file_contains_regex(p, re, whole_words) {
                    on_event(FindEvent::Hit(p.to_path_buf()));
                }
            }
        }
    }
}

/// Returns true if the caller should stop the walk (cancel).
fn wait_while_paused(cancel: &AtomicBool, pause: &AtomicBool) -> bool {
    while pause.load(Ordering::Relaxed) {
        if cancel.load(Ordering::Relaxed) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    cancel.load(Ordering::Relaxed)
}

fn keep_walk_entry(
    entry: &walkdir::DirEntry,
    skip_hidden: bool,
    ignore: Option<&IgnoreSpec>,
) -> bool {
    if let Some(spec) = ignore {
        if spec.skips(entry) {
            return false;
        }
    }
    if !skip_hidden || entry.depth() == 0 {
        return true;
    }
    let name = entry.file_name().to_string_lossy();
    !(name.starts_with('.') && name.as_ref() != "..")
}

/// Directories to skip when "Enable ignore directories" is on (mc(1) Find File).
struct IgnoreSpec {
    /// Absolute paths: skip the path itself and anything under it.
    abs: Vec<PathBuf>,
    /// Relative tokens: skip a directory whose file name equals the token.
    names: Vec<String>,
}

impl IgnoreSpec {
    fn from_params(params: &FindParams) -> Option<Self> {
        if !params.enable_ignore_dirs {
            return None;
        }
        let mut abs = Vec::new();
        let mut names = Vec::new();
        for token in params.ignore_dirs.split(':') {
            if token.is_empty() {
                continue;
            }
            if token == "." {
                // Man page: a lone dot means the current absolute start directory.
                abs.push(params.start_dir.clone());
            } else {
                let p = Path::new(token);
                if p.is_absolute() {
                    abs.push(p.to_path_buf());
                } else {
                    names.push(token.to_string());
                }
            }
        }
        Some(Self { abs, names })
    }

    fn skips(&self, entry: &walkdir::DirEntry) -> bool {
        let path = entry.path();
        if self.abs.iter().any(|p| path.starts_with(p)) {
            return true;
        }
        if entry.file_type().is_dir() {
            let name = entry.file_name().to_string_lossy();
            if self.names.iter().any(|n| n == name.as_ref()) {
                return true;
            }
        }
        false
    }
}

enum ContentFilter<'a> {
    None,
    Substring(&'a str),
    Regex(Regex),
    InvalidRegex,
}

fn compile_content_filter(params: &FindParams) -> ContentFilter<'_> {
    match params.content_substring.as_deref() {
        None => ContentFilter::None,
        Some(q) if params.regular_expression => match RegexBuilder::new(q)
            .case_insensitive(!params.content_case_sensitive)
            .build()
        {
            Ok(re) => ContentFilter::Regex(re),
            Err(_) => ContentFilter::InvalidRegex,
        },
        Some(q) => ContentFilter::Substring(q),
    }
}

fn file_contains(path: &Path, needle: &str, case_sensitive: bool, whole_words: bool) -> bool {
    for_each_line(path, |buf| {
        line_contains(buf, needle, case_sensitive, whole_words)
    })
}

fn file_contains_regex(path: &Path, re: &Regex, whole_words: bool) -> bool {
    for_each_line(path, |buf| {
        if !whole_words {
            re.is_match(buf)
        } else {
            // Apply grep -w bounds around each match; do not wrap the user pattern
            // (wrapping would break anchors such as ^ and $).
            re.find_iter(buf)
                .any(|m| match_is_whole_word(buf, m.start(), m.end()))
        }
    })
}

fn is_ascii_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// grep -w: the match is a whole word if it is bounded by non-word chars or string edges.
fn match_is_whole_word(hay: &str, start: usize, end: usize) -> bool {
    let before_ok = start == 0
        || hay
            .get(..start)
            .and_then(|s| s.chars().next_back())
            .is_none_or(|c| !is_ascii_word_char(c));
    let after_ok = end >= hay.len()
        || hay
            .get(end..)
            .and_then(|s| s.chars().next())
            .is_none_or(|c| !is_ascii_word_char(c));
    before_ok && after_ok
}

fn line_contains(hay: &str, needle: &str, case_sensitive: bool, whole_words: bool) -> bool {
    if !whole_words {
        return if case_sensitive {
            hay.contains(needle)
        } else {
            hay.to_lowercase().contains(&needle.to_lowercase())
        };
    }
    if needle.is_empty() {
        return true;
    }
    if case_sensitive {
        find_whole_word_substring(hay, needle)
    } else {
        find_whole_word_substring(&hay.to_lowercase(), &needle.to_lowercase())
    }
}

fn find_whole_word_substring(hay: &str, needle: &str) -> bool {
    let mut from = 0;
    while from <= hay.len() {
        let Some(rel) = hay.get(from..).and_then(|rest| rest.find(needle)) else {
            return false;
        };
        let start = from + rel;
        let end = start + needle.len();
        if match_is_whole_word(hay, start, end) {
            return true;
        }
        let Some(ch) = hay[start..].chars().next() else {
            return false;
        };
        from = start + ch.len_utf8();
    }
    false
}

fn for_each_line(path: &Path, mut pred: impl FnMut(&str) -> bool) -> bool {
    use std::fs::File;
    use std::io::{BufRead, BufReader};
    let file = match File::open(path) {
        Ok(f) => f,
        Err(_) => return false,
    };
    let mut buf = String::new();
    let mut reader = BufReader::new(file);
    loop {
        buf.clear();
        match reader.read_line(&mut buf) {
            Ok(0) => break,
            Ok(_) => {
                if pred(&buf) {
                    return true;
                }
            }
            Err(_) => break,
        }
    }
    false
}

// Very small glob matcher supporting * and ?
struct GlobMatcher {
    pat: String,
    case_sensitive: bool,
}

impl GlobMatcher {
    fn new(pat: &str, case_sensitive: bool) -> Self {
        Self {
            pat: pat.to_string(),
            case_sensitive,
        }
    }
    fn is_match(&self, name: &str) -> bool {
        glob_match_simple(&self.pat, name, self.case_sensitive)
    }
}

fn glob_match_simple(pattern: &str, text: &str, case_sensitive: bool) -> bool {
    glob_match_bytes(pattern.as_bytes(), text.as_bytes(), case_sensitive)
}

fn glob_byte_eq(a: u8, b: u8, case_sensitive: bool) -> bool {
    if case_sensitive {
        a == b
    } else {
        a.eq_ignore_ascii_case(&b)
    }
}

fn glob_match_bytes(pat: &[u8], text: &[u8], case_sensitive: bool) -> bool {
    // Classic backtracking matcher for '*' and '?' only
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star_idx, mut match_idx) = (None, 0usize);
    while ti < text.len() {
        if pi < pat.len() && (pat[pi] == b'?' || glob_byte_eq(pat[pi], text[ti], case_sensitive)) {
            pi += 1;
            ti += 1;
        } else if pi < pat.len() && pat[pi] == b'*' {
            star_idx = Some(pi);
            match_idx = ti;
            pi += 1;
        } else if let Some(si) = star_idx {
            pi = si + 1;
            match_idx += 1;
            ti = match_idx;
        } else {
            return false;
        }
    }
    while pi < pat.len() && pat[pi] == b'*' {
        pi += 1;
    }
    pi == pat.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn params(root: PathBuf) -> FindParams {
        FindParams {
            start_dir: root,
            enable_ignore_dirs: false,
            ..FindParams::default()
        }
    }

    fn hits(p: &FindParams) -> Vec<PathBuf> {
        search_files(p, &Arc::new(AtomicBool::new(false)))
    }

    #[test]
    fn test_glob_match() {
        assert!(glob_match_simple("*.rs", "main.rs", true));
        assert!(glob_match_simple("m?in.rs", "main.rs", true));
        assert!(!glob_match_simple("*.rs", "main.c", true));
        assert!(glob_match_simple("*", "anything", true));
        assert!(glob_match_simple("*.txt", "FOO.TXT", false));
        assert!(!glob_match_simple("*.txt", "FOO.TXT", true));
    }

    #[test]
    fn test_search_name_and_content() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let foo = root.join("foo.txt");
        let bar = root.join("bar.log");
        std::fs::write(&foo, "Hello World").unwrap();
        std::fs::write(&bar, "nothing here").unwrap();
        let mut p = params(root.to_path_buf());
        p.name_pattern = NamePattern::Glob("*.txt".into());
        p.content_substring = Some("world".into());
        p.case_sensitive = false;
        p.content_case_sensitive = false;
        let res = hits(&p);
        assert_eq!(res.len(), 1);
        assert_eq!(res[0], foo);
    }

    #[test]
    fn root_dir_not_included_for_star() {
        let dir = tempdir().unwrap();
        let root = dir.path().to_path_buf();
        std::fs::write(root.join("x"), "x").unwrap();
        let p = params(root.clone());
        let found = hits(&p);
        assert!(!found.iter().any(|h| h == &root));
    }

    #[test]
    fn glob_txt_with_regex_off() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let txt = root.join("note.txt");
        let log = root.join("note.log");
        std::fs::write(&txt, "a").unwrap();
        std::fs::write(&log, "b").unwrap();
        let mut p = params(root.to_path_buf());
        p.name_pattern = NamePattern::Glob("*.txt".into());
        p.regular_expression = false;
        let res = hits(&p);
        assert_eq!(res, vec![txt]);
    }

    #[test]
    fn regex_filename_foo_dot_star_txt() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let foo = root.join("foo_bar.txt");
        let other = root.join("x.txt");
        std::fs::write(&foo, "a").unwrap();
        std::fs::write(&other, "b").unwrap();
        let mut p = params(root.to_path_buf());
        p.name_pattern = NamePattern::Glob(r"foo.*\.txt".into());
        p.using_shell_patterns = false;
        let res = hits(&p);
        assert_eq!(res, vec![foo]);
        assert!(!res.contains(&other));
    }

    #[test]
    fn content_substring_vs_regex() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let f = root.join("n.txt");
        std::fs::write(&f, "hello world\n").unwrap();
        let mut p = params(root.to_path_buf());
        p.name_pattern = NamePattern::Glob("*".into());
        p.content_substring = Some("wor.d".into());
        p.regular_expression = false;
        assert!(
            hits(&p).is_empty(),
            "substring must not treat '.' as any char"
        );
        p.name_pattern = NamePattern::Glob(".*".into());
        p.using_shell_patterns = false;
        p.regular_expression = true;
        assert_eq!(hits(&p), vec![f]);
    }

    #[test]
    fn skip_hidden_dot_secret() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let visible = root.join("visible.txt");
        let secret = root.join(".secret");
        std::fs::write(&visible, "a").unwrap();
        std::fs::write(&secret, "b").unwrap();
        let mut p = params(root.to_path_buf());
        p.name_pattern = NamePattern::Glob("*".into());
        p.skip_hidden = true;
        let hidden_on = hits(&p);
        assert!(hidden_on.contains(&visible));
        assert!(!hidden_on.contains(&secret));
        p.skip_hidden = false;
        let hidden_off = hits(&p);
        assert!(hidden_off.contains(&visible));
        assert!(hidden_off.contains(&secret));
    }

    #[test]
    fn skip_hidden_does_not_descend_dot_dir() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let hidden_dir = root.join(".hid");
        std::fs::create_dir(&hidden_dir).unwrap();
        let nested = hidden_dir.join("inside.txt");
        std::fs::write(&nested, "x").unwrap();
        let mut p = params(root.to_path_buf());
        p.name_pattern = NamePattern::Glob("*.txt".into());
        p.skip_hidden = true;
        assert!(!hits(&p).contains(&nested));
        p.skip_hidden = false;
        assert!(hits(&p).contains(&nested));
    }

    #[test]
    fn follow_symlinks_descends_symlink_dir() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let start = root.join("start");
        let target = root.join("target");
        std::fs::create_dir(&start).unwrap();
        std::fs::create_dir(&target).unwrap();
        let child = start.join("child.txt");
        let inside = target.join("inside.txt");
        std::fs::write(&child, "c").unwrap();
        std::fs::write(&inside, "i").unwrap();
        let link = start.join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let mut p = params(start.clone());
        p.name_pattern = NamePattern::Glob("*.txt".into());
        p.follow_symlinks = false;
        let off = hits(&p);
        assert!(off.contains(&child));
        assert!(!off.iter().any(|h| h.ends_with("inside.txt")));

        p.follow_symlinks = true;
        let on = hits(&p);
        assert!(on.contains(&child));
        assert!(
            on.iter().any(|h| h.ends_with("inside.txt")),
            "symlink-to-dir must be descended when Follow symlinks is on: {on:?}"
        );
    }

    #[test]
    fn recursively_off_skips_nested() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let child = root.join("child.txt");
        let sub = root.join("sub");
        std::fs::create_dir(&sub).unwrap();
        let nested = sub.join("nested.txt");
        std::fs::write(&child, "c").unwrap();
        std::fs::write(&nested, "n").unwrap();
        let mut p = params(root.to_path_buf());
        p.name_pattern = NamePattern::Glob("*.txt".into());
        p.find_recursively = false;
        let res = hits(&p);
        assert!(res.contains(&child));
        assert!(!res.contains(&nested));
        p.find_recursively = true;
        let rec = hits(&p);
        assert!(rec.contains(&child));
        assert!(rec.contains(&nested));
    }

    #[test]
    fn invalid_filename_regex_yields_zero_hits() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("foo.txt"), "x").unwrap();
        let mut p = params(root.to_path_buf());
        p.name_pattern = NamePattern::Glob("*".into());
        p.using_shell_patterns = false;
        assert!(hits(&p).is_empty());
    }

    #[test]
    fn whole_words_skips_category() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let category = root.join("category.txt");
        let cat = root.join("cat.txt");
        std::fs::write(&category, "category\n").unwrap();
        std::fs::write(&cat, "a cat here\n").unwrap();
        let mut p = params(root.to_path_buf());
        p.content_substring = Some("cat".into());
        p.whole_words = true;
        let res = hits(&p);
        assert_eq!(res, vec![cat]);
        assert!(!res.contains(&category));
    }

    #[test]
    fn whole_words_off_hits_category() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let category = root.join("category.txt");
        let cat = root.join("cat.txt");
        std::fs::write(&category, "category\n").unwrap();
        std::fs::write(&cat, "a cat here\n").unwrap();
        let mut p = params(root.to_path_buf());
        p.content_substring = Some("cat".into());
        p.whole_words = false;
        let res = hits(&p);
        assert!(
            res.contains(&category),
            "substring cat must hit category: {res:?}"
        );
        assert!(
            res.contains(&cat),
            "substring cat must hit cat.txt: {res:?}"
        );
    }

    #[test]
    fn ignore_dirs_relative_git() {
        let dir = tempdir().unwrap();
        let start = dir.path().join("start");
        std::fs::create_dir(&start).unwrap();
        let keep = start.join("keep.txt");
        std::fs::write(&keep, "k").unwrap();
        let git = start.join(".git");
        std::fs::create_dir(&git).unwrap();
        std::fs::write(git.join("HEAD"), "ref").unwrap();
        std::fs::create_dir(git.join("objects")).unwrap();
        std::fs::write(git.join("objects").join("x"), "blob").unwrap();

        let mut p = params(start);
        p.enable_ignore_dirs = true;
        p.ignore_dirs = ".git".into();
        let res = hits(&p);
        assert_eq!(res, vec![keep]);
        assert!(
            !res.iter()
                .any(|h| h.components().any(|c| c.as_os_str() == ".git")),
            "must not report hits under .git: {res:?}"
        );
    }

    #[test]
    fn ignore_dirs_colon_list() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let git = root.join(".git");
        let cvs = root.join("CVS");
        let visible = root.join("visible");
        std::fs::create_dir(&git).unwrap();
        std::fs::create_dir(&cvs).unwrap();
        std::fs::create_dir(&visible).unwrap();
        std::fs::write(git.join("HEAD"), "ref").unwrap();
        std::fs::write(cvs.join("Entries"), "e").unwrap();
        let kept = visible.join("file.txt");
        std::fs::write(&kept, "v").unwrap();

        let mut p = params(root.to_path_buf());
        p.enable_ignore_dirs = true;
        p.ignore_dirs = ".git:CVS".into();
        let res = hits(&p);
        assert!(
            res.contains(&kept),
            "file under visible/ must be kept: {res:?}"
        );
        assert!(
            !res.iter()
                .any(|h| h.starts_with(&git) || h.starts_with(&cvs)),
            ".git and CVS must both be skipped: {res:?}"
        );
    }

    #[test]
    fn ignore_dirs_checkbox_off_does_not_skip() {
        let dir = tempdir().unwrap();
        let start = dir.path().join("start");
        std::fs::create_dir(&start).unwrap();
        let keep = start.join("keep.txt");
        std::fs::write(&keep, "k").unwrap();
        let git = start.join(".git");
        std::fs::create_dir(&git).unwrap();
        let head = git.join("HEAD");
        std::fs::write(&head, "ref").unwrap();

        let mut p = params(start);
        p.enable_ignore_dirs = false;
        p.ignore_dirs = ".git".into();
        let res = hits(&p);
        assert!(res.contains(&keep));
        assert!(
            res.contains(&head),
            "ignore list must be unused when the checkbox is off: {res:?}"
        );
    }

    #[test]
    fn ignore_dirs_dot_means_start_dir() {
        // Man page: a lone "." in the ignore list is the current absolute start
        // directory. Skipping that path does not descend, so the walk yields no hits.
        let dir = tempdir().unwrap();
        let start = dir.path().join("start");
        std::fs::create_dir(&start).unwrap();
        std::fs::write(start.join("keep.txt"), "k").unwrap();

        let mut p = params(start);
        p.enable_ignore_dirs = true;
        p.ignore_dirs = ".".into();
        assert!(
            hits(&p).is_empty(),
            "ignoring start_dir via '.' must skip the whole walk"
        );
    }

    #[test]
    fn find_params_deserialize_defaults_new_fields() {
        let json = r#"{
            "start_dir": "/tmp",
            "name_pattern": {"Glob": "*"},
            "content_substring": null,
            "case_sensitive": false,
            "regular_expression": false,
            "find_recursively": true,
            "follow_symlinks": false,
            "skip_hidden": false
        }"#;
        let p: FindParams = serde_json::from_str(json).unwrap();
        assert!(!p.whole_words);
        assert!(p.enable_ignore_dirs);
        assert!(p.ignore_dirs.is_empty());
        assert!(p.using_shell_patterns);
        assert!(p.content_case_sensitive);
        assert!(!p.file_all_charsets);
        assert!(!p.first_hit);
    }

    #[test]
    fn tree_focus_follows_start_dir_in_cycle() {
        assert_eq!(FindDialogFocus::StartDir.next(), FindDialogFocus::Tree);
        assert_eq!(
            FindDialogFocus::Tree.next(),
            FindDialogFocus::EnableIgnoreDirs
        );
        assert_eq!(
            FindDialogFocus::NamePattern.prev(),
            FindDialogFocus::IgnoreDirs
        );
        assert_eq!(FindDialogFocus::Tree.prev(), FindDialogFocus::StartDir);
        assert!(FindDialogFocus::Tree.is_button());
        assert!(FindDialogFocus::Tree.is_form_widget());
        assert!(!FindDialogFocus::Tree.is_checkbox());
        assert!(!FindDialogFocus::StartDir.is_button());
    }

    #[test]
    fn tree_ancestor_chain_starts_at_root() {
        let chain = find_tree_ancestor_chain(Path::new("/tmp/a/b"));
        assert_eq!(
            chain,
            vec![
                PathBuf::from("/"),
                PathBuf::from("/tmp"),
                PathBuf::from("/tmp/a"),
                PathBuf::from("/tmp/a/b"),
            ]
        );
        assert_eq!(
            find_tree_ancestor_chain(Path::new("/")),
            vec![PathBuf::from("/")]
        );
    }

    #[test]
    fn action_buttons_tab_in_gnu_order() {
        use FindDialogFocus as F;
        use FindDialogPhase as P;
        assert_eq!(F::IgnoreDirs.next_setup(), F::NamePattern);
        assert_eq!(F::FirstHit.next_setup(), F::ButtonOk);
        assert_eq!(F::ButtonOk.next_setup(), F::ButtonCancel);
        assert_eq!(F::ButtonCancel.next_setup(), F::StartDir);
        assert_eq!(F::ButtonOk.prev_setup(), F::FirstHit);
        assert_eq!(F::ResultsList.next_in(P::Results), F::ButtonChdir);
        assert_eq!(F::ButtonChdir.next_in(P::Results), F::ButtonAgain);
        assert_eq!(F::ButtonAgain.next_in(P::Results), F::ButtonSuspend);
        assert_eq!(F::ButtonSuspend.next_in(P::Results), F::ButtonQuit);
        assert_eq!(F::ButtonQuit.next_in(P::Results), F::ButtonPanelize);
        assert_eq!(F::ButtonPanelize.next_in(P::Results), F::ButtonView);
        assert_eq!(F::ButtonView.next_in(P::Results), F::ButtonEdit);
        assert_eq!(F::ButtonEdit.next_in(P::Results), F::ResultsList);
        assert_eq!(F::FindRecursively.setup_across(), F::WholeWords);
        assert_eq!(F::NamePattern.setup_down(), F::FindRecursively);
        assert_eq!(F::Content.setup_down(), F::WholeWords);
        assert!(F::ButtonOk.is_action_button());
        assert!(F::ButtonCancel.is_action_button());
        assert!(F::ButtonSuspend.is_action_button());
        assert!(F::ButtonView.is_action_button());
        assert!(F::ButtonEdit.is_action_button());
        assert!(!F::ResultsList.is_action_button());
        assert!(!F::Tree.is_action_button());
    }

    #[test]
    fn results_size_matches_live_gnu_80x24_and_grows() {
        assert_eq!(find_results_size_tuple(80, 24), ((9, 3), (62, 18)));
        assert_eq!(find_results_size_tuple(100, 30), ((9, 3), (82, 24)));
    }

    fn find_results_size_tuple(cols: u16, rows: u16) -> ((u16, u16), (u16, u16)) {
        (
            find_results_origin(cols, rows),
            (find_results_width(cols), find_results_height(rows)),
        )
    }

    #[test]
    fn display_rows_group_by_parent() {
        let rows = find_display_rows(&[
            PathBuf::from("/tmp/a/one.txt"),
            PathBuf::from("/tmp/a/two.txt"),
            PathBuf::from("/tmp/b/three.txt"),
        ]);
        assert_eq!(rows[0], FindDisplayRow::Header(PathBuf::from("/tmp/a")));
        assert_eq!(rows[1].display_text(), "    one.txt");
        assert_eq!(rows[2].display_text(), "    two.txt");
        assert_eq!(rows[3], FindDisplayRow::Header(PathBuf::from("/tmp/b")));
        assert_eq!(rows[4].display_text(), "    three.txt");
        assert_eq!(find_display_index_for_hit(&rows, 2), 4);
    }

    #[test]
    fn search_pause_blocks_until_resume() {
        let dir = tempdir().unwrap();
        let hit = dir.path().join("a.txt");
        std::fs::write(&hit, "x").unwrap();
        let p = params(dir.path().to_path_buf());
        let handle = CancelHandle::new();
        handle.pause();
        let (tx, rx) = std::sync::mpsc::channel();
        let flag = handle.flag();
        let pause = handle.pause_flag();
        let worker = std::thread::spawn(move || {
            search_files_streaming(&p, &flag, &pause, |path| {
                let _ = tx.send(path);
            });
        });
        std::thread::sleep(std::time::Duration::from_millis(80));
        assert!(
            rx.try_recv().is_err(),
            "paused walk must not emit hits until Start"
        );
        handle.resume();
        let got = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("resume must continue the walk");
        assert_eq!(got, hit);
        worker.join().unwrap();
    }
}
