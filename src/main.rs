// No console window behind the app on Windows.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod backend;
mod generate;
mod typer;

use std::time::Duration;

use eframe::egui;
use generate::{KeyFormat, PasswordOptions};
use typer::{Job, Phase, Typer};

const SETTINGS_KEY: &str = "settings";
const SPEEDS: [(&str, u32); 4] = [
    ("Fast", 5),
    ("Normal", 20),
    ("Slow", 60),
    ("Very slow", 150),
];
const KEY_SIZES: [usize; 5] = [16, 24, 32, 48, 64];

/// Everything remembered between runs. The text itself never is.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct Settings {
    key_delay_ms: u32,
    start_delay_ms: u32,
    line_delay_ms: u32,
    password: PasswordOptions,
    guid_uppercase: bool,
    key_bytes: usize,
    key_format: KeyFormat,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            key_delay_ms: 20,
            start_delay_ms: 3000,
            line_delay_ms: 0,
            password: PasswordOptions::default(),
            guid_uppercase: false,
            key_bytes: 32,
            key_format: KeyFormat::Hex,
        }
    }
}

struct App {
    settings: Settings,
    text: String,
    generated: String,
    generated_note: String,
    typer: Typer,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let settings = cc
            .storage
            .and_then(|storage| eframe::get_value(storage, SETTINGS_KEY))
            .unwrap_or_default();
        let ctx = cc.egui_ctx.clone();
        Self {
            settings,
            text: String::new(),
            generated: String::new(),
            generated_note: String::new(),
            typer: Typer::spawn(move || ctx.request_repaint()),
        }
    }

    fn send(&self) {
        self.typer.start(Job {
            text: self.text.clone(),
            key_delay: Duration::from_millis(self.settings.key_delay_ms.into()),
            line_delay: Duration::from_millis(self.settings.line_delay_ms.into()),
            start_delay: Duration::from_millis(self.settings.start_delay_ms.into()),
        });
    }

    fn timing_ui(&mut self, ui: &mut egui::Ui) {
        let s = &mut self.settings;
        egui::Grid::new("timing").num_columns(2).show(ui, |ui| {
            ui.label("Typing speed");
            ui.horizontal(|ui| {
                let current = SPEEDS
                    .iter()
                    .find(|(_, ms)| *ms == s.key_delay_ms)
                    .map_or("Custom", |(name, _)| name);
                egui::ComboBox::from_id_salt("speed")
                    .selected_text(current)
                    .show_ui(ui, |ui| {
                        for (name, ms) in SPEEDS {
                            ui.selectable_value(&mut s.key_delay_ms, ms, name);
                        }
                    });
                ui.add(
                    egui::DragValue::new(&mut s.key_delay_ms)
                        .range(0..=2000)
                        .suffix(" ms per key"),
                );
            });
            ui.end_row();

            ui.label("Wait before typing");
            ui.add(
                egui::DragValue::new(&mut s.start_delay_ms)
                    .range(0..=120_000)
                    .speed(25)
                    .suffix(" ms"),
            )
            .on_hover_text("Time to click into the window that should receive the text.");
            ui.end_row();

            ui.label("Pause after Enter");
            ui.add(
                egui::DragValue::new(&mut s.line_delay_ms)
                    .range(0..=10_000)
                    .suffix(" ms"),
            )
            .on_hover_text("Extra wait after each line, for consoles that are slow to accept one.");
            ui.end_row();
        });
    }

    fn generators_ui(&mut self, ui: &mut egui::Ui) {
        let s = &mut self.settings;
        let mut made: Option<(String, String)> = None;
        egui::Grid::new("generators").num_columns(2).show(ui, |ui| {
            ui.label("Password");
            ui.horizontal_wrapped(|ui| {
                ui.add(
                    egui::DragValue::new(&mut s.password.length)
                        .range(PasswordOptions::MIN_LENGTH..=PasswordOptions::MAX_LENGTH)
                        .suffix(" chars"),
                );
                ui.checkbox(&mut s.password.lower, "a-z");
                ui.checkbox(&mut s.password.upper, "A-Z");
                ui.checkbox(&mut s.password.digits, "0-9");
                ui.checkbox(&mut s.password.symbols, "!@#");
                ui.checkbox(&mut s.password.avoid_look_alikes, "No look-alikes")
                    .on_hover_text("Leaves out I, l, 1, |, O, 0 and o.");
                if ui.button("Generate").clicked() {
                    made = Some(match generate::password(&s.password) {
                        Some(password) => (
                            password,
                            format!("about {:.0} bits", s.password.entropy_bits()),
                        ),
                        None => (String::new(), "Pick at least one character set.".into()),
                    });
                }
            });
            ui.end_row();

            ui.label("GUID");
            ui.horizontal(|ui| {
                ui.checkbox(&mut s.guid_uppercase, "Uppercase");
                if ui.button("Generate").clicked() {
                    made = Some((generate::guid(s.guid_uppercase), "122 random bits".into()));
                }
            });
            ui.end_row();

            ui.label("Random key");
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("key_bytes")
                    .selected_text(format!("{} bytes", s.key_bytes))
                    .show_ui(ui, |ui| {
                        for bytes in KEY_SIZES {
                            ui.selectable_value(&mut s.key_bytes, bytes, format!("{bytes} bytes"));
                        }
                    });
                ui.radio_value(&mut s.key_format, KeyFormat::Hex, "Hex");
                ui.radio_value(&mut s.key_format, KeyFormat::Base64, "Base64");
                if ui.button("Generate").clicked() {
                    made = Some((
                        generate::random_key(s.key_bytes, s.key_format),
                        format!("{} bits", s.key_bytes * 8),
                    ));
                }
            });
            ui.end_row();
        });
        if let Some((value, note)) = made {
            self.generated = value;
            self.generated_note = note;
        }

        ui.horizontal(|ui| {
            let has_value = !self.generated.is_empty();
            if ui
                .add_enabled(has_value, egui::Button::new("Add to text"))
                .clicked()
            {
                self.text.push_str(&self.generated);
            }
            if ui.add_enabled(has_value, egui::Button::new("Copy")).clicked() {
                ui.ctx().copy_text(self.generated.clone());
            }
            ui.weak(&self.generated_note);
        });
        if !self.generated.is_empty() {
            ui.add(egui::Label::new(egui::RichText::new(&self.generated).monospace()).wrap());
        }
    }

    fn status_ui(&self, ui: &mut egui::Ui, phase: &Phase) {
        match phase {
            Phase::Idle => {
                ui.weak(format!("Ready ({})", self.typer.backend_name()));
            }
            Phase::Preparing => {
                ui.label("Waiting for permission to send keystrokes…");
            }
            Phase::Countdown { left } => {
                ui.strong(format!(
                    "Click into the target window. Typing starts in {:.1} s…",
                    left.as_secs_f32()
                ));
            }
            Phase::Typing { done, total } => {
                ui.add(
                    egui::ProgressBar::new(*done as f32 / (*total).max(1) as f32)
                        .text(format!("Typing {done} of {total}")),
                );
            }
            Phase::Finished { typed, off_layout } => {
                ui.label(format!("Done: typed {typed} characters."));
                if !off_layout.is_empty() {
                    let shown: String = off_layout.iter().take(12).collect();
                    let more = if off_layout.len() > 12 { "…" } else { "" };
                    ui.colored_label(
                        ui.visuals().warn_fg_color,
                        format!(
                            "Not on your active keyboard layout: {shown}{more} \
                             Browser-based consoles may have missed these."
                        ),
                    );
                }
            }
            Phase::Stopped { typed, total } => {
                ui.label(format!("Stopped after {typed} of {total} characters."));
            }
            Phase::Failed { typed, message } => {
                let color = ui.visuals().error_fg_color;
                ui.colored_label(color, format!("{message} (typed {typed} characters)"));
            }
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let phase = self.typer.phase();
        let busy = phase.is_busy();
        if busy && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.typer.stop();
        }

        egui::Panel::bottom("controls").show(ui, |ui| {
            ui.add_space(6.0);
            ui.add_enabled_ui(!busy, |ui| {
                self.timing_ui(ui);
                egui::CollapsingHeader::new("Generators")
                    .default_open(true)
                    .show(ui, |ui| self.generators_ui(ui));
            });
            ui.separator();
            ui.horizontal(|ui| {
                let can_send = !busy && !self.text.is_empty();
                if ui
                    .add_enabled(can_send, egui::Button::new("Send"))
                    .clicked()
                {
                    self.send();
                }
                if ui.add_enabled(busy, egui::Button::new("Stop (Esc)")).clicked() {
                    self.typer.stop();
                }
                if ui
                    .add_enabled(can_send, egui::Button::new("Clear"))
                    .clicked()
                {
                    self.text.clear();
                }
                if ui
                    .add_enabled(!busy, egui::Button::new("Defaults"))
                    .on_hover_text("Reset the speed, delays and generator options.")
                    .clicked()
                {
                    self.settings = Settings::default();
                }
            });
            ui.horizontal_wrapped(|ui| self.status_ui(ui, &phase));
            ui.add_space(6.0);
        });

        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.add_enabled_ui(!busy, |ui| {
                    ui.add_sized(
                        ui.available_size(),
                        egui::TextEdit::multiline(&mut self.text)
                            .code_editor()
                            .hint_text("Text to type into the other window"),
                    );
                });
            });
        });
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, SETTINGS_KEY, &self.settings);
    }
}

fn main() -> eframe::Result {
    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png"))
        .expect("the bundled icon is a valid PNG");
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_app_id("sendkeys")
            .with_icon(icon)
            .with_inner_size([600.0, 640.0])
            .with_min_inner_size([420.0, 360.0]),
        ..Default::default()
    };
    eframe::run_native(
        "SendKeys",
        options,
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    )
}
