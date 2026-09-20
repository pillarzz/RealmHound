/// UI modal/dialog visibility state.
///
/// Groups all boolean flags and enum state that control which modals
/// and dialogs are currently visible. Extracted from `RealmHoundApp`
/// to reduce field count on the god object.

/// Settings category in the settings modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SettingsCategory {
    #[default]
    LiveFeed,
    Loot,
    Sound,
    Account,
    Treasury,
    Appearance,
    Party,
    TrophyHall,
    CombatHistory,
    WidgetBar,
    Taskbar,
}

impl SettingsCategory {
    /// Stable string key used as the shadcn tab id.
    pub fn to_key(self) -> &'static str {
        match self {
            SettingsCategory::Loot => "loot",
            SettingsCategory::Sound => "sound",
            SettingsCategory::LiveFeed => "live_feed",
            SettingsCategory::Account => "account",
            SettingsCategory::Treasury => "treasury",
            SettingsCategory::Appearance => "appearance",
            SettingsCategory::Party => "party",
            SettingsCategory::TrophyHall => "trophy_hall",
            SettingsCategory::CombatHistory => "combat_history",
            SettingsCategory::WidgetBar => "widget_bar",
            SettingsCategory::Taskbar => "taskbar",
        }
    }

    /// Parse a tab key back into a category, falling back to the default.
    pub fn from_key(key: &str) -> Self {
        match key {
            "sound" => SettingsCategory::Sound,
            "live_feed" => SettingsCategory::LiveFeed,
            "account" => SettingsCategory::Account,
            "treasury" => SettingsCategory::Treasury,
            "appearance" => SettingsCategory::Appearance,
            "party" => SettingsCategory::Party,
            "trophy_hall" => SettingsCategory::TrophyHall,
            "combat_history" => SettingsCategory::CombatHistory,
            "widget_bar" => SettingsCategory::WidgetBar,
            "taskbar" => SettingsCategory::Taskbar,
            _ => SettingsCategory::Loot,
        }
    }
}

/// Tracks which modal dialogs are open and their sub-state.
#[derive(Debug, Default)]
pub struct ModalState {
    /// Whether to show the About window.
    pub show_about: bool,
    /// Whether to show the Settings modal.
    pub show_settings: bool,
    /// Active settings category within the settings modal.
    pub settings_category: SettingsCategory,
    /// Whether to show the exit confirmation dialog.
    pub show_exit_confirm: bool,
    /// Whether exit has been confirmed (user clicked "Yes").
    pub exit_confirmed: bool,
    /// Whether to show the account reset confirmation dialog.
    pub show_account_reset_confirm: bool,
    /// Whether to show the "delete all combat history" confirmation (inline in
    /// the Combat History settings tab).
    pub show_combat_clear_confirm: bool,
    /// Whether to show the full changelog window.
    pub show_changelog: bool,
}

#[allow(dead_code)]
impl ModalState {
    /// Create a new `ModalState` with all dialogs closed.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns `true` if any blocking modal is open.
    ///
    /// This is used to disable interaction with the main content area
    /// while a modal is displayed. Note: the update details panel is
    /// managed separately by `UpdatePanel` and must be checked independently.
    pub fn any_blocking_open(&self) -> bool {
        self.show_about || self.show_exit_confirm || self.show_settings || self.show_changelog
    }

    /// Close all modals and reset transient state.
    ///
    /// This does NOT reset `exit_confirmed` -- that flag is only
    /// meaningful during the exit flow and is handled separately.
    pub fn close_all(&mut self) {
        self.show_about = false;
        self.show_settings = false;
        self.show_exit_confirm = false;
        self.show_account_reset_confirm = false;
        self.show_combat_clear_confirm = false;
        self.show_changelog = false;
        self.settings_category = SettingsCategory::default();
    }

    /// Open the About dialog, closing other modals.
    pub fn open_about(&mut self) {
        self.close_all();
        self.show_about = true;
    }

    /// Open the Settings modal, closing other modals.
    pub fn open_settings(&mut self) {
        self.close_all();
        self.show_settings = true;
    }

    /// Request exit confirmation.
    pub fn request_exit(&mut self) {
        self.show_exit_confirm = true;
    }

    /// Confirm exit (user clicked "Yes" in the exit dialog).
    pub fn confirm_exit(&mut self) {
        self.exit_confirmed = true;
        self.show_exit_confirm = false;
    }

    /// Cancel exit (user clicked "Cancel" or closed the exit dialog).
    pub fn cancel_exit(&mut self) {
        self.show_exit_confirm = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_state_has_nothing_open() {
        let state = ModalState::new();
        assert!(!state.any_blocking_open());
        assert!(!state.show_about);
        assert!(!state.show_settings);
        assert!(!state.show_exit_confirm);
        assert!(!state.exit_confirmed);
        assert!(!state.show_account_reset_confirm);
        assert!(!state.show_changelog);
        assert_eq!(state.settings_category, SettingsCategory::LiveFeed);
    }

    #[test]
    fn any_blocking_open_detects_about() {
        let mut state = ModalState::new();
        assert!(!state.any_blocking_open());
        state.show_about = true;
        assert!(state.any_blocking_open());
    }

    #[test]
    fn any_blocking_open_detects_settings() {
        let mut state = ModalState::new();
        state.show_settings = true;
        assert!(state.any_blocking_open());
    }

    #[test]
    fn any_blocking_open_detects_exit_confirm() {
        let mut state = ModalState::new();
        state.show_exit_confirm = true;
        assert!(state.any_blocking_open());
    }

    #[test]
    fn any_blocking_open_detects_changelog() {
        let mut state = ModalState::new();
        state.show_changelog = true;
        assert!(state.any_blocking_open());
    }

    #[test]
    fn account_reset_confirm_is_not_blocking() {
        // Account reset is an inline confirmation, not a modal overlay
        let mut state = ModalState::new();
        state.show_account_reset_confirm = true;
        assert!(!state.any_blocking_open());
    }

    #[test]
    fn close_all_resets_dialogs() {
        let mut state = ModalState::new();
        state.show_about = true;
        state.show_settings = true;
        state.show_exit_confirm = true;
        state.show_account_reset_confirm = true;
        state.show_changelog = true;
        state.settings_category = SettingsCategory::Account;

        state.close_all();

        assert!(!state.show_about);
        assert!(!state.show_settings);
        assert!(!state.show_exit_confirm);
        assert!(!state.show_account_reset_confirm);
        assert!(!state.show_changelog);
        assert_eq!(state.settings_category, SettingsCategory::LiveFeed);
    }

    #[test]
    fn close_all_preserves_exit_confirmed() {
        let mut state = ModalState::new();
        state.exit_confirmed = true;
        state.close_all();
        // exit_confirmed is NOT reset by close_all
        assert!(state.exit_confirmed);
    }

    #[test]
    fn open_about_closes_others() {
        let mut state = ModalState::new();
        state.show_settings = true;
        state.show_account_reset_confirm = true;

        state.open_about();

        assert!(state.show_about);
        assert!(!state.show_settings);
        assert!(!state.show_account_reset_confirm);
    }

    #[test]
    fn open_settings_closes_others() {
        let mut state = ModalState::new();
        state.show_about = true;

        state.open_settings();

        assert!(!state.show_about);
        assert!(state.show_settings);
    }

    #[test]
    fn exit_flow_request_then_confirm() {
        let mut state = ModalState::new();

        state.request_exit();
        assert!(state.show_exit_confirm);
        assert!(!state.exit_confirmed);

        state.confirm_exit();
        assert!(!state.show_exit_confirm);
        assert!(state.exit_confirmed);
    }

    #[test]
    fn exit_flow_request_then_cancel() {
        let mut state = ModalState::new();

        state.request_exit();
        assert!(state.show_exit_confirm);

        state.cancel_exit();
        assert!(!state.show_exit_confirm);
        assert!(!state.exit_confirmed);
    }
}
