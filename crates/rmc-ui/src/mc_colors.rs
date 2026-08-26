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
        }
    }
}
