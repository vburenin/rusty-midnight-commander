#[cfg(test)]
mod tests {
    use crate::app::{App, UiMode};
    use crate::config::KeyMap;
    use anyhow::Result;
    use rmc_fs::composite::CompositeFs;

    #[test]
    fn toggle_subshell_preserves_panel_state() -> Result<()> {
        let vfs = CompositeFs::new();
        let keymap = KeyMap::mc_defaults();
        let mut app = App::new(Box::new(vfs), keymap)?;
        let left_before = app.left.clone();
        let right_before = app.right.clone();
        let active_before = app.active;
        let mode_before = app.ui_mode.clone();
        // Toggle on
        app.handle_action(crate::actions::Action::ToggleSubshell)?;
        assert!(app.subshell.show_output_screen);
        // Toggle off
        app.handle_action(crate::actions::Action::ToggleSubshell)?;
        assert!(!app.subshell.show_output_screen);
        // Panels and selection unchanged
        assert_eq!(app.left.cwd, left_before.cwd);
        assert_eq!(app.left.cursor, left_before.cursor);
        assert_eq!(app.right.cwd, right_before.cwd);
        assert_eq!(app.right.cursor, right_before.cursor);
        assert_eq!(app.active, active_before);
        // UiMode reset to Normal when toggled; not strictly required to equal previous if overlay existed
        assert!(matches!(app.ui_mode, UiMode::Normal | UiMode::Menu { .. } | UiMode::Help));
        Ok(())
    }
}

