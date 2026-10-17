//! Desktop front-end, modelled on the original wxPython Levelator window:
//! drop audio files onto the window (or double-click / Ctrl+O to browse);
//! each is levelled in turn and written next to the source as `name.output.ext`.
//! Unlike the original it also takes FLAC/MP3/M4A/Ogg, can reduce background
//! noise first, and can convert the output format.
//! Files given on the command line are processed and the app then quits
//! (the original "batch mode").

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;

use eframe::egui;
use levelator_core::audio::{Container, READABLE_EXTENSIONS};
use levelator_core::denoise::Strength;
use levelator_core::{level_file, output_path_for, Settings};

const DRAG_DROP: &str = "Drag and drop audio files here.";
const TITLE: &str = "The Levelator";

enum Job {
    File(PathBuf, Box<Settings>),
    /// Sent after the command-line files: quit once everything before it is done.
    BatchQuit,
}

enum Event {
    Progress { name: String, percent: u32 },
    Done,
    Failed { message: String },
    BatchQuit,
}

struct LevelatorApp {
    jobs: Sender<Job>,
    events: Receiver<Event>,
    cancel: Arc<AtomicBool>,
    status: String,
    percent: Option<u32>,
    complete: bool,
    pending: usize,
    message: Option<String>,
    last_dir: Option<PathBuf>,
    noise_reduction: Option<Strength>,
    output_format: Option<Container>,
}

impl LevelatorApp {
    fn new(ctx: egui::Context, initial: Vec<PathBuf>) -> Self {
        let (job_tx, job_rx) = mpsc::channel::<Job>();
        let (ev_tx, ev_rx) = mpsc::channel::<Event>();
        let cancel = Arc::new(AtomicBool::new(false));
        spawn_worker(job_rx, ev_tx, ctx, cancel.clone());

        let mut app = LevelatorApp {
            jobs: job_tx,
            events: ev_rx,
            cancel,
            status: DRAG_DROP.to_owned(),
            percent: None,
            complete: false,
            pending: 0,
            message: None,
            last_dir: None,
            noise_reduction: Settings::default().noise_reduction,
            output_format: None,
        };
        let batch = !initial.is_empty();
        for path in initial {
            app.handle_file(path);
        }
        if batch {
            let _ = app.jobs.send(Job::BatchQuit);
        }
        app
    }

    /// Validate a dropped/opened file and queue it (mirrors `BaseFrame.handleFile`).
    fn handle_file(&mut self, path: PathBuf) {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        if !READABLE_EXTENSIONS.contains(&ext.as_str()) {
            self.message = Some(format!(
                "Unsupported file type: {}\nSupported: WAV, AIFF, FLAC, MP3, M4A (AAC/ALAC), Ogg Vorbis.",
                path.display()
            ));
            return;
        }
        if !path.is_file() || std::fs::File::open(&path).is_err() {
            self.message = Some(format!("Can't open the source file: {}", path.display()));
            return;
        }
        self.pending += 1;
        let settings = Settings {
            noise_reduction: self.noise_reduction,
            output_format: self.output_format,
            ..Settings::default()
        };
        let _ = self.jobs.send(Job::File(path, Box::new(settings)));
    }

    fn open_dialog(&mut self) {
        let mut dialog = rfd::FileDialog::new()
            .set_title("Choose a file")
            .add_filter("All audio files", READABLE_EXTENSIONS)
            .add_filter("WAV / AIFF", &["wav", "aiff", "aif", "aifc"])
            .add_filter("FLAC", &["flac"])
            .add_filter("MP3", &["mp3"])
            .add_filter("M4A (AAC / ALAC)", &["m4a", "mp4"])
            .add_filter("Ogg Vorbis", &["ogg", "oga"])
            .add_filter("All files (*.*)", &["*"]);
        if let Some(dir) = &self.last_dir {
            dialog = dialog.set_directory(dir);
        }
        if let Some(paths) = dialog.pick_files() {
            if let Some(dir) = paths.first().and_then(|p| p.parent()) {
                self.last_dir = Some(dir.to_path_buf());
            }
            for p in paths {
                self.handle_file(p);
            }
        }
    }

    fn drain_events(&mut self, ctx: &egui::Context) {
        while let Ok(ev) = self.events.try_recv() {
            match ev {
                Event::Progress { name, percent } => {
                    self.status = shorten(&name, 23);
                    self.percent = Some(percent);
                    self.complete = false;
                }
                Event::Done => {
                    self.pending = self.pending.saturating_sub(1);
                    self.percent = None;
                    self.complete = true;
                    self.status = DRAG_DROP.to_owned();
                }
                Event::Failed { message } => {
                    self.pending = self.pending.saturating_sub(1);
                    self.percent = None;
                    self.complete = false;
                    self.status = DRAG_DROP.to_owned();
                    self.message = Some(message);
                }
                Event::BatchQuit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            }
        }
    }
}

impl LevelatorApp {
    /// Noise-reduction and output-format pickers; they apply to files queued afterwards.
    fn options_row(&mut self, ui: &mut egui::Ui) {
        let label = |t: &str| egui::RichText::new(t).color(egui::Color32::GRAY);
        ui.horizontal(|ui| {
            ui.label(label("Noise reduction"));
            egui::ComboBox::from_id_salt("noise")
                .selected_text(self.noise_reduction.map_or("Off", strength_label))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.noise_reduction, None, "Off");
                    for s in [Strength::Mild, Strength::Medium, Strength::Strong] {
                        ui.selectable_value(&mut self.noise_reduction, Some(s), strength_label(s));
                    }
                });
            ui.add_space(8.0);
            ui.label(label("Output"));
            egui::ComboBox::from_id_salt("format")
                .selected_text(self.output_format.map_or("Same as input", Container::name))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.output_format, None, "Same as input");
                    for c in Container::WRITABLE {
                        ui.selectable_value(&mut self.output_format, Some(c), c.name());
                    }
                });
        });
    }
}

fn strength_label(s: Strength) -> &'static str {
    match s {
        Strength::Mild => "Mild",
        Strength::Medium => "Medium",
        Strength::Strong => "Strong",
    }
}

fn spawn_worker(
    jobs: Receiver<Job>,
    events: Sender<Event>,
    ctx: egui::Context,
    cancel: Arc<AtomicBool>,
) {
    thread::Builder::new()
        .name("Levelator".into())
        .spawn(move || {
            for job in jobs {
                let (input, settings) = match job {
                    Job::File(p, s) => (p, s),
                    Job::BatchQuit => {
                        let _ = events.send(Event::BatchQuit);
                        ctx.request_repaint();
                        continue;
                    }
                };
                let name = input
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let output = output_path_for(&input);
                let mut last = u32::MAX;
                let mut progress = |p: u32| {
                    if p != last && p > 0 {
                        last = p;
                        let _ = events.send(Event::Progress {
                            name: name.clone(),
                            percent: p,
                        });
                        ctx.request_repaint();
                    }
                };
                let result = level_file(&input, &output, &settings, &mut progress, &cancel);
                let _ = events.send(match result {
                    Ok(_) => Event::Done,
                    Err(e) => Event::Failed {
                        message: format!("Level reports: {e}"),
                    },
                });
                ctx.request_repaint();
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
            }
        })
        .expect("failed to start worker thread");
}

fn shorten(name: &str, max: usize) -> String {
    if name.chars().count() > max {
        let head: String = name.chars().take(max - 3).collect();
        format!("{head}...")
    } else {
        name.to_owned()
    }
}

/// The original drew a row of short vertical ticks every 5px: blue up to the
/// current percentage, grey for the rest.
fn progress_ticks(ui: &mut egui::Ui, percent: Option<u32>) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 12.0), egui::Sense::hover());
    let Some(percent) = percent else { return };
    let painter = ui.painter_at(rect);
    let done = rect.left() + width * percent.min(100) as f32 / 100.0;
    let mut x = rect.left() + 5.0;
    while x < rect.right() {
        let colour = if x < done {
            egui::Color32::from_rgb(51, 153, 255)
        } else {
            egui::Color32::from_rgb(73, 73, 73)
        };
        painter.line_segment(
            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
            egui::Stroke::new(2.0, colour),
        );
        x += 5.0;
    }
}

impl eframe::App for LevelatorApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.drain_events(&ctx);

        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .collect()
        });
        for p in dropped {
            self.handle_file(p);
        }
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::O)) {
            self.open_dialog();
        }
        let hovering = ctx.input(|i| !i.raw.hovered_files.is_empty());

        let frame = egui::Frame::central_panel(ui.style())
            .fill(egui::Color32::BLACK)
            .inner_margin(16.0);
        egui::CentralPanel::default().frame(frame).show(ui, |ui| {
            let area = ui.interact(
                ui.max_rect(),
                ui.scope_id().with("drop-area"),
                egui::Sense::click(),
            );
            if area.double_clicked() {
                self.open_dialog();
            }
            ui.vertical_centered(|ui| {
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new(TITLE)
                        .size(26.0)
                        .strong()
                        .color(egui::Color32::WHITE),
                );
                ui.label(
                    egui::RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION")))
                        .color(egui::Color32::GRAY),
                );
                ui.add_space(18.0);
                let status = if hovering {
                    "Drop to level"
                } else {
                    self.status.as_str()
                };
                ui.label(
                    egui::RichText::new(status)
                        .size(15.0)
                        .color(egui::Color32::WHITE),
                );
                ui.add_space(6.0);
                progress_ticks(ui, self.percent);
                ui.add_space(4.0);
                let right = match (self.percent, self.complete) {
                    (Some(p), _) => format!("{p} %"),
                    (None, true) => "complete".to_owned(),
                    _ => String::new(),
                };
                ui.label(egui::RichText::new(right).color(egui::Color32::LIGHT_GRAY));
                if self.pending > 1 {
                    ui.label(
                        egui::RichText::new(format!("{} files queued", self.pending - 1))
                            .color(egui::Color32::GRAY),
                    );
                }
                ui.add_space(10.0);
                ui.label(
                    egui::RichText::new("Double-click or press Ctrl/Cmd+O to choose files")
                        .small()
                        .color(egui::Color32::DARK_GRAY),
                );
                ui.add_space(12.0);
                self.options_row(ui);
            });
        });

        if let Some(msg) = self.message.clone() {
            egui::Window::new(TITLE)
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .show(&ctx, |ui| {
                    ui.label(msg);
                    if ui.button("OK").clicked() {
                        self.message = None;
                    }
                });
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

fn main() -> eframe::Result {
    let initial: Vec<PathBuf> = std::env::args_os()
        .skip(1)
        .map(PathBuf::from)
        .filter(|p| !is_psn_arg(p))
        .collect();
    #[allow(unused_mut)]
    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(TITLE)
            .with_inner_size([440.0, 270.0])
            .with_resizable(false)
            .with_drag_and_drop(true),
        ..Default::default()
    };
    #[cfg(target_os = "linux")]
    linux::choose_backend(&mut options);

    let result = eframe::run_native(
        TITLE,
        options,
        Box::new(|cc| Ok(Box::new(LevelatorApp::new(cc.egui_ctx.clone(), initial)))),
    );
    #[cfg(target_os = "linux")]
    if result.is_err() {
        linux::print_help();
    }
    result
}

/// Linux display-server selection.
///
/// winit picks Wayland whenever `WAYLAND_DISPLAY` is set and then fails hard
/// if Wayland can't actually be used: `libwayland-client` not installed
/// ("NoWaylandLib"), or no compositor behind the socket ("NoCompositor"),
/// as happens under WSL or minimal installs. In that case, fall back to X11
/// (including XWayland) when `DISPLAY` is available.
#[cfg(target_os = "linux")]
mod linux {
    use std::path::PathBuf;

    use winit::platform::x11::EventLoopBuilderExtX11;

    fn env_set(name: &str) -> bool {
        std::env::var_os(name).is_some_and(|v| !v.is_empty())
    }

    fn wayland_lib_available() -> bool {
        // SAFETY: loading libwayland-client only runs its (side-effect free)
        // initialisers; the handle is dropped straight away.
        unsafe { libloading::Library::new("libwayland-client.so.0") }.is_ok()
    }

    fn wayland_socket_exists() -> bool {
        let Some(display) = std::env::var_os("WAYLAND_DISPLAY") else {
            return false;
        };
        let path = PathBuf::from(&display);
        if path.is_absolute() {
            return path.exists();
        }
        std::env::var_os("XDG_RUNTIME_DIR")
            .is_some_and(|dir| PathBuf::from(dir).join(path).exists())
    }

    pub fn choose_backend(options: &mut eframe::NativeOptions) {
        if !env_set("WAYLAND_DISPLAY") || !env_set("DISPLAY") {
            return;
        }
        let reason = if !wayland_lib_available() {
            "libwayland-client is not installed"
        } else if !wayland_socket_exists() {
            "no Wayland compositor is running"
        } else {
            return;
        };
        eprintln!("levelator: {reason}; using X11 instead");
        options.event_loop_builder = Some(Box::new(|builder| {
            builder.with_x11();
        }));
    }

    pub fn print_help() {
        if std::path::Path::new("/etc/NIXOS").exists() {
            eprintln!(
                "\nlevelator could not open a window. NixOS keeps the Wayland/OpenGL libraries out of\n\
                 the standard paths, so this prebuilt binary can't find them. Either:\n  \
                 - run the Nix build instead:  nix run github:jack3898/levelator-ng\n  \
                 - or add them to nix-ld:      programs.nix-ld.libraries = with pkgs; [ wayland libxkbcommon libGL ];\n\
                 The command-line tool `level` needs none of these."
            );
            return;
        }
        eprintln!(
            "\nlevelator could not open a window. On Linux it needs a Wayland or X11 desktop and these libraries:\n  \
             Debian/Ubuntu: sudo apt install libwayland-client0 libxkbcommon0 libxkbcommon-x11-0 libx11-6 libxcursor1 libxrandr2 libxi6 libgl1 libegl1\n  \
             Fedora:        sudo dnf install libwayland-client libxkbcommon libxkbcommon-x11 libX11 libXcursor libXrandr libXi mesa-libGL mesa-libEGL\n  \
             Arch:          sudo pacman -S wayland libxkbcommon libxkbcommon-x11 libx11 libxcursor libxrandr libxi mesa\n\
             To force X11, run: env -u WAYLAND_DISPLAY levelator\n\
             The command-line tool `level` needs none of these."
        );
    }
}

/// macOS Finder passes `-psn_0_12345` to apps launched from older systems.
fn is_psn_arg(p: &Path) -> bool {
    p.to_str().is_some_and(|s| s.starts_with("-psn_"))
}
