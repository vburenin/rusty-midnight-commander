use crossterm::style::Color;

#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub struct McPalette {
    pub core_default_fg: Color,
    pub core_default_bg: Color,
    pub selected_fg: Color,
    pub selected_bg: Color,
    pub marked_fg: Color,
    pub marked_bg: Color,
    pub markselect_fg: Color,
    pub markselect_bg: Color,
    pub header_fg: Color,
    pub header_bg: Color,
    pub frame_fg: Color,
    pub frame_bg: Color,
    pub shadow_fg: Color,
    pub shadow_bg: Color,
    pub dialog_default_fg: Color,
    pub dialog_default_bg: Color,
    pub dfocus_fg: Color,
    pub dfocus_bg: Color,
    pub dtitle_fg: Color,
    pub dtitle_bg: Color,
    pub menu_fg: Color,
    pub menu_bg: Color,
    pub menusel_fg: Color,
    pub menusel_bg: Color,
    pub menuhot_fg: Color,
    pub menuhot_bg: Color,
    /// Menu hotkey on the selected item (GNU `menuhotsel=yellow;black`).
    pub menuhotsel_fg: Color,
    pub menuhotsel_bg: Color,
    /// Error dialog default (GNU `[error] _default_=white;red`).
    pub error_default_fg: Color,
    pub error_default_bg: Color,
    /// Error dialog focused widget (GNU `errdfocus=black;lightgray`).
    pub errdfocus_fg: Color,
    pub errdfocus_bg: Color,
    pub buttonbar_hotkey_fg: Color,
    pub buttonbar_hotkey_bg: Color,
    pub buttonbar_button_fg: Color,
    pub buttonbar_button_bg: Color,
    pub statusbar_fg: Color,
    pub statusbar_bg: Color,
    // filehighlight
    pub dir_color: Color,
    pub exec_color: Color,
    pub archive_color: Color,
    pub source_color: Color,
    pub symlink_color: Color,
    /// Viewer pairs from public default.ini `[viewer]`:
    /// `_default_=lightgray;blue`, selected/`viewselected`=yellow;cyan.
    /// Distinct from panel `selected` (black;cyan) and from `[core] _default_`.
    pub viewer_default_fg: Color,
    pub viewer_default_bg: Color,
    pub viewer_selected_fg: Color,
    pub viewer_selected_bg: Color,
    /// Editor default (public GNU `[editor] _default_=lightgray;blue`).
    pub edit_normal_fg: Color,
    pub edit_normal_bg: Color,
    /// Keyword/emphasis pair (public GNU `editbold=yellow;green`).
    pub edit_bold_fg: Color,
    pub edit_bold_bg: Color,
    /// Block selection (public GNU `editmarked=black;cyan`; not panel `marked`).
    pub edit_marked_fg: Color,
    pub edit_marked_bg: Color,
    pub edit_whitespace_fg: Color,
    pub edit_whitespace_bg: Color,
    pub edit_linestate_fg: Color,
    pub edit_linestate_bg: Color,
}

impl McPalette {
    pub fn default() -> Self {
        Self {
            core_default_fg: Color::Grey, // lightgray
            core_default_bg: Color::Blue,
            selected_fg: Color::Black,
            selected_bg: Color::Cyan,
            marked_fg: Color::Yellow,
            marked_bg: Color::Blue,
            markselect_fg: Color::Yellow,
            markselect_bg: Color::Cyan,
            header_fg: Color::Yellow,
            header_bg: Color::Blue,
            frame_fg: Color::Grey,
            frame_bg: Color::Blue,
            shadow_fg: Color::Grey,
            shadow_bg: Color::Black,
            dialog_default_fg: Color::Black,
            dialog_default_bg: Color::Grey, // lightgray
            dfocus_fg: Color::Black,
            dfocus_bg: Color::Cyan,
            dtitle_fg: Color::Blue,
            dtitle_bg: Color::Grey,
            menu_fg: Color::White,
            menu_bg: Color::Cyan,
            menusel_fg: Color::White,
            menusel_bg: Color::Black,
            menuhot_fg: Color::Yellow,
            menuhot_bg: Color::Cyan,
            menuhotsel_fg: Color::Yellow,
            menuhotsel_bg: Color::Black,
            error_default_fg: Color::White,
            error_default_bg: Color::Red,
            errdfocus_fg: Color::Black,
            errdfocus_bg: Color::Grey,
            buttonbar_hotkey_fg: Color::White,
            buttonbar_hotkey_bg: Color::Black,
            buttonbar_button_fg: Color::Black,
            buttonbar_button_bg: Color::Cyan,
            statusbar_fg: Color::Black,
            statusbar_bg: Color::Cyan,
            dir_color: Color::White,
            exec_color: Color::Green,      // brightgreen approximation
            archive_color: Color::Magenta, // brightmagenta approximation
            source_color: Color::Cyan,
            symlink_color: Color::Grey,
            viewer_default_fg: Color::Grey, // lightgray
            viewer_default_bg: Color::Blue,
            viewer_selected_fg: Color::Yellow,
            viewer_selected_bg: Color::Cyan,
            edit_normal_fg: Color::Grey,
            edit_normal_bg: Color::Blue,
            edit_bold_fg: Color::Yellow,
            edit_bold_bg: Color::Green,
            edit_marked_fg: Color::Black,
            edit_marked_bg: Color::Cyan,
            // Public `brightblue;blue`. Crossterm binds `blue` to Color::Blue already,
            // so brightblue uses Cyan to stay visible on the editor background.
            edit_whitespace_fg: Color::Cyan,
            edit_whitespace_bg: Color::Blue,
            edit_linestate_fg: Color::White,
            edit_linestate_bg: Color::Cyan,
        }
    }
}
