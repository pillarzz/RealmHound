//! Update notification panel for RealmHound.
//!
//! Displays a notification when a new version is available and shows
//! the changelog for versions between the current and latest.

use crate::ui_ext::HoverTooltipExt;
use eframe::egui::{self, Color32, RichText, Ui};
use realmhound_core::update::{Release, SelfUpdateState, SelfUpdater, UpdateChecker, UpdateState};

use crate::shadcn_ui::Shadcn;

/// Panel for displaying update notifications.
pub struct UpdatePanel {
    /// Whether the details panel is open
    show_details: bool,
    /// Set when the user clicks "Restart to apply update"; the app polls and clears it.
    relaunch_requested: bool,
}

impl UpdatePanel {
    /// Create a new update panel.
    pub fn new() -> Self {
        Self {
            show_details: false,
            relaunch_requested: false,
        }
    }

    /// Take a pending "apply update and relaunch" request, if any.
    pub fn take_relaunch_request(&mut self) -> bool {
        std::mem::take(&mut self.relaunch_requested)
    }

    /// Render the update notification indicator in the toolbar.
    /// Returns true if an update is available (for layout purposes).
    pub fn render_indicator(
        &mut self,
        ui: &mut Ui,
        update_checker: &UpdateChecker,
        shadcn: &Shadcn,
    ) -> bool {
        match update_checker.state() {
            UpdateState::UpdateAvailable { manifest, .. } => {
                // Show update available indicator
                if shadcn
                    .btn(
                        ui,
                        RichText::new(format!("🔔 Update v{}", manifest.latest_version))
                            .color(Color32::from_rgb(255, 200, 100)),
                    )
                    .hover_tip("Click to see what's new")
                    .clicked()
                {
                    self.show_details = !self.show_details;
                }

                true
            }
            UpdateState::Checking => {
                ui.spinner();
                ui.label(RichText::new("Checking...").weak());
                false
            }
            UpdateState::Error(err) => {
                ui.label(RichText::new("⚠").color(Color32::from_rgb(200, 150, 100)))
                    .hover_tip(format!("Update check failed: {}", err.message));
                false
            }
            UpdateState::UpToDate | UpdateState::Unknown => {
                // Don't show anything when up to date or unknown
                false
            }
        }
    }

    /// Render the update details window if open.
    pub fn render_details_window(
        &mut self,
        ctx: &egui::Context,
        update_checker: &UpdateChecker,
        self_updater: &SelfUpdater,
        shadcn: &Shadcn,
    ) {
        if !self.show_details {
            return;
        }

        let (manifest, new_releases) = match update_checker.state() {
            UpdateState::UpdateAvailable {
                manifest,
                new_releases,
            } => (manifest, new_releases),
            _ => {
                // No update available, close the panel
                self.show_details = false;
                return;
            }
        };

        let mut open = self.show_details;

        egui::Area::new(egui::Id::new("update_details_dialog_host"))
            .fixed_pos(egui::pos2(0.0, 0.0))
            .show(ctx, |ui| {
                shadcn.dialog(
                    ui,
                    "update_details_modal",
                    &mut open,
                    "Update Available",
                    420.0,
                    450.0,
                    |ui| {
                        egui::Grid::new("version_info_grid")
                            .num_columns(2)
                            .spacing([20.0, 6.0])
                            .show(ui, |ui| {
                                ui.label("Current version:");
                                ui.label(
                                    RichText::new(format!("v{}", update_checker.current_version()))
                                        .monospace(),
                                );
                                ui.end_row();

                                ui.label("Latest version:");
                                ui.label(
                                    RichText::new(format!("v{}", manifest.latest_version))
                                        .monospace()
                                        .color(Color32::from_rgb(100, 200, 100)),
                                );
                                ui.end_row();
                            });

                        ui.add_space(15.0);
                        shadcn.separator(ui);
                        ui.add_space(10.0);

                        // Changelog section
                        ui.label(RichText::new("What's New:").strong());
                        ui.add_space(5.0);

                        // Scrollable changelog area
                        egui::ScrollArea::vertical()
                            .max_height(200.0)
                            .show(ui, |ui| {
                                for release in new_releases {
                                    self.render_release(ui, release);
                                    ui.add_space(10.0);
                                }
                            });

                        ui.add_space(15.0);
                        shadcn.separator(ui);
                        ui.add_space(10.0);

                        // Action buttons
                        match self_updater.state() {
                            SelfUpdateState::Idle => {
                                if shadcn
                                    .btn(ui, format!("⬇ Update to v{}", manifest.latest_version))
                                    .clicked()
                                {
                                    self_updater.start_download(
                                        manifest.download_url.clone(),
                                        manifest.sha256.clone(),
                                    );
                                }
                            }
                            SelfUpdateState::Downloading(progress) => {
                                ui.horizontal(|ui| {
                                    ui.spinner();
                                    if let Some(total) = progress.total {
                                        let pct = (progress.downloaded as f32 / total as f32
                                            * 100.0)
                                            as u32;
                                        ui.label(format!("Downloading... {}%", pct));
                                    } else {
                                        ui.label("Downloading...");
                                    }
                                });
                            }
                            SelfUpdateState::ReadyToApply => {
                                if shadcn.btn(ui, "✓ Restart to apply update").clicked() {
                                    self.relaunch_requested = true;
                                }
                            }
                            SelfUpdateState::Failed(msg) => {
                                ui.label(
                                    RichText::new(format!("Failed: {}", msg))
                                        .color(Color32::from_rgb(200, 100, 100))
                                        .small(),
                                );
                                if shadcn.btn(ui, "Retry").clicked() {
                                    self_updater.reset();
                                }
                            }
                        }
                    },
                );
            });

        self.show_details = open;
    }

    /// Render a single release entry.
    fn render_release(&self, ui: &mut Ui, release: &Release) {
        // Version header with date
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(format!("v{}", release.version))
                    .strong()
                    .color(Color32::from_rgb(150, 180, 255)),
            );
            ui.label(RichText::new(format!("({})", release.date)).weak().small());
        });

        // Changes as bullet list
        for change in &release.changes {
            ui.label(format!("  • {}", change));
        }
    }

    /// Check if the details panel is open.
    pub fn is_details_open(&self) -> bool {
        self.show_details
    }

    /// Open the details panel.
    pub fn open_details(&mut self) {
        self.show_details = true;
    }
}

impl Default for UpdatePanel {
    fn default() -> Self {
        Self::new()
    }
}
