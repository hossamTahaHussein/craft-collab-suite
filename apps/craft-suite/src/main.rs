//! `craft-suite [FILE]`: the Craft Collab Suite start window. New document, workbook or
//! presentation, or Open… a file: each opens in its editor's own window. With a file it opens
//! it at once, in the editor its extension names.

use craft_suite::{Editor, editor_for, open_command};

struct Start {
    message: Option<String>,
}

impl Start {
    fn open(&mut self, editor: Editor, file: Option<&std::path::Path>) {
        self.message = match open_command(editor, file).spawn() {
            Ok(_) => None,
            Err(e) => Some(format!("Couldn't start {}: {e}", editor.name())),
        };
    }
}

impl eframe::App for Start {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(24.0);
                ui.heading("Craft Collab Suite");
                ui.label("Craft Docs, Craft Sheets and Craft Slides, by Hossam Taha");
                ui.add_space(20.0);
                ui.horizontal(|ui| {
                    for editor in Editor::ALL {
                        let tile = egui::Button::new(egui::RichText::new(format!("{}\n{}", editor.new_label(), editor.name())).size(16.0))
                            .min_size(egui::vec2(170.0, 90.0));
                        if ui.add(tile).clicked() {
                            self.open(editor, None);
                        }
                    }
                });
                ui.add_space(16.0);
                if ui.button("Open…").clicked() {
                    let all: Vec<&str> = Editor::ALL.iter().flat_map(|e| e.extensions().iter().copied()).collect();
                    if let Some(file) = rfd::FileDialog::new().add_filter("Documents, workbooks and presentations", &all).pick_file() {
                        match editor_for(&file) {
                            Some(editor) => self.open(editor, Some(&file)),
                            None => self.message = Some(format!("No Craft editor opens {}", file.display())),
                        }
                    }
                }
                if let Some(m) = &self.message {
                    ui.add_space(8.0);
                    ui.colored_label(ui.visuals().error_fg_color, m);
                }
            });
        });
    }
}

fn main() -> eframe::Result<()> {
    if let Some(file) = std::env::args_os().nth(1).map(std::path::PathBuf::from) {
        let Some(editor) = editor_for(&file) else {
            eprintln!("craft-suite: no Craft editor opens {}", file.display());
            std::process::exit(2);
        };
        if let Err(e) = open_command(editor, Some(&file)).spawn() {
            eprintln!("craft-suite: couldn't start {}: {e}", editor.name());
            std::process::exit(1);
        }
        return Ok(());
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([620.0, 320.0]).with_title("Craft Collab Suite"),
        ..Default::default()
    };
    eframe::run_native("Craft Collab Suite", options, Box::new(|_| Ok(Box::new(Start { message: None }))))
}
