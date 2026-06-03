use crate::config::{self, LaunchConfig, ResourceEstimate};
use crate::gguf::{self, GgufInfo};
use crate::hwdetect::{self, HardwareInfo};
use crate::persistence::{self, ModelEntry, NamedPreset, SavedConfig};
use crate::server::{ServerManager, ServerStatus};
#[cfg(windows)]
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::path::Path;
use std::sync::{mpsc::Receiver, Arc, Mutex};

#[cfg(windows)]
use windows_sys::Win32::Foundation::HWND;
#[cfg(windows)]
use windows_sys::Win32::UI::WindowsAndMessaging::{
    IsIconic, SetForegroundWindow, ShowWindow, SW_RESTORE, SW_SHOW,
};

const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

pub enum AppEvent {
    ShowWindow,
    Quit,
}

pub struct LamaBlanketApp {
    hw: HardwareInfo,
    models_folder: String,
    model_list: Vec<ModelEntry>,
    model_path: String,
    mmproj_path: String,
    gguf_info: Option<GgufInfo>,
    gguf_error: Option<String>,
    config: LaunchConfig,
    resources: Option<ResourceEstimate>,
    server: ServerManager,
    log_scroll: bool,
    status_msg: String,
    needs_rescan: bool,
    pending_scan: bool,
    prev_status: ServerStatus,
    crash_detected: bool,
    presets: Vec<NamedPreset>,
    selected_preset: String,
    preset_name_input: String,
    loading_model: bool,
    loading_model_path: String,
    model_result: Arc<Mutex<Option<Result<GgufInfo, String>>>>,
    auto_launch: bool,
    auto_restart: bool,
    dark_mode: bool,
    prev_dark_mode: bool,
    pub tray: Option<tray_icon::TrayIcon>,
    app_events: Receiver<AppEvent>,
    window_handle: Arc<Mutex<Option<isize>>>,
}

impl LamaBlanketApp {
    pub fn new(app_events: Receiver<AppEvent>, window_handle: Arc<Mutex<Option<isize>>>) -> Self {
        let exe_path = find_server_exe();
        let saved = persistence::load_config();

        let models_folder = saved.models_folder.clone().unwrap_or_default();
        let model_list = if !models_folder.is_empty() {
            persistence::scan_models_folder(&models_folder)
        } else {
            Vec::new()
        };

        let mut config = LaunchConfig::default();

        if let Some(v) = saved.ctx_size { config.ctx_size = v; }
        if let Some(v) = saved.gpu_layers { config.gpu_layers = v; }
        if let Some(v) = saved.threads { config.threads = v; }
        if let Some(v) = saved.threads_batch { config.threads_batch = v; }
        if let Some(v) = saved.batch_size { config.batch_size = v; }
        if let Some(ref v) = saved.cache_type_k { config.cache_type_k = v.clone(); config.cache_type_v = v.clone(); }
        if let Some(ref v) = saved.flash_attn { config.flash_attn = v.clone(); }
        if let Some(v) = saved.port { config.port = v; }
        if let Some(v) = saved.mlock { config.mlock = v; }

        let presets = persistence::load_presets();

        let dark_mode = saved.dark_mode.unwrap_or_else(|| is_system_dark_mode());

        Self {
            hw: hwdetect::detect(),
            models_folder,
            model_list,
            model_path: String::new(),
            mmproj_path: String::new(),
            gguf_info: None,
            gguf_error: None,
            config,
            resources: None,
            server: ServerManager::new(&exe_path),
            log_scroll: true,
            status_msg: format!("llama-server path: {exe_path}"),
            needs_rescan: false,
            pending_scan: false,
            prev_status: ServerStatus::Stopped,
            crash_detected: false,
            presets,
            selected_preset: String::new(),
            preset_name_input: String::new(),
            loading_model: false,
            loading_model_path: String::new(),
            model_result: Arc::new(Mutex::new(None)),
            auto_launch: saved.auto_launch.unwrap_or(false),
            auto_restart: false,
            dark_mode,
            prev_dark_mode: dark_mode,
            tray: None,
            app_events,
            window_handle,
        }
    }

    pub fn apply_theme(&mut self, ctx: &egui::Context) {
        self.prev_dark_mode = self.dark_mode;
        ctx.set_visuals(self.theme_visuals());
    }

    fn theme_visuals(&self) -> egui::Visuals {
        if self.dark_mode {
            egui::Visuals::dark()
        } else {
            light_visuals()
        }
    }

    fn show_window(&self, ctx: &egui::Context) {
        restore_native_window(&self.window_handle);
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    fn capture_native_window_handle(&self, frame: &eframe::Frame) {
        #[cfg(windows)]
        {
            let Ok(handle) = frame.window_handle() else {
                return;
            };

            let raw = handle.as_raw();
            if let RawWindowHandle::Win32(win32) = raw {
                let hwnd = win32.hwnd.get();
                let mut guard = self.window_handle.lock().unwrap();
                if guard.is_none() {
                    *guard = Some(hwnd);
                }
            }
        }
    }

    fn handle_app_event(&mut self, event: AppEvent, ctx: &egui::Context) {
        match event {
            AppEvent::ShowWindow => self.show_window(ctx),
            AppEvent::Quit => {
                self.server.stop();
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    fn recompute_config(&mut self) {
        if let Some(ref info) = self.gguf_info {
            let overrides = LaunchConfig {
                model_path: self.model_path.clone(),
                mmproj_path: self.mmproj_path.clone(),
                ctx_size: self.config.ctx_size,
                gpu_layers: self.config.gpu_layers,
                threads: self.config.threads,
                threads_batch: self.config.threads_batch,
                batch_size: self.config.batch_size,
                ubatch_size: self.config.ubatch_size,
                flash_attn: self.config.flash_attn.clone(),
                cache_type_k: self.config.cache_type_k.clone(),
                cache_type_v: self.config.cache_type_v.clone(),
                mlock: self.config.mlock,
                port: self.config.port,
                host: self.config.host.clone(),
                split_mode: self.config.split_mode.clone(),
            };
            self.config = config::compute(info, &self.hw, Some(&overrides));
            self.resources = Some(config::estimate_resources(
                info,
                &self.config,
                &self.hw,
            ));
        }
    }

    fn load_model(&mut self, path: &str) {
        self.model_path = path.to_string();
        self.gguf_error = None;
        self.loading_model = true;
        self.loading_model_path = path.to_string();
        *self.model_result.lock().unwrap() = None;

        let path_buf = Path::new(path).to_path_buf();
        let result = self.model_result.clone();
        std::thread::spawn(move || {
            let r = gguf::parse_gguf(&path_buf)
                .map_err(|e| e.to_string());
            *result.lock().unwrap() = Some(r);
        });
    }

    fn poll_model_result(&mut self) {
        if !self.loading_model {
            return;
        }
        let mut guard = self.model_result.lock().unwrap();
        if let Some(ref result) = *guard {
            let res = result.clone();
            *guard = None;
            drop(guard);
            self.loading_model = false;

            match res {
                Ok(info) => {
                    self.gguf_info = Some(info);
                    self.config.model_path = self.loading_model_path.clone();
                    self.mmproj_path = find_mmproj(&self.loading_model_path);
                    self.config.mmproj_path = self.mmproj_path.clone();
                    self.recompute_config();
                    self.save_current_config();
                }
                Err(e) => {
                    self.gguf_info = None;
                    self.gguf_error = Some(e);
                    self.resources = None;
                }
            }
        }
    }

    fn save_current_config(&self) {
        let saved = SavedConfig {
            models_folder: if self.models_folder.is_empty() {
                None
            } else {
                Some(self.models_folder.clone())
            },
            last_model: if self.model_path.is_empty() {
                None
            } else {
                Some(self.model_path.clone())
            },
            ctx_size: Some(self.config.ctx_size),
            gpu_layers: Some(self.config.gpu_layers),
            threads: Some(self.config.threads),
            threads_batch: Some(self.config.threads_batch),
            batch_size: Some(self.config.batch_size),
            cache_type_k: Some(self.config.cache_type_k.clone()),
            flash_attn: Some(self.config.flash_attn.clone()),
            port: Some(self.config.port),
            mlock: Some(self.config.mlock),
            auto_launch: Some(self.auto_launch),
            dark_mode: Some(self.dark_mode),
        };
        persistence::save_config(&saved);
    }

    fn set_models_folder(&mut self, folder: &str) {
        self.models_folder = folder.to_string();
        self.model_list = persistence::scan_models_folder(folder);
        let saved = SavedConfig {
            models_folder: Some(folder.to_string()),
            ..persistence::load_config()
        };
        persistence::save_config(&saved);
    }

    fn format_size(mb: u64) -> String {
        if mb >= 1024 {
            format!("{:.2} GB", mb as f64 / 1024.0)
        } else {
            format!("{mb} MB")
        }
    }

    fn check_crash(&mut self) {
        let current = self.server.status();
        let was_running = matches!(&self.prev_status,
            ServerStatus::Running | ServerStatus::Healthy
        );
        let now_stopped = matches!(&current,
            ServerStatus::Stopped | ServerStatus::Error(_)
        );

        if was_running && now_stopped && !self.crash_detected {
            self.crash_detected = true;
            let code = self.server.last_exit_code();
            let msg = match &current {
                ServerStatus::Error(e) => format!("Server crashed: {e}"),
                ServerStatus::Stopped => {
                    if let Some(c) = code {
                        if c != 0 {
                            format!("Server exited with code {c}")
                        } else {
                            "Server stopped.".into()
                        }
                    } else {
                        "Server stopped.".into()
                    }
                }
                _ => "Server stopped unexpectedly.".into(),
            };
            self.status_msg = msg.clone();

            if self.auto_restart && self.gguf_info.is_some() && !self.model_path.is_empty() {
                self.status_msg = format!("{msg} — auto-restarting...");
                self.config.model_path = self.model_path.clone();
                self.config.mmproj_path = self.mmproj_path.clone();
                match self.server.start(&self.config) {
                    Ok(()) => {
                        self.status_msg = format!(
                            "Restarted on {}:{}",
                            self.config.host, self.config.port
                        );
                        self.crash_detected = false;
                    }
                    Err(e) => {
                        self.status_msg = format!("Restart failed: {e}");
                    }
                }
            }
        }

        if !now_stopped {
            self.crash_detected = false;
        }
        self.prev_status = current;
    }

    fn apply_preset(&mut self, name: &str) {
        let preset = self.presets.iter().find(|p| p.name == name);
        if let Some(p) = preset.cloned() {
            if let Some(ref path) = p.model_path {
                if Path::new(path).exists() {
                    self.load_model(path);
                }
            }
            if let Some(v) = p.ctx_size { self.config.ctx_size = v; }
            if let Some(v) = p.gpu_layers { self.config.gpu_layers = v; }
            if let Some(v) = p.threads { self.config.threads = v; }
            if let Some(v) = p.threads_batch { self.config.threads_batch = v; }
            if let Some(v) = p.batch_size { self.config.batch_size = v; }
            if let Some(ref v) = p.cache_type_k {
                self.config.cache_type_k = v.clone();
                self.config.cache_type_v = v.clone();
            }
            if let Some(ref v) = p.flash_attn { self.config.flash_attn = v.clone(); }
            if let Some(v) = p.port { self.config.port = v; }
            if let Some(v) = p.mlock { self.config.mlock = v; }
            self.selected_preset = name.to_string();
            self.recompute_config();
            self.save_current_config();
        }
    }

    fn save_preset(&mut self) {
        let name = self.preset_name_input.trim().to_string();
        if name.is_empty() {
            return;
        }
        let preset = NamedPreset {
            name: name.clone(),
            model_path: if self.model_path.is_empty() {
                None
            } else {
                Some(self.model_path.clone())
            },
            ctx_size: Some(self.config.ctx_size),
            gpu_layers: Some(self.config.gpu_layers),
            threads: Some(self.config.threads),
            threads_batch: Some(self.config.threads_batch),
            batch_size: Some(self.config.batch_size),
            cache_type_k: Some(self.config.cache_type_k.clone()),
            flash_attn: Some(self.config.flash_attn.clone()),
            port: Some(self.config.port),
            mlock: Some(self.config.mlock),
        };

        self.presets.retain(|p| p.name != name);
        self.presets.push(preset);
        self.presets
            .sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        persistence::save_presets(&self.presets);
        self.selected_preset = name;
        self.preset_name_input.clear();
    }

    fn import_presets(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Preset JSON", &["json"])
            .pick_file()
        else {
            return;
        };

        match persistence::import_presets(&path) {
            Ok(imported) => {
                let imported_count = imported.len();
                let (added, replaced) = persistence::merge_presets(&mut self.presets, imported);
                persistence::save_presets(&self.presets);
                self.status_msg = format!(
                    "Imported {imported_count} preset(s): {added} added, {replaced} replaced.",
                );
            }
            Err(error) => {
                self.status_msg = error;
            }
        }
    }

    fn delete_preset(&mut self) {
        if self.selected_preset.is_empty() {
            return;
        }
        let name = self.selected_preset.clone();
        self.presets.retain(|p| p.name != name);
        persistence::save_presets(&self.presets);
        self.selected_preset.clear();
    }
}

fn find_mmproj(model_path: &str) -> String {
    let model = Path::new(model_path);
    let dir = match model.parent() {
        Some(d) => d,
        None => return String::new(),
    };

    let model_stem = model
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();

    let mut best: Option<(String, usize)> = None;

    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map(|e| e == "gguf").unwrap_or(false) {
                let fname = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_lowercase();
                if !fname.contains("mmproj") {
                    continue;
                }
                let stem = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_lowercase();

                let score = common_prefix_len(&model_stem, &stem);
                if score > 0 {
                    let replace = best
                        .as_ref()
                        .map(|(_, s)| score > *s)
                        .unwrap_or(true);
                    if replace {
                        best = Some((path.to_string_lossy().to_string(), score));
                    }
                }
            }
        }
    }

    best.map(|(p, _)| p).unwrap_or_default()
}

fn common_prefix_len(a: &str, b: &str) -> usize {
    a.chars()
        .zip(b.chars())
        .take_while(|(ac, bc)| ac == bc)
        .count()
}

fn is_system_dark_mode() -> bool {
    #[cfg(target_os = "windows")]
    {
        use winreg::enums::*;
        use winreg::RegKey;
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let path = r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize";
        hkcu
            .open_subkey_with_flags(path, KEY_READ)
            .ok()
            .and_then(|key| key.get_value::<u32, _>("AppsUseLightTheme").ok())
            .map(|v| v == 0)
            .unwrap_or(true)
    }
    #[cfg(not(target_os = "windows"))]
    {
        true
    }
}

fn light_visuals() -> egui::Visuals {
    let mut v = egui::Visuals::light();
    v.window_fill = egui::Color32::from_rgb(250, 250, 250);
    v.panel_fill = egui::Color32::from_rgb(245, 245, 245);
    v.extreme_bg_color = egui::Color32::from_rgb(240, 240, 240);
    v.faint_bg_color = egui::Color32::from_rgb(238, 238, 238);
    v.widgets.inactive.bg_fill = egui::Color32::from_rgb(230, 230, 230);
    v.widgets.hovered.bg_fill = egui::Color32::from_rgb(210, 210, 210);
    v
}

fn find_server_exe() -> String {
    let candidates = [
        "llama.cpp\\llama-server.exe",
        "..\\llama.cpp\\llama-server.exe",
        "llama-server.exe",
    ];
    for c in &candidates {
        if Path::new(c).exists() {
            return c.to_string();
        }
    }
    candidates[0].to_string()
}

impl eframe::App for LamaBlanketApp {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.capture_native_window_handle(frame);

        while let Ok(event) = self.app_events.try_recv() {
            self.handle_app_event(event, ctx);
        }

        if self.pending_scan {
            self.pending_scan = false;
            self.model_list = persistence::scan_models_folder(&self.models_folder);
        }

        if self.needs_rescan {
            self.needs_rescan = false;
            self.pending_scan = true;
        }

        self.poll_model_result();

        self.check_crash();

        if self.dark_mode != self.prev_dark_mode {
            self.apply_theme(ctx);
        }

        egui::TopBottomPanel::top("menu_bar").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.heading("Lama Blanket");
                if ui
                    .button(if self.dark_mode { "Dark" } else { "Light" })
                    .clicked()
                {
                    self.dark_mode = !self.dark_mode;
                    self.save_current_config();
                }
                if ui.button("—").on_hover_text("Minimize to tray").clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let status = self.server.health_check();
                    let (text, color) = match status {
                        ServerStatus::Stopped => ("Stopped", egui::Color32::GRAY),
                        ServerStatus::Running => ("Running", egui::Color32::from_rgb(200, 150, 0)),
                        ServerStatus::Healthy => ("Healthy", egui::Color32::from_rgb(60, 170, 80)),
                        ServerStatus::Error(_) => ("Crashed", egui::Color32::RED),
                    };
                    ui.label(
                        egui::RichText::new(format!("● {text}")).color(color),
                    );
                });
            });
        });

        let status = self.server.health_check();
        let is_running = matches!(status, ServerStatus::Running | ServerStatus::Healthy);

        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical()
                .id_source("main_scroll")
                .show(ui, |ui| {
                    self.ui_model_panel(ui, is_running);
                    ui.separator();
                    self.ui_hardware_panel(ui);
                    ui.separator();
                    self.ui_config_panel(ui, is_running);
                    ui.separator();
                    self.ui_server_panel(ui, is_running);
                });
        });

        egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(&self.status_msg);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(format!("v{APP_VERSION}"))
                            .weak()
                            .monospace(),
                    );
                });
            });
        });

        ctx.request_repaint_after(std::time::Duration::from_millis(500));
    }
}

pub fn restore_native_window(window_handle: &Arc<Mutex<Option<isize>>>) {
    #[cfg(windows)]
    {
        let hwnd = {
            let guard = window_handle.lock().unwrap();
            guard.map(|value| value as HWND)
        };

        let Some(hwnd) = hwnd else {
            return;
        };

        unsafe {
            if IsIconic(hwnd) != 0 {
                ShowWindow(hwnd, SW_RESTORE);
            } else {
                ShowWindow(hwnd, SW_SHOW);
            }
            SetForegroundWindow(hwnd);
        }
    }

    #[cfg(not(windows))]
    let _ = window_handle;
}

impl LamaBlanketApp {
    fn ui_model_panel(&mut self, ui: &mut egui::Ui, _is_running: bool) {
        ui.heading("Model");

        ui.horizontal(|ui| {
            if ui.button("Open model...").clicked() {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("GGUF Models", &["gguf"])
                    .pick_file()
                {
                    self.load_model(&path.to_string_lossy());
                }
            }
            if self.loading_model {
                ui.spinner();
                ui.label(format!("Loading {}...", self.loading_model_path));
            }
        });

        ui.horizontal(|ui| {
            ui.label("Models folder:");
            ui.monospace(if self.models_folder.is_empty() {
                "(none)"
            } else {
                &self.models_folder
            });
            if ui.button("Set folder...").clicked() {
                if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                    let path = folder.to_string_lossy().to_string();
                    self.set_models_folder(&path);
                }
            }
            if !self.models_folder.is_empty() && ui.button("Refresh").clicked() {
                self.needs_rescan = true;
            }
        });

        if !self.model_list.is_empty() {
            ui.separator();
            ui.label(format!("{} model(s) found:", self.model_list.len()));

            let available_height = ui.available_height();
            let list_height = (available_height * 0.4).min(200.0).max(80.0);

            egui::ScrollArea::vertical()
                .id_source("model_list")
                .auto_shrink([false, false])
                .max_height(list_height)
                .show(ui, |ui| {
                    egui::Grid::new("model_grid")
                        .striped(true)
                        .min_col_width(60.0)
                        .show(ui, |ui| {
                            ui.strong("Name");
                            ui.strong("Size");
                            ui.strong("Author");
                            ui.end_row();

                            let mut clicked_model: Option<String> = None;

                            for entry in &self.model_list {
                                let selected = entry.path == self.model_path;
                                let resp = ui.selectable_label(selected, &entry.filename);
                                if resp.clicked() {
                                    clicked_model = Some(entry.path.clone());
                                }
                                ui.label(Self::format_size(entry.size_mb));
                                ui.label(&entry.author);
                                ui.end_row();
                            }

                            if let Some(path) = clicked_model {
                                self.load_model(&path);
                            }
                        });
                });
        }

        if !self.model_path.is_empty() {
            ui.separator();
            ui.label(format!("Selected: {}", self.model_path));
        }

        if let Some(ref err) = self.gguf_error {
            ui.colored_label(egui::Color32::RED, format!("Error: {err}"));
            return;
        }

        if let Some(ref info) = self.gguf_info {
            ui.horizontal(|ui| {
                ui.label("Name:");
                ui.monospace(&info.model_name);
                ui.separator();
                ui.label("Architecture:");
                ui.monospace(&info.architecture);
                ui.separator();
                ui.label("Layers:");
                ui.monospace(info.block_count.to_string());
                ui.separator();
                ui.label("Context:");
                ui.monospace(format!("{}", info.context_length));
            });
            ui.horizontal(|ui| {
                ui.label("Quantization:");
                ui.monospace(gguf::file_type_name(info.file_type));
                ui.separator();
                ui.label("File size:");
                ui.monospace(Self::format_size(info.file_size / (1024 * 1024)));
                ui.separator();
                ui.label("Emb. length:");
                ui.monospace(info.embedding_length.to_string());
            });

            if let Some(ref res) = self.resources {
                let params_est = info.file_size as f64
                    / gguf::quant_bits_per_parameter(info.file_type) / 8.0;
                ui.label(format!("Estimated params: {:.1} B", params_est / 1e9));
                ui.label(format!(
                    "Offload: {}/{} layers, KV cache: {}, overhead: {}",
                    res.gpu_layers,
                    res.total_layers,
                    Self::format_size(res.kv_cache_mb),
                    Self::format_size(res.overhead_mb),
                ));
            }

            if !self.mmproj_path.is_empty() {
                ui.colored_label(
                    egui::Color32::from_rgb(0, 80, 160),
                    format!("mmproj: {}", self.mmproj_path),
                );
            }
        }
    }

    fn ui_hardware_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("Hardware");
        ui.horizontal(|ui| {
            ui.label("CPU:");
            ui.label(format!(
                "{} ({} physical / {} logical)",
                self.hw.cpu_name.trim(),
                self.hw.physical_cores,
                self.hw.logical_cores
            ));
        });
        ui.label(format!(
            "RAM: {} total, {} available",
            Self::format_size(self.hw.total_ram_mb),
            Self::format_size(self.hw.available_ram_mb),
        ));

        ui.label("GPU:");
        for gpu in &self.hw.gpus {
            ui.label(format!(
                "  {} — {} VRAM",
                gpu.name,
                Self::format_size(gpu.vram_mb)
            ));
        }
    }

    fn ui_config_panel(&mut self, ui: &mut egui::Ui, is_running: bool) {
        ui.heading("Configuration");

        ui.horizontal(|ui| {
            ui.label("Preset:");
            let preset_names: Vec<String> = self
                .presets
                .iter()
                .map(|p| p.name.clone())
                .collect();
            let mut selected = if preset_names.contains(&self.selected_preset) {
                self.selected_preset.clone()
            } else {
                String::new()
            };
            let r = egui::ComboBox::from_id_source("preset_combo")
                .selected_text(if selected.is_empty() {
                    "(none)"
                } else {
                    &selected
                })
                .show_ui(ui, |ui| {
                    for name in &preset_names {
                        if ui.selectable_label(selected == *name, name).clicked() {
                            selected = name.clone();
                        }
                    }
                })
                .response;
            if r.changed() {}
            if selected != self.selected_preset && !selected.is_empty() {
                self.apply_preset(&selected);
            }
        });

        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.preset_name_input)
                    .hint_text("Preset name")
                    .desired_width(120.0),
            );
            if ui
                .add_enabled(
                    !self.preset_name_input.trim().is_empty(),
                    egui::Button::new("Save"),
                )
                .clicked()
            {
                self.save_preset();
            }
            if ui
                .add_enabled(
                    !self.selected_preset.is_empty(),
                    egui::Button::new("Delete"),
                )
                .clicked()
            {
                self.delete_preset();
            }
            if ui.button("Import...").clicked() {
                self.import_presets();
            }
        });

        ui.separator();

        if self.gguf_info.is_none() {
            ui.label("Select a model to see configuration.");
            return;
        }

        let info = self.gguf_info.as_ref().unwrap().clone();
        let changed = self.config_sliders(ui, &info, is_running);

        if changed {
            self.recompute_config();
            self.save_current_config();
        }

        if let Some(ref res) = self.resources {
            ui.separator();
            ui.label("Resource Estimates:");
            ui.label(format!("  Model: {} on disk", Self::format_size(res.model_size_mb)));
            ui.label(format!(
                "  KV Cache ({}): {}",
                self.config.cache_type_k,
                Self::format_size(res.kv_cache_mb)
            ));

            if let Some(gpu) = self.hw.gpus.first() {
                if gpu.vram_mb > 0 {
                    let vram_frac = res.total_vram_mb as f32 / gpu.vram_mb as f32;
                    let vram_frac = vram_frac.min(1.0);
                    let vram_pct = (vram_frac * 100.0) as u32;
                    let bar_color = if res.fits_vram {
                        egui::Color32::from_rgb(0, 140, 60)
                    } else {
                        egui::Color32::RED
                    };
                    ui.label(format!(
                        "  VRAM: {} / {} ({}%)",
                        Self::format_size(res.total_vram_mb),
                        Self::format_size(gpu.vram_mb),
                        vram_pct
                    ));
                    ui.add(
                        egui::ProgressBar::new(vram_frac)
                            .desired_width(200.0)
                            .fill(bar_color)
                            .text(format!("{vram_pct}%")),
                    );
                }
            }

            let ram_frac = res.total_ram_mb as f32 / self.hw.total_ram_mb as f32;
            let ram_frac = ram_frac.min(1.0);
            let ram_pct = (ram_frac * 100.0) as u32;
            let ram_color = if res.fits_ram {
                egui::Color32::from_rgb(0, 140, 60)
            } else {
                egui::Color32::RED
            };
            ui.label(format!(
                "  RAM: {} / {} ({}%)",
                Self::format_size(res.total_ram_mb),
                Self::format_size(self.hw.available_ram_mb),
                ram_pct
            ));
            ui.add(
                egui::ProgressBar::new(ram_frac)
                    .desired_width(200.0)
                    .fill(ram_color)
                    .text(format!("{ram_pct}%")),
            );

            if !res.fits_vram && self.config.gpu_layers > 0 {
                ui.colored_label(
                    egui::Color32::RED,
                    "VRAM exceeded! Reduce GPU layers or context size.",
                );
            }
            if !res.fits_ram {
                ui.colored_label(
                    egui::Color32::RED,
                    "RAM exceeded! Reduce context size or use a smaller model.",
                );
            }
        }

        ui.separator();
        ui.collapsing("How defaults are estimated", |ui| {
            ui.label("Context starts from the model's GGUF context length, capped at 128k, or falls back to 4096 if the file does not provide one.");
            ui.label("GPU layers are estimated from the first GPU's available VRAM by subtracting a 1 GB safety overhead and the estimated KV cache, then fitting as many layers as possible based on model size and layer count.");
            ui.label("Threads default to physical CPU cores, and batch threads default to logical CPU cores.");
            ui.label("Flash attention defaults to auto only for NVIDIA-backed llama models; otherwise it defaults to off.");
            ui.label("mlock defaults on when available RAM is comfortably above model size.");
        });

        ui.separator();
        ui.collapsing("Generated command", |ui| {
            let args = self.config.to_args();
            ui.monospace(format!("llama-server.exe {}", args.join(" ")));
        });
    }

    fn config_sliders(&mut self, ui: &mut egui::Ui, info: &GgufInfo, is_running: bool) -> bool {
        let mut changed = false;

        ui.horizontal(|ui| {
            ui.label("GPU Layers:");
            let gpu_text = format!("{}/{}", self.config.gpu_layers, info.block_count);
            ui.add_enabled_ui(!is_running, |ui| {
                changed |= ui
                    .add(
                        egui::Slider::new(&mut self.config.gpu_layers, 0..=info.block_count)
                            .text(gpu_text),
                    )
                    .changed();
            });
        });

        if self.hw.gpus.len() > 1 {
            ui.horizontal(|ui| {
                ui.label("Multi-GPU mode:");
                ui.add_enabled_ui(!is_running, |ui| {
                    let r = egui::ComboBox::from_id_source("split_mode")
                        .selected_text(if self.config.split_mode.is_empty() {
                            "off"
                        } else {
                            &self.config.split_mode
                        })
                        .show_ui(ui, |ui| {
                            for opt in &["", "layer", "row"] {
                                let label = if opt.is_empty() { "off" } else { *opt };
                                if ui.selectable_value(&mut self.config.split_mode, opt.to_string(), label).changed() {
                                    changed = true;
                                }
                            }
                        })
                        .response;
                    changed |= r.changed();
                });
            });
        }

        ui.horizontal(|ui| {
            ui.label("Context:");
            let max_ctx = (info.context_length.max(4096)).min(128_000);
            let mut ctx = self.config.ctx_size;
            let ctx_text = format!("{ctx}");
            ui.add_enabled_ui(!is_running, |ui| {
                changed |= ui
                    .add(
                        egui::Slider::new(&mut ctx, 512..=max_ctx)
                            .step_by(512.0)
                            .text(ctx_text),
                    )
                    .changed();
            });
            if ctx != self.config.ctx_size {
                self.config.ctx_size = ctx;
            }
        });

        ui.horizontal(|ui| {
            ui.label("Threads:");
            let max_thr = self.hw.logical_cores.max(1) as u64;
            let mut t = self.config.threads as u64;
            let t_text = format!("{t}");
            ui.add_enabled_ui(!is_running, |ui| {
                changed |= ui
                    .add(egui::Slider::new(&mut t, 1..=max_thr).text(t_text))
                    .changed();
            });
            if t != self.config.threads as u64 {
                self.config.threads = t as usize;
            }
        });

        ui.horizontal(|ui| {
            ui.label("Threads (batch):");
            let max_thr = self.hw.logical_cores.max(1) as u64;
            let mut tb = self.config.threads_batch as u64;
            let tb_text = format!("{tb}");
            ui.add_enabled_ui(!is_running, |ui| {
                changed |= ui
                    .add(egui::Slider::new(&mut tb, 1..=max_thr).text(tb_text))
                    .changed();
            });
            if tb != self.config.threads_batch as u64 {
                self.config.threads_batch = tb as usize;
            }
        });

        ui.horizontal(|ui| {
            ui.label("Batch size:");
            ui.add_enabled_ui(!is_running, |ui| {
                let r = egui::ComboBox::from_id_source("batch")
                    .selected_text(self.config.batch_size.to_string())
                    .show_ui(ui, |ui| {
                        for &v in &[512u64, 1024, 2048, 4096, 8192] {
                            if ui.selectable_value(&mut self.config.batch_size, v, v.to_string()).changed() {
                                changed = true;
                            }
                        }
                    })
                    .response;
                changed |= r.changed();
            });
        });

        ui.horizontal(|ui| {
            ui.label("KV Cache:");
            ui.add_enabled_ui(!is_running, |ui| {
                let r = egui::ComboBox::from_id_source("kv_cache")
                    .selected_text(self.config.cache_type_k.clone())
                    .show_ui(ui, |ui| {
                        for opt in &["q8_0", "f16", "q4_0"] {
                            let s = opt.to_string();
                            if ui.selectable_value(&mut self.config.cache_type_k, s.clone(), *opt).changed() {
                                self.config.cache_type_v = s;
                                changed = true;
                            }
                        }
                    })
                    .response;
                changed |= r.changed();
            });
        });

        ui.horizontal(|ui| {
            ui.label("Flash Attn:");
            ui.add_enabled_ui(!is_running, |ui| {
                let r = egui::ComboBox::from_id_source("flash_attn")
                    .selected_text(self.config.flash_attn.clone())
                    .show_ui(ui, |ui| {
                        for opt in &["auto", "on", "off"] {
                            if ui.selectable_value(&mut self.config.flash_attn, opt.to_string(), *opt).changed() {
                                changed = true;
                            }
                        }
                    })
                    .response;
                changed |= r.changed();
            });
        });

        ui.horizontal(|ui| {
            ui.label("Port:");
            let mut port = self.config.port;
            ui.add_enabled_ui(!is_running, |ui| {
                changed |= ui
                    .add(egui::DragValue::new(&mut port).range(1024..=65535))
                    .changed();
            });
            if port != self.config.port {
                self.config.port = port;
            }
        });

        ui.horizontal(|ui| {
            ui.add_enabled_ui(!is_running, |ui| {
                changed |= ui
                    .add(egui::Checkbox::new(&mut self.config.mlock, "Lock model in RAM (mlock)"))
                    .changed();
            });
        });

        ui.horizontal(|ui| {
            let mut al = self.auto_launch;
            if ui
                .add(egui::Checkbox::new(&mut al, "Launch on Windows startup"))
                .changed()
            {
                self.auto_launch = al;
                persistence::set_auto_launch(al);
                let saved = persistence::load_config();
                persistence::save_config(&SavedConfig {
                    auto_launch: Some(al),
                    ..saved
                });
            }
        });

        changed
    }

    fn ui_server_panel(&mut self, ui: &mut egui::Ui, is_running: bool) {
        ui.heading("Server");

        let status = self.server.health_check();
        ui.horizontal(|ui| {
            let (text, color) = match &status {
                ServerStatus::Stopped => ("Stopped", egui::Color32::GRAY),
                ServerStatus::Running => ("Running", egui::Color32::from_rgb(200, 150, 0)),
                ServerStatus::Healthy => ("Healthy", egui::Color32::from_rgb(60, 170, 80)),
                ServerStatus::Error(e) => {
                    self.status_msg = e.clone();
                    ("Crashed", egui::Color32::RED)
                }
            };
            ui.label(
                egui::RichText::new(format!("● {text}")).color(color),
            );
            if let Some(ref cfg) = self.server.active_config() {
                ui.label(format!("on {}:{}", cfg.host, cfg.port));
            }
        });

        if let ServerStatus::Healthy = status {
            ui.label(
                egui::RichText::new("Health check OK").color(egui::Color32::from_rgb(60, 170, 80)),
            );
        } else if let ServerStatus::Running = status {
            ui.label(
                egui::RichText::new("Waiting for server to accept connections...")
                    .color(egui::Color32::from_rgb(200, 150, 0)),
            );
        }

        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    !is_running && self.gguf_info.is_some() && !self.model_path.is_empty(),
                    egui::Button::new("Launch Server"),
                )
                .clicked()
            {
                self.config.model_path = self.model_path.clone();
                self.crash_detected = false;
                match self.server.start(&self.config) {
                    Ok(()) => {
                        self.status_msg = format!(
                            "Server launched on {}:{}",
                            self.config.host, self.config.port
                        );
                    }
                    Err(e) => {
                        self.status_msg = format!("Launch error: {e}");
                    }
                }
            }

            if is_running {
                if ui.button("Open Web UI").clicked() {
                    let url = format!(
                        "http://{}:{}",
                        self.config.host, self.config.port
                    );
                    let _ = open::that(&url);
                }
            }

            if ui
                .add_enabled(is_running, egui::Button::new("Stop Server"))
                .clicked()
            {
                self.server.stop();
                self.status_msg = "Server stopped.".into();
            }
        });

        if self.crash_detected {
            ui.horizontal(|ui| {
                if ui.button("Restart Server").clicked() {
                    self.config.model_path = self.model_path.clone();
                    self.config.mmproj_path = self.mmproj_path.clone();
                    self.crash_detected = false;
                    match self.server.start(&self.config) {
                        Ok(()) => {
                            self.status_msg = format!(
                                "Restarted on {}:{}",
                                self.config.host, self.config.port
                            );
                        }
                        Err(e) => {
                            self.status_msg = format!("Restart failed: {e}");
                        }
                    }
                }
                ui.checkbox(&mut self.auto_restart, "Auto-restart on crash");
            });
        }

        ui.separator();
        ui.label("Log:");
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.log_scroll, "Auto-scroll");
            if ui.button("Clear").clicked() {}
        });

        let log_lines = self.server.log_lines();
        egui::ScrollArea::vertical()
            .id_source("log_area")
            .auto_shrink([false, false])
            .stick_to_bottom(self.log_scroll)
            .max_height(200.0)
            .show(ui, |ui| {
                if log_lines.is_empty() {
                    ui.label("No output yet. Launch the server to see logs.");
                } else {
                    for line in &log_lines {
                        ui.monospace(line);
                    }
                }
            });
    }
}
