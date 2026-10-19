use eframe::egui;
use onda_run::RunThemeMode;

use crate::{apply_run_theme, configure_run_theme, RunTheme};

#[test]
fn forced_theme_survives_system_theme_reports() {
    for (mode, expected) in [
        (RunThemeMode::Dark, egui::Theme::Dark),
        (RunThemeMode::Light, egui::Theme::Light),
    ] {
        let ctx = egui::Context::default();
        configure_run_theme(&ctx, mode);
        for system_theme in [egui::Theme::Light, egui::Theme::Dark, egui::Theme::Light] {
            let _ = ctx.run(
                egui::RawInput {
                    system_theme: Some(system_theme),
                    ..Default::default()
                },
                |ctx| {
                    let theme = RunTheme::from_dark_mode(ctx.style().visuals.dark_mode);
                    apply_run_theme(ctx, &theme);
                    assert_eq!(
                        ctx.style().visuals.dark_mode,
                        expected == egui::Theme::Dark,
                        "{mode:?} with system theme {system_theme:?}"
                    );
                    assert_eq!(ctx.theme(), expected);
                },
            );
        }
    }
}

#[test]
fn auto_theme_follows_system_after_a_saved_preference() {
    let ctx = egui::Context::default();
    ctx.set_theme(egui::Theme::Light);
    configure_run_theme(&ctx, RunThemeMode::Auto);
    for system_theme in [egui::Theme::Dark, egui::Theme::Light, egui::Theme::Dark] {
        let _ = ctx.run(
            egui::RawInput {
                system_theme: Some(system_theme),
                ..Default::default()
            },
            |ctx| {
                assert_eq!(ctx.theme(), system_theme);
                assert_eq!(
                    ctx.style().visuals.dark_mode,
                    system_theme == egui::Theme::Dark
                );
            },
        );
    }
}
