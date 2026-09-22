use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::thread;

use eframe::egui;

use crate::apple;
use crate::device::{ConnectionMode, DeviceInfo, DeviceTransport, list_connected_devices};
use crate::flasher::{flash_passcode_theme, flash_wallet_skin};
use crate::image_skin::PreparedSkin;
use crate::passthm::{PasscodeTheme, parse_passthm_file};
use crate::scanner::{SavedCard, load_saved_cards, scan_syslog_for_cards};

#[derive(PartialEq, Eq)]
enum AppTab {
    Wallet,
    Passcode,
    Help,
}

enum BackgroundTaskMessage {
    Progress { step: usize, total: usize, message: String },
    Log(String),
    CardFound { hash: String, name: String },
    Done(Result<String, String>),
}

#[cfg(windows)]
fn current_timestamp() -> String {
    #[repr(C)]
    struct SystemTime {
        w_year: u16,
        w_month: u16,
        w_day_of_week: u16,
        w_day: u16,
        w_hour: u16,
        w_minute: u16,
        w_second: u16,
        w_milliseconds: u16,
    }
    unsafe extern "system" {
        fn GetLocalTime(lpSystemTime: *mut SystemTime);
    }
    let mut st = std::mem::MaybeUninit::<SystemTime>::uninit();
    unsafe {
        GetLocalTime(st.as_mut_ptr());
        let st = st.assume_init();
        format!(
            "{:02}:{:02}:{:02}.{:03}",
            st.w_hour, st.w_minute, st.w_second, st.w_milliseconds
        )
    }
}

#[cfg(not(windows))]
fn current_timestamp() -> String {
    "00:00:00.000".to_string()
}

pub struct AirCardApp {
    current_tab: AppTab,
    apple_status: String,
    apple_ready: bool,

    // Device management
    devices: Vec<DeviceInfo>,
    selected_udid: Option<String>,
    connection_mode: ConnectionMode,

    // Wallet tab
    card_hash: String,
    saved_cards: Vec<SavedCard>,
    source_path: Option<PathBuf>,
    skin: Option<PreparedSkin>,
    skin_texture: Option<egui::TextureHandle>,
    scanning_syslog: bool,
    scan_stop_flag: Option<Arc<AtomicBool>>,

    // Passcode tab
    theme_path: Option<PathBuf>,
    loaded_theme: Option<PasscodeTheme>,
    forced_telephony_ver: String,
    keypad_language: String,
    passcode_bold: bool,
    keypad_textures: Vec<(String, egui::TextureHandle)>,

    // Worker thread & progress
    is_busy: bool,
    progress_step: usize,
    progress_total: usize,
    progress_msg: String,
    status_msg: String,
    task_rx: Option<Receiver<BackgroundTaskMessage>>,
    logs: Vec<String>,
    show_logs_window: bool,
}

impl AirCardApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        setup_custom_fonts(&cc.egui_ctx);
        setup_custom_theme(&cc.egui_ctx);

        let (apple_ready, apple_status) = match apple::verify_support() {
            Ok(msg) => (true, msg),
            Err(err) => (false, err.to_string()),
        };

        let mut app = Self {
            current_tab: AppTab::Wallet,
            apple_status,
            apple_ready,

            devices: Vec::new(),
            selected_udid: None,
            connection_mode: ConnectionMode::Auto,

            card_hash: String::new(),
            saved_cards: load_saved_cards(),
            source_path: None,
            skin: None,
            skin_texture: None,
            scanning_syslog: false,
            scan_stop_flag: None,

            theme_path: None,
            loaded_theme: None,
            forced_telephony_ver: "Auto (TelephonyUI-10)".to_string(),
            keypad_language: "English".to_string(),
            passcode_bold: false,
            keypad_textures: Vec::new(),

            is_busy: false,
            progress_step: 0,
            progress_total: 0,
            progress_msg: String::new(),
            status_msg: "就绪。请通过 USB 或已配对 WiFi 连接 iPhone 并解锁。".to_string(),
            task_rx: None,
            logs: Vec::new(),
            show_logs_window: false,
        };

        app.add_log("AirCard Windows 中文版 v1.2.2 已初始化");
        app.add_log(format!("Apple Support Runtime: {}", if app.apple_ready { "已加载并可正常使用" } else { "未找到（需要安装 iTunes）" }));
        app.add_log(format!("已从数据库加载 {} 张已保存的卡片", app.saved_cards.len()));

        if app.apple_ready {
            app.refresh_devices();
        }

        app
    }

    fn add_log(&mut self, text: impl AsRef<str>) {
        let ts = current_timestamp();
        self.logs.push(format!("[{}] {}", ts, text.as_ref()));
        if self.logs.len() > 1000 {
            self.logs.remove(0);
        }
    }

    fn refresh_devices(&mut self) {
        self.add_log("正在通过 usbmuxd 扫描已连接的 iOS 设备...");
        match list_connected_devices() {
            Ok(devs) => {
                self.devices = devs;
                let selection_still_exists = self.selected_udid.as_ref().is_some_and(|selected| {
                    self.devices
                        .iter()
                        .any(|device| device.udid.eq_ignore_ascii_case(selected))
                });
                if !selection_still_exists && !self.devices.is_empty() {
                    self.selected_udid = Some(self.devices[0].udid.clone());
                }
                if self.devices.is_empty() {
                    self.selected_udid = None;
                    self.add_log("未检测到设备。请通过 USB 连接，或在首次 USB 配对后启用 WiFi 同步。");
                    self.status_msg = "未通过 USB 或已配对 WiFi 连接 iPhone。".to_string();
                } else {
                    let dev_logs: Vec<String> = self.devices.iter().enumerate().map(|(i, d)| {
                        format!("设备 #{}：{} - UDID：{}", i + 1, d, d.udid)
                    }).collect();
                    for line in dev_logs {
                        self.add_log(line);
                    }
                    self.status_msg = format!(
                        "发现 {} 台已连接设备；传输模式：{}",
                        self.devices.len(),
                        self.connection_mode.label()
                    );
                }
            }
            Err(err) => {
                self.add_log(format!("设备扫描错误：{}", err));
                self.status_msg = format!("错误：无法枚举设备：{}", err);
            }
        }
    }

    fn selected_transport_available(&self) -> bool {
        self.selected_udid.as_ref().is_some_and(|selected| {
            self.devices.iter().any(|device| {
                if !device.udid.eq_ignore_ascii_case(selected) {
                    return false;
                }
                if self.connection_mode == ConnectionMode::Wifi {
                    return device.has_transport(DeviceTransport::Wifi)
                        && !device.has_transport(DeviceTransport::Usb);
                }
                device.supports(self.connection_mode)
            })
        })
    }

    fn validate_selected_transport(&mut self, operation: &str) -> bool {
        if self.selected_udid.is_none() {
            self.add_log(format!("{} 失败：未选择已连接的 iPhone。", operation));
            self.status_msg = "请选择一台已连接的 iPhone。".to_string();
            return false;
        }
        if !self.selected_transport_available() {
            let wifi_has_usb_attached = self.connection_mode == ConnectionMode::Wifi
                && self.selected_udid.as_ref().is_some_and(|selected| {
                    self.devices.iter().any(|device| {
                        device.udid.eq_ignore_ascii_case(selected)
                            && device.has_transport(DeviceTransport::Wifi)
                            && device.has_transport(DeviceTransport::Usb)
                    })
                });
            self.add_log(format!(
                "{} 失败：所选设备在 {} 模式下不可用。",
                operation,
                self.connection_mode.label()
            ));
            self.status_msg = if wifi_has_usb_attached {
                "请拔掉 USB 线并刷新，以确保 AirTraffic 全程使用 WiFi。"
                    .to_string()
            } else {
                format!(
                    "所选 iPhone 没有 {} 连接。请刷新设备或切换传输模式。",
                    self.connection_mode.label()
                )
            };
            return false;
        }
        true
    }

    fn select_skin(&mut self, ctx: &egui::Context) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Images", &["png", "jpg", "jpeg", "webp"])
            .pick_file()
        else {
            return;
        };

        self.add_log(format!("正在打开卡面图片：{}", path.display()));
        match PreparedSkin::from_path(&path) {
            Ok(skin) => {
                self.add_log(format!(
                    "卡面已处理：原图 {}x{} 已缩放为 1536x969 PNG（{:.1} KB）",
                    skin.source_width,
                    skin.source_height,
                    skin.png.len() as f32 / 1024.0,
                ));
                self.skin_texture = Some(ctx.load_texture(
                    "card-skin-preview",
                    skin.preview.clone(),
                    egui::TextureOptions::LINEAR,
                ));
                self.status_msg = format!(
                    "已准备 {}（{}x{} → 1536x969 PNG，{:.1} KB）",
                    path.file_name().and_then(|n| n.to_str()).unwrap_or("image"),
                    skin.source_width,
                    skin.source_height,
                    skin.png.len() as f32 / 1024.0,
                );
                self.source_path = Some(path);
                self.skin = Some(skin);
            }
            Err(error) => {
                self.add_log(format!("图片处理失败：{error:#}"));
                self.status_msg = format!("错误：无法处理图片：{error:#}");
            }
        }
    }

    fn save_prepared_png(&mut self) {
        let Some(skin) = &self.skin else {
            return;
        };
        let Some(path) = rfd::FileDialog::new()
            .set_file_name("aircard-skin.png")
            .save_file()
        else {
            return;
        };
        match std::fs::write(&path, &skin.png) {
            Ok(()) => {
                self.add_log(format!("已导出处理好的卡面 PNG：{}", path.display()));
                self.status_msg = format!("已保存处理好的 PNG：{}", path.display());
            }
            Err(err) => {
                self.add_log(format!("保存 PNG 失败：{err}"));
                self.status_msg = format!("错误：无法保存 PNG：{err}");
            }
        }
    }

    fn toggle_syslog_scan(&mut self) {
        if self.scanning_syslog {
            if let Some(flag) = self.scan_stop_flag.take() {
                flag.store(true, Ordering::Relaxed);
            }
            self.scanning_syslog = false;
            self.add_log("用户已停止系统日志扫描。");
            self.status_msg = "系统日志扫描已停止。".to_string();
            return;
        }

        if !self.validate_selected_transport("Syslog scan") {
            return;
        }

        let stop_flag = Arc::new(AtomicBool::new(false));
        self.scan_stop_flag = Some(Arc::clone(&stop_flag));
        self.scanning_syslog = true;
        self.add_log("正在启动系统日志监控会话...");
        self.status_msg = "正在扫描系统日志... 请在 iPhone 上打开钱包或轻点卡片。".to_string();

        let (tx, rx) = channel();
        self.task_rx = Some(rx);
        let udid = self.selected_udid.clone();
        let connection_mode = self.connection_mode;

        thread::spawn(move || {
            let tx_card = tx.clone();
            let tx_log = tx.clone();
            let res = scan_syslog_for_cards(
                udid.as_deref(),
                connection_mode,
                stop_flag,
                move |hash, name| {
                    let _ = tx_card.send(BackgroundTaskMessage::CardFound { hash, name });
                },
                move |msg| {
                    let _ = tx_log.send(BackgroundTaskMessage::Log(msg));
                },
            );
            match res {
                Ok(()) => {
                    let _ = tx.send(BackgroundTaskMessage::Done(Ok("系统日志扫描完成".into())));
                }
                Err(e) => {
                    let _ = tx.send(BackgroundTaskMessage::Done(Err(e.to_string())));
                }
            }
        });
    }

    fn flash_card(&mut self) {
        if !self.validate_selected_transport("Card flash") {
            return;
        }
        let Some(udid) = self.selected_udid.clone() else {
            return;
        };
        let hash = self.card_hash.trim().to_string();
        if hash.is_empty() {
            self.add_log("写入失败：目标卡片哈希为空。");
            self.status_msg = "请先输入或扫描目标卡片的哈希值。".to_string();
            return;
        }
        let Some(skin) = self.skin.as_ref() else {
            self.add_log("写入失败：尚未准备卡面图片。");
            self.status_msg = "请先选择一张卡面图片。".to_string();
            return;
        };

        let png_bytes = skin.png.clone();
        let pdf_bytes = skin.pdf.clone();
        if let Some(ref flag) = self.scan_stop_flag {
            flag.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.scanning_syslog = false;
        self.is_busy = true;
        self.progress_step = 0;
        self.progress_total = 3;
        self.progress_msg = "正在开始写入卡面...".to_string();
        self.status_msg = "正在向 iPhone 写入卡面...".to_string();
        let connection_mode = self.connection_mode;
        self.add_log(format!(
            "开始为哈希 {} 写入卡面（UDID：{}，传输：{}）",
            hash,
            udid,
            connection_mode.label()
        ));

        let (tx, rx) = channel();
        self.task_rx = Some(rx);

        thread::spawn(move || {
            let tx_progress = tx.clone();
            let tx_log = tx.clone();
            let res = flash_wallet_skin(
                &udid,
                connection_mode,
                &hash,
                &png_bytes,
                &pdf_bytes,
                move |step, total, msg| {
                    let _ = tx_progress.send(BackgroundTaskMessage::Progress {
                        step,
                        total,
                        message: msg.to_string(),
                    });
                },
                move |msg| {
                    let _ = tx_log.send(BackgroundTaskMessage::Log(msg.to_string()));
                },
            );

            match res {
                Ok(()) => {
                    let _ = tx.send(BackgroundTaskMessage::Done(Ok(
                        "卡面写入成功！请在 iPhone 上强制退出钱包并重新打开。".into(),
                    )));
                }
                Err(e) => {
                    let _ = tx.send(BackgroundTaskMessage::Done(Err(format!("{:#}", e))));
                }
            }
        });
    }

    fn select_theme_file(&mut self, ctx: &egui::Context) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Passcode Theme", &["passthm", "passtheme", "zip"])
            .pick_file()
        else {
            return;
        };

        self.load_theme_from_path(ctx, &path);
    }

    fn load_theme_from_path(&mut self, ctx: &egui::Context, path: &Path) {
        self.add_log(format!("正在打开密码键盘主题包：{}", path.display()));
        let target_ver = match self.forced_telephony_ver.as_str() {
            "TelephonyUI-10" => Some("TelephonyUI-10"),
            "TelephonyUI-9" => Some("TelephonyUI-9"),
            "TelephonyUI-8" => Some("TelephonyUI-8"),
            _ => Some("TelephonyUI-10"),
        };

        match parse_passthm_file(path, target_ver, &self.keypad_language, self.passcode_bold) {
            Ok(theme) => {
                self.keypad_textures.clear();
                for (digit, bytes) in &theme.key_previews {
                    if let Ok(img) = image::load_from_memory(bytes) {
                        let rgba = img.to_rgba8();
                        let color_image = egui::ColorImage::from_rgba_unmultiplied(
                            [rgba.width() as usize, rgba.height() as usize],
                            &rgba,
                        );
                        let tex = ctx.load_texture(
                            format!("keypad-{}", digit),
                            color_image,
                            egui::TextureOptions::LINEAR,
                        );
                        self.keypad_textures.push((digit.clone(), tex));
                    }
                }
                self.keypad_textures.sort_by(|a, b| a.0.cmp(&b.0));

                self.add_log(format!(
                    "已加载密码键盘主题：'{}'（TelephonyUI：{}，语言：{}，粗体：{}，{} 个素材）",
                    theme.name,
                    theme.detected_version,
                    self.keypad_language,
                    self.passcode_bold,
                    theme.items.len()
                ));
                self.status_msg = format!(
                    "已加载 '{}'，共 {} 个素材（目标：{}，语言：{}，粗体：{}）",
                    theme.name,
                    theme.items.len(),
                    theme.detected_version,
                    self.keypad_language,
                    if self.passcode_bold { "ON" } else { "OFF" }
                );
                self.theme_path = Some(path.to_path_buf());
                self.loaded_theme = Some(theme);
            }
            Err(err) => {
                self.add_log(format!("失败：无法解析主题：{err:#}"));
                self.status_msg = format!("失败：无法解析主题：{err:#}");
            }
        }
    }

    fn flash_theme(&mut self) {
        if !self.validate_selected_transport("Theme flash") {
            return;
        }
        let Some(udid) = self.selected_udid.clone() else {
            return;
        };
        let Some(theme) = self.loaded_theme.as_ref() else {
            self.add_log("主题写入失败：未加载 .passthm 主题。");
            self.status_msg = "请先选择 .passthm 主题文件。".to_string();
            return;
        };

        let items = theme.items.clone();
        self.is_busy = true;
        self.progress_step = 0;
        self.progress_total = items.len();
        self.progress_msg = "正在开始写入密码键盘主题...".to_string();
        self.status_msg = "正在写入密码键盘按钮素材...".to_string();
        let connection_mode = self.connection_mode;
        self.add_log(format!(
            "正在通过 {} 向设备 {} 写入密码键盘主题 '{}'（{} 个按钮素材）",
            theme.name,
            items.len(),
            udid,
            connection_mode.label()
        ));

        let (tx, rx) = channel();
        self.task_rx = Some(rx);

        thread::spawn(move || {
            let tx_progress = tx.clone();
            let tx_log = tx.clone();
            let res = flash_passcode_theme(
                &udid,
                connection_mode,
                &items,
                move |step, total, msg| {
                    let _ = tx_progress.send(BackgroundTaskMessage::Progress {
                        step,
                        total,
                        message: msg.to_string(),
                    });
                },
                move |msg| {
                    let _ = tx_log.send(BackgroundTaskMessage::Log(msg.to_string()));
                },
            );

            match res {
                Ok(()) => {
                    let _ = tx.send(BackgroundTaskMessage::Done(Ok(
                        "密码键盘主题已应用！锁定 iPhone 即可查看新键盘。".into(),
                    )));
                }
                Err(e) => {
                    let _ = tx.send(BackgroundTaskMessage::Done(Err(format!("{:#}", e))));
                }
            }
        });
    }

    fn handle_messages(&mut self) {
        let mut messages = Vec::new();
        if let Some(ref rx) = self.task_rx {
            while let Ok(msg) = rx.try_recv() {
                messages.push(msg);
            }
        }

        let mut finished = false;
        for msg in messages {
            match msg {
                BackgroundTaskMessage::Progress { step, total, message } => {
                    self.progress_step = step;
                    self.progress_total = total;
                    self.progress_msg = message.clone();
                    let msg_str = format!("[{}/{}] {}", step, total, message);
                    self.add_log(&msg_str);
                    self.status_msg = msg_str;
                }
                BackgroundTaskMessage::Log(log_line) => {
                    self.add_log(log_line);
                }
                BackgroundTaskMessage::CardFound { hash, name } => {
                    self.card_hash = hash.clone();
                    self.saved_cards = load_saved_cards();
                    let msg_str = format!("已捕获卡片：{}（{}）", name, hash);
                    self.add_log(&msg_str);
                    self.status_msg = msg_str;
                }
                BackgroundTaskMessage::Done(res) => {
                    self.is_busy = false;
                    self.scanning_syslog = false;
                    finished = true;
                    match res {
                        Ok(ok_msg) => {
                            self.add_log(format!("操作完成：{}", ok_msg));
                            self.status_msg = ok_msg;
                        }
                        Err(err_msg) => {
                            self.add_log(format!("操作失败：{}", err_msg));
                            self.status_msg = format!("错误：{}", err_msg);
                        }
                    }
                }
            }
        }
        if finished {
            self.task_rx = None;
        }
    }
}

pub mod md3 {
    use eframe::egui::Color32;

    // M3 Dark scheme
    pub const SURFACE: Color32 = Color32::from_rgb(18, 18, 20);
    pub const SURFACE_CONTAINER: Color32 = Color32::from_rgb(33, 31, 36);
    pub const SURFACE_CONTAINER_HIGH: Color32 = Color32::from_rgb(43, 41, 48);
    pub const SURFACE_CONTAINER_HIGHEST: Color32 = Color32::from_rgb(54, 52, 59);
    pub const ON_SURFACE: Color32 = Color32::from_rgb(230, 225, 229);
    pub const ON_SURFACE_VARIANT: Color32 = Color32::from_rgb(196, 199, 197);
    pub const OUTLINE: Color32 = Color32::from_rgb(147, 143, 153);
    pub const OUTLINE_VARIANT: Color32 = Color32::from_rgb(73, 69, 79);

    // Primary
    pub const PRIMARY: Color32 = Color32::from_rgb(208, 188, 255);
    pub const ON_PRIMARY: Color32 = Color32::from_rgb(56, 30, 114);
    pub const PRIMARY_CONTAINER: Color32 = Color32::from_rgb(79, 55, 139);
    pub const ON_PRIMARY_CONTAINER: Color32 = Color32::from_rgb(234, 221, 255);

    // Secondary
    pub const SECONDARY_CONTAINER: Color32 = Color32::from_rgb(74, 68, 88);
    pub const ON_SECONDARY_CONTAINER: Color32 = Color32::from_rgb(232, 222, 248);

    // Tertiary
    pub const TERTIARY_CONTAINER: Color32 = Color32::from_rgb(99, 59, 72);
    pub const ON_TERTIARY_CONTAINER: Color32 = Color32::from_rgb(255, 216, 228);

    // Error
    pub const ERROR: Color32 = Color32::from_rgb(242, 184, 181);
    pub const ERROR_CONTAINER: Color32 = Color32::from_rgb(140, 29, 24);

    // Extra
    pub const SUCCESS: Color32 = Color32::from_rgb(120, 220, 120);
}

fn draw_status_dot(ui: &mut egui::Ui, color: egui::Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.0, color);
}

/// TelephonyUI 缓存选项：值保留英文（供逻辑匹配），仅显示用中文
fn telephony_label(ver: &str) -> &'static str {
    match ver {
        "TelephonyUI-10" => "TelephonyUI-10（iOS 18 及以上）",
        "TelephonyUI-9" => "TelephonyUI-9（iOS 16-17）",
        "TelephonyUI-8" => "TelephonyUI-8（旧版）",
        _ => "自动（TelephonyUI-10）",
    }
}

/// 键盘语言选项：值保留英文（供逻辑匹配），仅显示用中文
fn keypad_language_label(lang: &str) -> &'static str {
    match lang {
        "Russian" => "俄语",
        "Ukrainian" => "乌克兰语",
        "Japanese" => "日语",
        "All Languages (Universal)" => "全部语言（通用）",
        _ => "英语",
    }
}

fn setup_custom_fonts(ctx: &egui::Context) {
    // 默认字体不含中文字形，需从系统中文字体加载；按顺序尝试常见候选
    let mut fonts = egui::FontDefinitions::default();
    const CJK_FONT_CANDIDATES: &[&str] = &[
        "C:\\Windows\\Fonts\\msyh.ttc",   // 微软雅黑
        "C:\\Windows\\Fonts\\simhei.ttf", // 黑体
        "C:\\Windows\\Fonts\\Deng.ttf",   // 等线
        "C:\\Windows\\Fonts\\simsun.ttc", // 宋体
        "C:\\Windows\\Fonts\\msyhl.ttc",  // 微软雅黑 Light
    ];
    for path in CJK_FONT_CANDIDATES {
        if let Ok(data) = std::fs::read(path) {
            let is_font = data.len() > 12
                && (&data[..4] == b"\x00\x01\x00\x00" // ttf
                    || &data[..4] == b"OTTO"          // otf
                    || &data[..4] == b"ttcf"          // ttc
                    || &data[..4] == b"true");
            if is_font {
                fonts
                    .font_data
                    .insert("system-cjk".to_owned(), std::sync::Arc::new(egui::FontData::from_owned(data)));
                fonts
                    .families
                    .entry(egui::FontFamily::Proportional)
                    .or_default()
                    .push("system-cjk".to_owned());
                fonts
                    .families
                    .entry(egui::FontFamily::Monospace)
                    .or_default()
                    .push("system-cjk".to_owned());
                break;
            }
        }
    }
    ctx.set_fonts(fonts);
}

fn setup_custom_theme(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();

    visuals.panel_fill = md3::SURFACE;
    visuals.window_fill = md3::SURFACE;
    visuals.extreme_bg_color = md3::SURFACE_CONTAINER;
    visuals.faint_bg_color = md3::SURFACE_CONTAINER;

    visuals.window_corner_radius = 16.into();
    visuals.menu_corner_radius = 12.into();

    visuals.widgets.noninteractive.corner_radius = 12.into();
    visuals.widgets.noninteractive.bg_fill = md3::SURFACE_CONTAINER;
    visuals.widgets.noninteractive.bg_stroke = egui::Stroke::NONE;
    visuals.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0_f32, md3::ON_SURFACE);

    visuals.widgets.inactive.bg_fill = md3::SURFACE_CONTAINER_HIGH;
    visuals.widgets.inactive.bg_stroke = egui::Stroke::NONE;
    visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0_f32, md3::ON_SURFACE_VARIANT);
    visuals.widgets.inactive.corner_radius = 12.into();

    visuals.widgets.hovered.bg_fill = md3::SURFACE_CONTAINER_HIGHEST;
    visuals.widgets.hovered.bg_stroke = egui::Stroke::NONE;
    visuals.widgets.hovered.fg_stroke = egui::Stroke::new(1.0_f32, md3::ON_SURFACE);
    visuals.widgets.hovered.corner_radius = 12.into();

    visuals.widgets.active.bg_fill = md3::PRIMARY_CONTAINER;
    visuals.widgets.active.bg_stroke = egui::Stroke::NONE;
    visuals.widgets.active.fg_stroke = egui::Stroke::new(1.0_f32, md3::ON_PRIMARY_CONTAINER);
    visuals.widgets.active.corner_radius = 12.into();

    visuals.widgets.open.bg_fill = md3::SURFACE_CONTAINER_HIGHEST;
    visuals.widgets.open.corner_radius = 12.into();
    visuals.widgets.open.bg_stroke = egui::Stroke::NONE;

    visuals.selection.bg_fill = md3::PRIMARY_CONTAINER;
    visuals.selection.stroke = egui::Stroke::new(1.0_f32, md3::PRIMARY);

    ctx.set_visuals(visuals);

    ctx.style_mut(|style| {
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(16.0, 8.0);
    });
}

fn m3_card<R>(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .fill(md3::SURFACE_CONTAINER)
        .corner_radius(16)
        .inner_margin(egui::Margin::same(20))
        .show(ui, |ui| ui.vertical(add_contents).inner)
        .inner
}

fn m3_button_filled(ui: &mut egui::Ui, label: &str) -> bool {
    let btn = egui::Button::new(
        egui::RichText::new(label).size(13.0).color(md3::ON_PRIMARY),
    )
    .fill(md3::PRIMARY)
    .corner_radius(20)
    .stroke(egui::Stroke::NONE);
    ui.add(btn).clicked()
}

fn m3_button_tonal(ui: &mut egui::Ui, label: &str) -> bool {
    let btn = egui::Button::new(
        egui::RichText::new(label).size(13.0).color(md3::ON_SECONDARY_CONTAINER),
    )
    .fill(md3::SECONDARY_CONTAINER)
    .corner_radius(20)
    .stroke(egui::Stroke::NONE);
    ui.add(btn).clicked()
}

fn m3_button_outlined(ui: &mut egui::Ui, label: &str) -> bool {
    let btn = egui::Button::new(
        egui::RichText::new(label).size(13.0).color(md3::PRIMARY),
    )
    .fill(egui::Color32::TRANSPARENT)
    .corner_radius(20)
    .stroke(egui::Stroke::new(1.0_f32, md3::OUTLINE));
    ui.add(btn).clicked()
}

fn m3_tab(ui: &mut egui::Ui, current: &mut AppTab, target: AppTab, label: &str) {
    let selected = *current == target;
    let (bg, fg) = if selected {
        (md3::SECONDARY_CONTAINER, md3::ON_SECONDARY_CONTAINER)
    } else {
        (egui::Color32::TRANSPARENT, md3::ON_SURFACE_VARIANT)
    };
    let btn = egui::Button::new(
        egui::RichText::new(label).size(12.5).color(fg),
    )
    .fill(bg)
    .corner_radius(20)
    .stroke(egui::Stroke::NONE);
    if ui.add(btn).clicked() {
        *current = target;
    }
}

impl eframe::App for AirCardApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_messages();

        if self.is_busy || self.scanning_syslog {
            ctx.request_repaint();
        }

        // Top bar
        egui::TopBottomPanel::top("header")
            .frame(
                egui::Frame::new()
                    .fill(md3::SURFACE)
                    .inner_margin(egui::Margin::symmetric(20, 10)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("AirCard")
                            .strong()
                            .size(18.0)
                            .color(md3::ON_SURFACE),
                    );
                    ui.label(
                        egui::RichText::new("v1.2.2")
                            .size(11.0)
                            .color(md3::ON_SURFACE_VARIANT),
                    );

                    ui.add_space(20.0);
                    m3_tab(ui, &mut self.current_tab, AppTab::Wallet, "钱包");
                    m3_tab(ui, &mut self.current_tab, AppTab::Passcode, "密码键盘");
                    m3_tab(ui, &mut self.current_tab, AppTab::Help, "帮助");

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if m3_button_outlined(ui, "刷新") {
                            self.refresh_devices();
                        }
                        ui.add_space(4.0);
                        let controls_enabled = !self.is_busy && !self.scanning_syslog;
                        let mut next_mode = self.connection_mode;
                        ui.add_enabled_ui(controls_enabled, |ui| {
                            egui::ComboBox::from_id_salt("connection_mode_combo")
                                .selected_text(next_mode.label())
                                .width(145.0)
                                .show_ui(ui, |ui| {
                                    for mode in ConnectionMode::ALL {
                                        ui.selectable_value(&mut next_mode, mode, mode.label());
                                    }
                                });
                        });
                        if next_mode != self.connection_mode {
                            self.connection_mode = next_mode;
                            self.add_log(format!(
                                "传输模式已切换为 {}。",
                                self.connection_mode.label()
                            ));
                            self.status_msg = format!(
                                "传输模式：{}",
                                self.connection_mode.label()
                            );
                        }

                        ui.add_space(4.0);
                        let mut next_udid = self.selected_udid.clone();
                        let selected_label = self
                            .devices
                            .iter()
                            .find(|device| Some(&device.udid) == self.selected_udid.as_ref())
                            .map(|device| format!("{} [{}]", device.name, device.transport_summary()))
                            .unwrap_or_else(|| "无设备".to_string());
                        ui.add_enabled_ui(controls_enabled && !self.devices.is_empty(), |ui| {
                            egui::ComboBox::from_id_salt("device_selector_combo")
                                .selected_text(selected_label)
                                .width(185.0)
                                .show_ui(ui, |ui| {
                                    for device in &self.devices {
                                        ui.selectable_value(
                                            &mut next_udid,
                                            Some(device.udid.clone()),
                                            format!("{} [{}]", device.name, device.transport_summary()),
                                        );
                                    }
                                });
                        });
                        if next_udid != self.selected_udid {
                            self.selected_udid = next_udid;
                            if let Some(selected) = self.selected_udid.clone() {
                                self.add_log(format!("已选择设备：{}", selected));
                            }
                        }

                        ui.add_space(4.0);
                        let connection_ready = self.selected_transport_available();
                        draw_status_dot(
                            ui,
                            if connection_ready { md3::SUCCESS } else { md3::ERROR },
                        );
                        ui.label(
                            egui::RichText::new(if connection_ready { "就绪" } else { "不可用" })
                                .size(12.0)
                                .color(if connection_ready {
                                    md3::ON_SURFACE
                                } else {
                                    md3::ON_SURFACE_VARIANT
                                }),
                        )
                        .on_hover_text(&self.apple_status);
                    });
                });
            });

        // Status bar
        egui::TopBottomPanel::bottom("status_bar")
            .frame(
                egui::Frame::new()
                    .fill(md3::SURFACE)
                    .inner_margin(egui::Margin::symmetric(20, 8)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let dot_col = if self.is_busy || self.scanning_syslog {
                        md3::PRIMARY
                    } else if self.status_msg.starts_with("错误") || self.status_msg.starts_with("失败") {
                        md3::ERROR
                    } else {
                        md3::SUCCESS
                    };
                    draw_status_dot(ui, dot_col);
                    if self.is_busy || self.scanning_syslog { ui.spinner(); }
                    ui.label(egui::RichText::new(&self.status_msg).size(11.5).color(md3::ON_SURFACE_VARIANT));

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let btn_text = if self.show_logs_window { "日志 [x]" } else { "日志" };
                        let btn = egui::Button::new(
                            egui::RichText::new(btn_text).size(11.0).color(
                                if self.show_logs_window { md3::ON_PRIMARY_CONTAINER } else { md3::ON_SURFACE_VARIANT }
                            ),
                        )
                        .fill(if self.show_logs_window { md3::PRIMARY_CONTAINER } else { egui::Color32::TRANSPARENT })
                        .corner_radius(20)
                        .stroke(egui::Stroke::new(1.0_f32, if self.show_logs_window { md3::PRIMARY } else { md3::OUTLINE_VARIANT }));
                        if ui.add(btn).clicked() {
                            self.show_logs_window = !self.show_logs_window;
                        }
                    });
                });
            });

        // Central - same SURFACE fill as header/status for flat look
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(md3::SURFACE)
                    .inner_margin(egui::Margin::same(16)),
            )
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    match self.current_tab {
                        AppTab::Wallet => self.show_wallet_tab(ctx, ui),
                        AppTab::Passcode => self.show_passcode_tab(ctx, ui),
                        AppTab::Help => self.show_help_tab(ui),
                    }
                });
            });

        let mut show_logs = self.show_logs_window;
        let mut file_saved_msg: Option<String> = None;
        if show_logs {
            egui::Window::new("日志")
                .open(&mut show_logs)
                .default_size([540.0, 300.0])
                .min_size([360.0, 180.0])
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        if m3_button_tonal(ui, "复制日志") {
                            ctx.copy_text(self.logs.join("\n"));
                        }
                        if m3_button_outlined(ui, "另存为文件...") {
                            if let Some(path) = rfd::FileDialog::new()
                                .set_file_name("aircard-diagnostics.log")
                                .add_filter("Log files", &["log", "txt"])
                                .save_file()
                            {
                                let content = self.logs.join("\r\n");
                                let _ = std::fs::write(&path, content);
                                file_saved_msg = Some(format!("日志文件已保存到 {}", path.display()));
                            }
                        }
                        if m3_button_outlined(ui, "清空") {
                            self.logs.clear();
                        }
                        ui.label(
                            egui::RichText::new(format!("共 {} 条", self.logs.len()))
                                .size(11.0)
                                .color(md3::ON_SURFACE_VARIANT),
                        );
                    });
                    ui.add_space(8.0);
                    egui::Frame::new()
                        .fill(md3::SURFACE_CONTAINER_HIGH)
                        .corner_radius(12)
                        .inner_margin(egui::Margin::same(10))
                        .show(ui, |ui| {
                            egui::ScrollArea::vertical()
                                .stick_to_bottom(true)
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    if self.logs.is_empty() {
                                        ui.label(egui::RichText::new("暂无日志记录。").size(11.0).color(md3::ON_SURFACE_VARIANT));
                                    } else {
                                        for line in &self.logs {
                                            ui.label(
                                                egui::RichText::new(line)
                                                    .size(10.5)
                                                    .monospace()
                                                    .color(md3::ON_SURFACE),
                                            );
                                        }
                                    }
                                });
                        });
                });
            self.show_logs_window = show_logs;
            if let Some(msg) = file_saved_msg {
                self.add_log(msg);
            }
        }
    }
}

impl AirCardApp {
    fn show_wallet_tab(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        if self.scanning_syslog {
            egui::Frame::new()
                .fill(md3::TERTIARY_CONTAINER)
                .corner_radius(16)
                .inner_margin(egui::Margin::same(16))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.vertical(|ui| {
                            ui.label(egui::RichText::new("正在扫描系统日志...").strong().size(13.0).color(md3::ON_TERTIARY_CONTAINER));
                            ui.label(egui::RichText::new("请在 iPhone 上打开钱包并轻点您的卡片").size(11.5).color(md3::ON_TERTIARY_CONTAINER));
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let btn = egui::Button::new(egui::RichText::new("停止").size(12.0).color(md3::ON_SURFACE))
                                .fill(md3::ERROR_CONTAINER).corner_radius(20).stroke(egui::Stroke::NONE);
                            if ui.add(btn).clicked() { self.toggle_syslog_scan(); }
                        });
                    });
                });
            ui.add_space(8.0);
        }

        ui.columns(2, |cols| {
            let left = &mut cols[0];
            m3_card(left, |ui| {
                ui.label(egui::RichText::new("卡片配置").strong().size(16.0).color(md3::ON_SURFACE));
                ui.add_space(4.0);
                ui.label(egui::RichText::new("定位您的卡片并选择替换图案").size(12.0).color(md3::ON_SURFACE_VARIANT));
                ui.add_space(16.0);

                // 目标卡片哈希
                ui.label(egui::RichText::new("目标卡片哈希").strong().size(12.0).color(md3::ON_SURFACE));
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let btn_w = 90.0;
                    let text_w = (ui.available_width() - btn_w - 12.0).max(150.0);
                    ui.add(egui::TextEdit::singleline(&mut self.card_hash).hint_text("Base64 卡片哈希...").desired_width(text_w));

                    let scan_label = if self.scanning_syslog { "停止" } else { "扫描" };
                    let scan_bg = if self.scanning_syslog { md3::ERROR_CONTAINER } else { md3::PRIMARY_CONTAINER };
                    let scan_fg = if self.scanning_syslog { md3::ERROR } else { md3::ON_PRIMARY_CONTAINER };
                    let scan_btn = egui::Button::new(egui::RichText::new(scan_label).size(12.0).color(scan_fg))
                        .fill(scan_bg).corner_radius(20).stroke(egui::Stroke::NONE);
                    if ui.add(scan_btn).clicked() { self.toggle_syslog_scan(); }
                });

                if !self.saved_cards.is_empty() {
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new("已保存的卡片").size(11.0).color(md3::ON_SURFACE_VARIANT));
                    ui.add_space(2.0);
                    let combo_w = (ui.available_width() - 4.0).max(150.0);
                    let sel_label = self.saved_cards.iter()
                        .find(|c| c.hash == self.card_hash)
                        .map(|c| format!("{} ({})", c.name, &c.hash[..8.min(c.hash.len())]))
                        .unwrap_or_else(|| "请选择...".into());

                    egui::ComboBox::from_id_salt("saved_cards_box")
                        .width(combo_w)
                        .selected_text(egui::RichText::new(sel_label).color(md3::ON_SURFACE))
                        .show_ui(ui, |ui| {
                            for card in &self.saved_cards {
                                let is_selected = self.card_hash == card.hash;
                                let label = format!("{} ({}...)", card.name, &card.hash[..8.min(card.hash.len())]);
                                let text = egui::RichText::new(label)
                                    .color(if is_selected { md3::ON_PRIMARY_CONTAINER } else { md3::ON_SURFACE })
                                    .strong();
                                if ui.selectable_label(is_selected, text).clicked() {
                                    self.card_hash = card.hash.clone();
                                }
                            }
                        });
                }

                ui.add_space(16.0);

                // Card Skin
                ui.label(egui::RichText::new("卡面图案").strong().size(12.0).color(md3::ON_SURFACE));
                ui.label(egui::RichText::new("PNG、JPG、WebP - 自动缩放至 1536x969").size(11.0).color(md3::ON_SURFACE_VARIANT));
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if m3_button_filled(ui, "选择图片...") { self.select_skin(ctx); }
                    if self.skin.is_some() {
                        if m3_button_tonal(ui, "导出 PNG") { self.save_prepared_png(); }
                    }
                });

                if let Some(skin) = &self.skin {
                    ui.add_space(4.0);
                    let fname = self.source_path.as_ref()
                        .and_then(|p| p.file_name()).and_then(|n| n.to_str()).unwrap_or("image");
                    ui.label(egui::RichText::new(format!("{} - 1536x969 - {:.0} KB", fname, skin.png.len() as f32 / 1024.0)).size(11.0).color(md3::PRIMARY));
                }

                ui.add_space(16.0);

                // Apply
                ui.label(egui::RichText::new("写入 iPhone").strong().size(12.0).color(md3::ON_SURFACE));
                ui.add_space(4.0);

                let can_flash = !self.is_busy
                    && self.selected_transport_available()
                    && !self.card_hash.trim().is_empty()
                    && self.skin.is_some();
                let flash_btn = egui::Button::new(
                    egui::RichText::new("应用卡面").strong().size(14.0)
                        .color(if can_flash { md3::ON_PRIMARY } else { md3::ON_SURFACE_VARIANT }),
                )
                .fill(if can_flash { md3::PRIMARY } else { md3::SURFACE_CONTAINER_HIGH })
                .corner_radius(20).stroke(egui::Stroke::NONE)
                .min_size(egui::vec2(ui.available_width(), 40.0));

                let resp = ui.add_enabled(can_flash, flash_btn);
                if resp.clicked() { self.flash_card(); }
                if !can_flash {
                    let mut r = Vec::new();
                    if self.selected_udid.is_none() { r.push("连接 iPhone"); }
                    else if !self.selected_transport_available() { r.push("选择可用的传输方式"); }
                    if self.card_hash.trim().is_empty() { r.push("输入卡片哈希"); }
                    if self.skin.is_none() { r.push("选择图片"); }
                    if !r.is_empty() { resp.on_disabled_hover_text(format!("需要：{}", r.join(", "))); }
                }

                if self.is_busy {
                    ui.add_space(8.0);
                    if self.progress_total > 0 {
                        ui.add(egui::ProgressBar::new(self.progress_step as f32 / self.progress_total as f32).animate(true));
                    }
                    ui.label(egui::RichText::new(&self.progress_msg).size(11.0).color(md3::PRIMARY));
                }
            });

            // Right: preview
            let right = &mut cols[1];
            m3_card(right, |ui| {
                ui.label(egui::RichText::new("钱包预览").strong().size(16.0).color(md3::ON_SURFACE));
                ui.add_space(4.0);
                ui.label(egui::RichText::new("1536 x 969 像素卡片画布").size(12.0).color(md3::ON_SURFACE_VARIANT));
                ui.add_space(12.0);

                let pass_w = (ui.available_width() - 8.0).clamp(250.0, 400.0);
                let pass_h = pass_w * (969.0 / 1536.0);
                ui.vertical_centered(|ui| {
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(pass_w, pass_h), egui::Sense::hover());
                    let painter = ui.painter();
                    if let Some(tex) = self.skin_texture.as_ref() {
                        painter.image(tex.id(), rect,
                            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                            egui::Color32::WHITE);
                        painter.rect_stroke(rect, 16.0,
                            egui::Stroke::new(1.0_f32, egui::Color32::from_rgba_premultiplied(255, 255, 255, 30)),
                            egui::StrokeKind::Inside);
                    } else {
                        painter.rect_filled(rect, 16.0, md3::SURFACE_CONTAINER_HIGH);
                        painter.text(rect.center(), egui::Align2::CENTER_CENTER,
                            "未加载图案", egui::FontId::proportional(14.0), md3::ON_SURFACE_VARIANT);
                    }
                });

                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("1536x969").size(11.0).color(md3::ON_SURFACE_VARIANT));
                    ui.label(egui::RichText::new("|").size(11.0).color(md3::OUTLINE_VARIANT));
                    ui.label(egui::RichText::new("1.585 比例").size(11.0).color(md3::ON_SURFACE_VARIANT));
                    ui.label(egui::RichText::new("|").size(11.0).color(md3::OUTLINE_VARIANT));
                    if self.skin.is_some() {
                        ui.label(egui::RichText::new("就绪").size(11.0).color(md3::SUCCESS));
                    } else {
                        ui.label(egui::RichText::new("无图片").size(11.0).color(md3::ON_SURFACE_VARIANT));
                    }
                });
                ui.add_space(8.0);
                ui.label(egui::RichText::new("应用后，请强制关闭 Apple 钱包并重新打开。").size(11.0).color(md3::ON_SURFACE_VARIANT));
            });
        });
    }

    fn show_passcode_tab(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        ui.columns(2, |cols| {
            // Left: config
            let left = &mut cols[0];
            m3_card(left, |ui| {
                ui.label(egui::RichText::new("密码键盘主题").strong().size(16.0).color(md3::ON_SURFACE));
                ui.add_space(4.0);
                ui.label(egui::RichText::new("来自 Cowabunga 或 Nugget 的自定义锁屏键盘").size(12.0).color(md3::ON_SURFACE_VARIANT));
                ui.add_space(16.0);

                // Theme file
                ui.label(egui::RichText::new("主题包").strong().size(12.0).color(md3::ON_SURFACE));
                ui.label(egui::RichText::new("选择包含拨号键盘图案的 .passthm 压缩包").size(11.0).color(md3::ON_SURFACE_VARIANT));
                ui.add_space(4.0);
                if m3_button_filled(ui, "选择 .passthm...") { self.select_theme_file(ctx); }

                if let Some(theme) = &self.loaded_theme {
                    let fname = self.theme_path.as_ref()
                        .and_then(|p| p.file_name()).and_then(|n| n.to_str()).unwrap_or("theme.passthm");
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new(format!("{} - {} 个素材", fname, theme.items.len())).size(11.0).color(md3::PRIMARY));
                }

                ui.add_space(16.0);

                // iOS version
                ui.label(egui::RichText::new("目标 iOS 缓存").strong().size(12.0).color(md3::ON_SURFACE));
                ui.label(egui::RichText::new("根据连接的 iOS 版本选择缓存格式").size(11.0).color(md3::ON_SURFACE_VARIANT));
                ui.add_space(4.0);
                let combo_w = (ui.available_width() - 4.0).max(150.0);
                let mut ver_changed = false;
                egui::ComboBox::from_id_salt("telephony_combo")
                    .width(combo_w)
                    .selected_text(telephony_label(&self.forced_telephony_ver))
                    .show_ui(ui, |ui| {
                        ver_changed |= ui.selectable_value(&mut self.forced_telephony_ver, "Auto (TelephonyUI-10)".into(), "自动（TelephonyUI-10）").clicked();
                        ver_changed |= ui.selectable_value(&mut self.forced_telephony_ver, "TelephonyUI-10".into(), "TelephonyUI-10（iOS 18 及以上）").clicked();
                        ver_changed |= ui.selectable_value(&mut self.forced_telephony_ver, "TelephonyUI-9".into(), "TelephonyUI-9（iOS 16-17）").clicked();
                        ver_changed |= ui.selectable_value(&mut self.forced_telephony_ver, "TelephonyUI-8".into(), "TelephonyUI-8（旧版）").clicked();
                    });

                if ver_changed {
                    if let Some(path) = self.theme_path.clone() {
                        self.load_theme_from_path(ctx, &path);
                    }
                }

                ui.add_space(16.0);

                // 键盘语言
                ui.label(egui::RichText::new("键盘语言").strong().size(12.0).color(md3::ON_SURFACE));
                ui.label(egui::RichText::new("数字下方字母布局（英语、俄语、乌克兰语、日语或通用）").size(11.0).color(md3::ON_SURFACE_VARIANT));
                ui.add_space(4.0);
                let mut lang_changed = false;
                egui::ComboBox::from_id_salt("keypad_lang_combo")
                    .width(combo_w)
                    .selected_text(egui::RichText::new(keypad_language_label(&self.keypad_language)).color(md3::ON_SURFACE))
                    .show_ui(ui, |ui| {
                        lang_changed |= ui.selectable_value(&mut self.keypad_language, "English".into(), "英语").clicked();
                        lang_changed |= ui.selectable_value(&mut self.keypad_language, "Russian".into(), "俄语").clicked();
                        lang_changed |= ui.selectable_value(&mut self.keypad_language, "Ukrainian".into(), "乌克兰语").clicked();
                        lang_changed |= ui.selectable_value(&mut self.keypad_language, "Japanese".into(), "日语").clicked();
                        lang_changed |= ui.selectable_value(&mut self.keypad_language, "All Languages (Universal)".into(), "全部语言（通用）").clicked();
                    });

                if lang_changed {
                    if let Some(path) = self.theme_path.clone() {
                        self.load_theme_from_path(ctx, &path);
                    }
                }

                ui.add_space(10.0);

                // Bold Font Toggle
                let mut bold_changed = false;
                ui.horizontal(|ui| {
                    if ui.checkbox(&mut self.passcode_bold, egui::RichText::new("粗体文本（iOS 辅助功能）").strong().size(12.0).color(md3::ON_SURFACE)).changed() {
                        bold_changed = true;
                    }
                });
                ui.label(
                    egui::RichText::new("为在 iPhone 设置 → 显示中开启粗体文本的设备生成 *-bold.png")
                        .size(11.0)
                        .color(md3::ON_SURFACE_VARIANT),
                );

                if bold_changed {
                    if let Some(path) = self.theme_path.clone() {
                        self.load_theme_from_path(ctx, &path);
                    }
                }

                ui.add_space(16.0);

                // Apply
                ui.label(egui::RichText::new("写入 iPhone").strong().size(12.0).color(md3::ON_SURFACE));
                ui.add_space(4.0);

                let can_flash = !self.is_busy
                    && self.selected_transport_available()
                    && self.loaded_theme.is_some();
                let flash_btn = egui::Button::new(
                    egui::RichText::new("应用密码键盘主题").strong().size(14.0)
                        .color(if can_flash { md3::ON_PRIMARY } else { md3::ON_SURFACE_VARIANT }),
                )
                .fill(if can_flash { md3::PRIMARY } else { md3::SURFACE_CONTAINER_HIGH })
                .corner_radius(20).stroke(egui::Stroke::NONE)
                .min_size(egui::vec2(ui.available_width(), 40.0));

                let resp = ui.add_enabled(can_flash, flash_btn);
                if resp.clicked() { self.flash_theme(); }
                if !can_flash {
                    let mut r = Vec::new();
                    if self.selected_udid.is_none() { r.push("连接 iPhone"); }
                    else if !self.selected_transport_available() { r.push("选择可用的传输方式"); }
                    if self.loaded_theme.is_none() { r.push("选择主题"); }
                    if !r.is_empty() { resp.on_disabled_hover_text(format!("需要：{}", r.join(", "))); }
                }

                if self.is_busy {
                    ui.add_space(8.0);
                    if self.progress_total > 0 {
                        ui.add(egui::ProgressBar::new(self.progress_step as f32 / self.progress_total as f32).animate(true));
                    }
                    ui.label(egui::RichText::new(&self.progress_msg).size(11.0).color(md3::PRIMARY));
                }
            });

            // Right: preview
            let right = &mut cols[1];
            m3_card(right, |ui| {
                ui.label(egui::RichText::new("键盘预览").strong().size(16.0).color(md3::ON_SURFACE));
                ui.add_space(4.0);
                ui.label(egui::RichText::new("拨号键盘按钮图案").size(12.0).color(md3::ON_SURFACE_VARIANT));
                ui.add_space(12.0);

                let pass_w = (ui.available_width() - 8.0).clamp(240.0, 360.0);
                let pass_h = 265.0;

                ui.vertical_centered(|ui| {
                    if self.keypad_textures.is_empty() {
                        let (rect, _) = ui.allocate_exact_size(egui::vec2(pass_w, pass_h), egui::Sense::hover());
                        let painter = ui.painter();
                        painter.rect_filled(rect, 16.0, md3::SURFACE_CONTAINER_HIGH);
                        painter.text(rect.center(), egui::Align2::CENTER_CENTER,
                            "未加载主题", egui::FontId::proportional(14.0), md3::ON_SURFACE_VARIANT);
                    } else {
                        let (rect, _) = ui.allocate_exact_size(egui::vec2(pass_w, pass_h), egui::Sense::hover());
                        let painter = ui.painter();
                        painter.rect_filled(rect, 16.0, md3::SURFACE_CONTAINER_HIGH);
                        ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                            ui.vertical_centered(|ui| {
                                ui.add_space(10.0);
                                const DIALER_LAYOUT: &[&[&str]] = &[
                                    &["1", "2", "3"],
                                    &["4", "5", "6"],
                                    &["7", "8", "9"],
                                    &["", "0", ""],
                                ];
                                egui::Grid::new("keypad_grid")
                                    .spacing([18.0, 6.0])
                                    .show(ui, |ui| {
                                        for row in DIALER_LAYOUT {
                                            for &d in *row {
                                                if d.is_empty() {
                                                    ui.allocate_exact_size(egui::vec2(44.0, 50.0), egui::Sense::hover());
                                                } else if let Some((_, tex)) = self.keypad_textures.iter().find(|(k, _)| k == d) {
                                                    ui.vertical_centered(|ui| {
                                                        egui::Frame::new()
                                                            .fill(md3::SURFACE)
                                                            .corner_radius(12)
                                                            .inner_margin(3)
                                                            .show(ui, |ui| { ui.image((tex.id(), egui::vec2(40.0, 40.0))); });
                                                        ui.label(egui::RichText::new(d).size(9.5).color(md3::ON_SURFACE_VARIANT));
                                                    });
                                                } else {
                                                    ui.vertical_centered(|ui| {
                                                        egui::Frame::new()
                                                            .fill(md3::SURFACE)
                                                            .corner_radius(12)
                                                            .inner_margin(3)
                                                            .show(ui, |ui| {
                                                                let (btn_rect, _) = ui.allocate_exact_size(egui::vec2(40.0, 40.0), egui::Sense::hover());
                                                                ui.painter().rect_filled(btn_rect, 8.0, md3::SURFACE_CONTAINER);
                                                                ui.painter().text(btn_rect.center(), egui::Align2::CENTER_CENTER, d, egui::FontId::proportional(14.0), md3::ON_SURFACE_VARIANT);
                                                            });
                                                        ui.label(egui::RichText::new(d).size(9.5).color(md3::ON_SURFACE_VARIANT));
                                                    });
                                                }
                                            }
                                            ui.end_row();
                                        }
                                    });
                            });
                        });
                    }
                });

                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("3x4 键盘").size(11.0).color(md3::ON_SURFACE_VARIANT));
                    ui.label(egui::RichText::new("|").size(11.0).color(md3::OUTLINE_VARIANT));
                    ui.label(egui::RichText::new("TelephonyUI").size(11.0).color(md3::ON_SURFACE_VARIANT));
                    ui.label(egui::RichText::new("|").size(11.0).color(md3::OUTLINE_VARIANT));
                    if self.loaded_theme.is_some() {
                        ui.label(egui::RichText::new("就绪").size(11.0).color(md3::SUCCESS));
                    } else {
                        ui.label(egui::RichText::new("无主题").size(11.0).color(md3::ON_SURFACE_VARIANT));
                    }
                });
                ui.add_space(8.0);
                ui.label(egui::RichText::new("应用后，锁定 iPhone 即可查看新键盘。").size(11.0).color(md3::ON_SURFACE_VARIANT));
            });
        });
    }

    fn show_help_tab(&mut self, ui: &mut egui::Ui) {
        ui.columns(2, |cols| {
            let left = &mut cols[0];
            m3_card(left, |ui| {
                ui.label(egui::RichText::new("设置与卡片哈希指南").strong().size(16.0).color(md3::ON_SURFACE));
                ui.add_space(4.0);
                ui.label(egui::RichText::new("连接并捕获卡片所需的一切").size(12.0).color(md3::ON_SURFACE_VARIANT));
                ui.add_space(16.0);

                ui.label(egui::RichText::new("前置条件").strong().size(12.0).color(md3::ON_SURFACE));
                ui.add_space(6.0);
                ui.label(egui::RichText::new("- 已安装 64 位 iTunes 或 Apple 移动设备支持").size(11.5).color(md3::ON_SURFACE_VARIANT));
                ui.label(egui::RichText::new("- 首次设置：通过 USB 连接并在 iPhone 上点按「信任此电脑」").size(11.5).color(md3::ON_SURFACE_VARIANT));
                ui.label(egui::RichText::new("- WiFi：启用 WiFi 同步，然后与电脑连接同一局域网").size(11.5).color(md3::ON_SURFACE_VARIANT));
                ui.label(egui::RichText::new("- 在顶部工具栏选择自动、仅 USB 或仅 WiFi").size(11.5).color(md3::ON_SURFACE_VARIANT));

                ui.add_space(18.0);

                ui.label(egui::RichText::new("查找您的卡片哈希").strong().size(12.0).color(md3::ON_SURFACE));
                ui.add_space(6.0);
                ui.label(egui::RichText::new("1. 在钱包页点击「扫描」").size(11.5).color(md3::ON_SURFACE_VARIANT));
                ui.label(egui::RichText::new("2. 在 iPhone 上打开 Apple 钱包").size(11.5).color(md3::ON_SURFACE_VARIANT));
                ui.label(egui::RichText::new("3. 轻点您要自定义的卡片").size(11.5).color(md3::ON_SURFACE_VARIANT));
                ui.label(egui::RichText::new("4. AirCard 会自动捕获卡片哈希").size(11.5).color(md3::ON_SURFACE_VARIANT));
                ui.label(egui::RichText::new("5. 捕获到后点击「停止」").size(11.5).color(md3::ON_SURFACE_VARIANT));
            });

            let right = &mut cols[1];
            m3_card(right, |ui| {
                ui.label(egui::RichText::new("应用与主题指南").strong().size(16.0).color(md3::ON_SURFACE));
                ui.add_space(4.0);
                ui.label(egui::RichText::new("应用卡面与拨号键盘主题包").size(12.0).color(md3::ON_SURFACE_VARIANT));
                ui.add_space(16.0);

                ui.label(egui::RichText::new("激活 Apple 钱包卡面").strong().size(12.0).color(md3::ON_SURFACE));
                ui.add_space(6.0);
                ui.label(egui::RichText::new("1. 点击「应用卡面」并等待完成").size(11.5).color(md3::ON_SURFACE_VARIANT));
                ui.label(egui::RichText::new("2. 在 iPhone 上打开应用切换器（从底部向上轻扫）").size(11.5).color(md3::ON_SURFACE_VARIANT));
                ui.label(egui::RichText::new("3. 向上轻扫强制关闭 Apple 钱包").size(11.5).color(md3::ON_SURFACE_VARIANT));
                ui.label(egui::RichText::new("4. 重新打开钱包 - 新卡面就出现了！").size(11.5).color(md3::ON_SURFACE_VARIANT));

                ui.add_space(18.0);

                ui.label(egui::RichText::new("密码键盘主题（.passthm）").strong().size(12.0).color(md3::ON_SURFACE));
                ui.add_space(6.0);
                ui.label(egui::RichText::new("- 兼容 Cowabunga 和 Nugget 主题包").size(11.5).color(md3::ON_SURFACE_VARIANT));
                ui.label(egui::RichText::new("- iOS 18 及以上：选择「自动（TelephonyUI-10）」").size(11.5).color(md3::ON_SURFACE_VARIANT));
                ui.label(egui::RichText::new("- iOS 16-17：选择「TelephonyUI-9」").size(11.5).color(md3::ON_SURFACE_VARIANT));
                ui.label(egui::RichText::new("- 锁定屏幕以查看更新后的键盘图案").size(11.5).color(md3::ON_SURFACE_VARIANT));
            });
        });
    }
}
