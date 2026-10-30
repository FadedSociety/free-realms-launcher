#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod controller;
mod diagnostics;
mod theme;

use controller::{Controller, Navigation};
use eframe::egui::{self, Color32, RichText, Vec2};
use osfr_launcher::{
    LaunchPlan, LaunchSettings, LocalLoginStub, LoginProvider, SessionToken,
    assets::Responder,
    auth::{self, AuthError},
    local::{LOCAL_HANDLE, LocalRealm, LocalSettings},
    patch::{self, PatchSource, Progress},
    release::{self, ReleaseSource},
};
use std::{
    cell::Cell,
    fs::File,
    path::{Path, PathBuf},
    process::Child,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const AUTH_BROKER_ENDPOINT: &str = "127.0.0.1:20043";
const AUTH_BROKER_PORT: u16 = 20043;

fn load_banner(ctx: &egui::Context) -> Result<egui::TextureHandle, String> {
    // (R) Fit bundled art to the current limit, including before the first frame.
    let limit = ctx.input(|input| input.max_texture_side) as u32;
    // Redesign 2026-10-30: the wordmark alone heads the window; the cast lives beside PLAY.
    let image = image::load_from_memory(include_bytes!("../assets/ui/logo.png"))
        .map_err(|_| "The bundled banner could not be decoded.".to_owned())?
        .thumbnail(limit, limit)
        .into_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    Ok(ctx.load_texture(
        "Free Realms Expanded banner",
        egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw()),
        egui::TextureOptions::LINEAR,
    ))
}

fn save_capture(path: &Path, screenshot: &egui::ColorImage) -> Result<(), image::ImageError> {
    use image::ImageEncoder;
    let rgba: Vec<u8> = screenshot
        .pixels
        .iter()
        .flat_map(|pixel| pixel.to_array())
        .collect();
    // Exclusive creation also protects against a file appearing after CLI validation.
    let file = File::options().write(true).create_new(true).open(path)?;
    image::codecs::png::PngEncoder::new(file).write_image(
        &rgba,
        screenshot.width() as u32,
        screenshot.height() as u32,
        image::ExtendedColorType::Rgba8,
    )
}

pub(crate) struct Server {
    pub(crate) name: String,
    pub(crate) address: String,
    pub(crate) profile_id: String,
    pub(crate) patch: PatchSource,
}

pub(crate) struct JoinedRealm {
    pub(crate) profile: auth::Profile,
    pub(crate) address: String,
    pub(crate) public_key: String,
}

type PatchSelection = (String, usize, PatchSource);
struct PatchJob {
    selection: PatchSelection,
    cancel: Arc<AtomicBool>,
    result: mpsc::Receiver<Result<patch::PatchReport, String>>,
}

/// L070: the client download from the community host, running off the UI thread.
struct ClientJob {
    folder: PathBuf,
    cancel: Arc<AtomicBool>,
    result: mpsc::Receiver<Result<patch::PatchReport, String>>,
    progress: Arc<Mutex<Progress>>,
}

/// The local server's install or update check, running off the UI thread.
struct ServerJob {
    cancel: Arc<AtomicBool>,
    result: mpsc::Receiver<Result<release::Outcome, String>>,
    progress: Arc<Mutex<Progress>>,
    had_server: bool,
    /// The last text this job put on the message line; it rewrites only its own text.
    shown: String,
    /// The message line before the check began, restored when the server is already current.
    before: String,
}

/// The message line for a server job: `done` is its result once it has finished. `None` means
/// nothing worth saying (the installed server is already the latest release).
fn server_message(
    had_server: bool,
    progress: &Progress,
    done: Option<&Result<release::Outcome, String>>,
) -> Option<String> {
    let megabytes = |bytes: u64| format!("{:.1} MB", bytes as f64 / 1_048_576.0);
    let reason = |error: &str| error.trim_end_matches('.').to_owned();
    Some(match done {
        Some(Ok(outcome)) if !outcome.installed => return None,
        Some(Ok(outcome)) if had_server => format!(
            "Local server updated to {}. Press Play locally when you are ready.",
            outcome.tag
        ),
        Some(Ok(outcome)) => format!(
            "Local server {} installed. Press Play locally to start it.",
            outcome.tag
        ),
        Some(Err(error)) if had_server => format!(
            "Could not check for a server update ({}). Using the installed server.",
            reason(error)
        ),
        Some(Err(error)) => format!("Could not download the local server ({}).", reason(error)),
        None => {
            // An installed server says an update exists; a first install just says it downloads.
            let head = if had_server {
                "A server update is available. "
            } else {
                ""
            };
            match progress.phase {
                "Update available" if had_server => "A server update is available.".into(),
                "Update available" => "Downloading the local server...".into(),
                "Installing" => format!("{head}Installing the local server files..."),
                "Verifying" | "Downloading" if progress.files_total > 0 => format!(
                    "{head}{}: {}/{} files, {} of {}",
                    if had_server {
                        "Downloading"
                    } else {
                        "Downloading the local server"
                    },
                    progress.files_done,
                    progress.files_total,
                    megabytes(progress.bytes_done),
                    megabytes(progress.bytes_total)
                ),
                "Verifying" => format!("{head}Checking the local server files..."),
                _ => "Checking for a server update...".into(),
            }
        }
    })
}

// (R) Discovery selects existing trusted profiles through auth's validation;
// it never reads private keys or learns trust metadata from the network.
fn saved_profiles() -> Vec<(String, String)> {
    let mut profiles = Vec::new();
    if let Ok(path) = auth::profile_directory("discovery") {
        if let Some(parent) = path.parent() {
            if let Ok(entries) = std::fs::read_dir(parent) {
                for entry in entries.take(128).flatten() {
                    if let Some(id) = entry.file_name().to_str() {
                        if let Ok(profile) = auth::load_profile(id) {
                            profiles.push((profile.id, profile.display_name));
                        }
                    }
                }
            }
        }
    }
    profiles.sort_by(|a, b| a.1.cmp(&b.1));
    profiles
}

pub(crate) fn parse_server_identity(input: &str) -> Option<[u8; 32]> {
    fn nibble(byte: u8) -> Option<u8> {
        match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            b'A'..=b'F' => Some(byte - b'A' + 10),
            _ => None,
        }
    }

    let hex = input.trim().as_bytes();
    if hex.len() != 64 {
        return None;
    }
    let mut identity = [0; 32];
    for (byte, pair) in identity.iter_mut().zip(hex.chunks_exact(2)) {
        *byte = (nibble(pair[0])? << 4) | nibble(pair[1])?;
    }
    Some(identity)
}

pub(crate) fn short_fingerprint(identity: &[u8; 32]) -> String {
    identity[..8]
        .chunks_exact(2)
        .map(|pair| format!("{:02X}{:02X}", pair[0], pair[1]))
        .collect::<Vec<_>>()
        .join("-")
}

/// The broker listens on the realm host's address at the default broker port;
/// a host on loopback or a LAN/VPN address shares it (Security Model decision 2).
pub(crate) fn default_broker(address: &str) -> String {
    address
        .trim()
        .parse::<std::net::SocketAddr>()
        .map(|realm| std::net::SocketAddr::new(realm.ip(), AUTH_BROKER_PORT).to_string())
        .unwrap_or_else(|_| AUTH_BROKER_ENDPOINT.into())
}

/// T771: the local realm's launcher profile id.
const LOCAL_PROFILE: &str = "local-realm";

fn launcher_folder() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()?
        .parent()
        .map(Path::to_path_buf)
}

/// The stock character select lists ids, not names; a client without the known movie is skipped
/// and a failure never stops play.
fn character_select_names(client: &Path) {
    if let Err(error) = osfr_launcher::character_select::ensure_names(client) {
        log::warn!("character select names: {error}");
    }
}

/// L073: text the local server release shows that the stock locale lacks goes into the player's
/// own locale. ponytail: a Settings locale overlay folder replaces these rows for that launch.
fn client_text(client: &Path, content: &Path) {
    match osfr_launcher::client_text::ensure_rows(client, &content.join("locale-additions.json")) {
        Ok(0) => (),
        Ok(added) => log::info!("added {added} text rows to the client locale"),
        Err(error) => log::warn!("client text: {error}"),
    }
}

/// T771: start the local servers, make and enroll this PC's local profile once (a marker file
/// records the enrollment, so a failed one is retried), then prepare and launch the client
/// against the local login port. The returned realm must live as long as the client.
fn start_local(
    local: &LocalSettings,
    client: &Path,
    locale: Option<&Path>,
    settings: LaunchSettings,
) -> Result<(Child, Responder, LocalRealm), String> {
    let realm_dir = auth::ensure_profile_directory(LOCAL_PROFILE)
        .map_err(auth_error_message)?
        .join("realm");
    // The stock creator tables fail osfr-login's pin; the overlay goes into the client copy first.
    osfr_launcher::local::ensure_creator_overlay(client).map_err(|e| e.to_string())?;
    character_select_names(client);
    client_text(client, &local.content);
    // Enroll before the servers start: in account-store mode osfr-login refuses a missing store
    // and never creates one from network input. The profile keeps only the fixed local broker.
    let broker = format!("127.0.0.1:{}", osfr_launcher::local::BROKER_PORT);
    let enrolled = realm_dir.join("enrolled");
    if !enrolled.is_file() {
        let public_key = match auth::load_profile(LOCAL_PROFILE) {
            Ok(_) => auth::print_public_key(LOCAL_PROFILE).map_err(auth_error_message)?,
            Err(_) => {
                let identity = osfr_launcher::local::server_identity(&realm_dir.join("server.id"))
                    .map_err(|e| e.to_string())?;
                let hex: String = identity.iter().map(|b| format!("{b:02x}")).collect();
                create_join_profile_with(
                    &broker,
                    &broker,
                    &hex,
                    LOCAL_HANDLE,
                    LOCAL_PROFILE,
                    auth::new_key,
                    auth::save_profile,
                )?
                .public_key
            }
        };
        let id = osfr_launcher::local::enroll(local, &public_key).map_err(|e| e.to_string())?;
        std::fs::write(&enrolled, id.to_string()).map_err(|e| e.to_string())?;
    }
    // L070: this PC's own account is the realm's admin, once. ponytail: a marker from before
    // L070 holds no id; the launcher's enrollment made the store, so its account is id 1.
    let admin = realm_dir.join("admin");
    if !admin.is_file() {
        let id = std::fs::read_to_string(&enrolled)
            .ok()
            .and_then(|id| id.trim().parse().ok())
            .unwrap_or(1);
        osfr_launcher::local::make_admin(local, id).map_err(|e| e.to_string())?;
        std::fs::write(&admin, b"").map_err(|e| e.to_string())?;
    }
    let realm = LocalRealm::start(local, client, &realm_dir).map_err(|e| e.to_string())?;
    let mut login = RealmLogin {
        profile_id: LOCAL_PROFILE.into(),
        broker: Some(realm.broker),
    };
    let plan = LaunchPlan::build(
        client,
        &realm.login.to_string(),
        locale,
        settings,
        &mut login,
    )?
    .with_portrait_upload(realm.gateway)?;
    plan.apply()?;
    let (child, responder) = plan.launch()?;
    Ok((child, responder, realm))
}

pub(crate) fn create_join_profile_with(
    address: &str,
    broker: &str,
    identity_hex: &str,
    account_handle: &str,
    profile_id: &str,
    create_key: impl FnOnce(&str) -> Result<String, AuthError>,
    save_profile: impl FnOnce(&auth::Profile) -> Result<(), AuthError>,
) -> Result<JoinedRealm, String> {
    osfr_launcher::validate_endpoint(address)?;
    let identity = parse_server_identity(identity_hex)
        .ok_or_else(|| "Paste the host's 64-digit hexadecimal server identity.".to_owned())?;
    let fingerprint = short_fingerprint(&identity);
    let display_name = format!("{account_handle} ({fingerprint})");
    let profile = auth::Profile::new(profile_id, &display_name, broker, identity, account_handle)
        .map_err(auth_error_message)?;
    let public_key = create_key(profile_id).map_err(auth_error_message)?;
    save_profile(&profile).map_err(auth_error_message)?;
    Ok(JoinedRealm {
        profile,
        address: address.to_owned(),
        public_key,
    })
}

pub(crate) fn register_joined_profile(
    joined: &JoinedRealm,
    servers: &mut Vec<Server>,
    profiles: &mut Vec<(String, String)>,
) -> usize {
    let profile = &joined.profile;
    if !profiles.iter().any(|(id, _)| id == &profile.id) {
        profiles.push((profile.id.clone(), profile.display_name.clone()));
        profiles.sort_by(|a, b| a.1.cmp(&b.1));
    }
    servers.push(Server {
        name: format!("Realm ({})", short_fingerprint(&profile.server_identity)),
        address: joined.address.clone(),
        profile_id: profile.id.clone(),
        patch: PatchSource::default(),
    });
    servers.len() - 1
}

pub(crate) fn profile_label<'a>(id: &str, profiles: &'a [(String, String)]) -> &'a str {
    profiles
        .iter()
        .find(|(profile_id, _)| profile_id == id)
        .map(|(_, name)| name.as_str())
        .unwrap_or("Local login")
}

pub(crate) fn copy_public_key(public_key: &str, copy: impl FnOnce(String)) {
    copy(public_key.to_owned());
}

fn next_profile_id(profiles: &[(String, String)]) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let base = format!("realm-{now:x}");
    let mut candidate = base.clone();
    let mut suffix = 2;
    while profiles.iter().any(|(id, _)| id == &candidate) {
        candidate = format!("{base}-{suffix}");
        suffix += 1;
    }
    candidate
}

struct RealmLogin {
    profile_id: String,
    /// L070: this PC's own realm, logged in at the address the launcher started it on.
    broker: Option<std::net::SocketAddr>,
}

impl RealmLogin {
    fn session_with(
        &self,
        endpoint: &str,
        load_profile: impl FnOnce(&str) -> Result<auth::Profile, AuthError>,
        authenticate: impl FnOnce(&auth::Profile) -> Result<SessionToken, AuthError>,
    ) -> osfr_launcher::Result<SessionToken> {
        if self.profile_id.trim().is_empty() {
            return LocalLoginStub.session(endpoint);
        }
        let profile = load_profile(self.profile_id.trim()).map_err(auth_error_message)?;
        login_profile(&profile, authenticate)
    }
}

impl LoginProvider for RealmLogin {
    fn session(&mut self, endpoint: &str) -> osfr_launcher::Result<SessionToken> {
        match self.broker {
            Some(broker) => auth::login_local(&self.profile_id, broker).map_err(auth_error_message),
            None => self.session_with(endpoint, auth::load_profile, auth::login),
        }
    }
}

pub(crate) fn auth_error_message(error: AuthError) -> String {
    match error {
        AuthError::Refused => {
            "Login refused. Another player may hold that handle, or the realm only takes listed players: make a profile with another handle, or send the host your public key from Settings.".into()
        }
        _ => error.to_string(),
    }
}

fn login_profile(
    profile: &auth::Profile,
    authenticate: impl FnOnce(&auth::Profile) -> Result<SessionToken, AuthError>,
) -> osfr_launcher::Result<SessionToken> {
    authenticate(profile).map_err(auth_error_message)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FocusTarget {
    RealmsTab,
    SettingsTab,
    RealmRow(usize),
    AddAddress,
    AddRealm,
    LoginProfile,
    Play,
    JoinAddress,
    JoinBroker,
    JoinIdentity,
    JoinHandle,
    CreateKey,
    CopyPublicKey,
    ClientFolder,
    LocaleFolder,
    WindowWidth,
    WindowHeight,
    ResolutionSmall,
    ResolutionLarge,
    ControllerToggle,
    PatchUrl,
    PatchMirror,
    PatchPin,
    CancelPatch,
    VerifyPatch,
    Back,
}

pub(crate) fn focus_order(
    settings: bool,
    realm_count: usize,
    patch_active: bool,
    public_key_ready: bool,
) -> Vec<FocusTarget> {
    let mut order = vec![FocusTarget::RealmsTab, FocusTarget::SettingsTab];
    if settings {
        order.extend([
            FocusTarget::JoinAddress,
            FocusTarget::JoinBroker,
            FocusTarget::JoinIdentity,
            FocusTarget::JoinHandle,
            FocusTarget::CreateKey,
        ]);
        if public_key_ready {
            order.push(FocusTarget::CopyPublicKey);
        }
        order.extend([
            FocusTarget::ClientFolder,
            FocusTarget::LocaleFolder,
            FocusTarget::PatchUrl,
            FocusTarget::PatchMirror,
            FocusTarget::PatchPin,
        ]);
        if patch_active {
            order.push(FocusTarget::CancelPatch);
        }
        order.extend([
            FocusTarget::WindowWidth,
            FocusTarget::WindowHeight,
            FocusTarget::ResolutionSmall,
            FocusTarget::ResolutionLarge,
            FocusTarget::ControllerToggle,
            FocusTarget::VerifyPatch,
            FocusTarget::Back,
        ]);
    } else {
        order.extend((0..realm_count).map(FocusTarget::RealmRow));
        order.extend([FocusTarget::AddAddress, FocusTarget::AddRealm]);
        if patch_active {
            order.push(FocusTarget::CancelPatch);
        }
        order.extend([FocusTarget::Play, FocusTarget::LoginProfile]);
    }
    order
}

pub(crate) fn focus_sequence_matches(
    actual: &[FocusTarget],
    settings: bool,
    realm_count: usize,
    patch_active: bool,
    public_key_ready: bool,
) -> bool {
    actual == focus_order(settings, realm_count, patch_active, public_key_ready)
}

fn next_focus_index(current: Option<usize>, len: usize, forward: bool) -> Option<usize> {
    if len == 0 {
        return None;
    }
    Some(match current {
        Some(index) if forward => (index + 1) % len,
        Some(0) if !forward => len - 1,
        Some(index) => index - 1,
        None if forward => 0,
        None => len - 1,
    })
}

pub(crate) struct Launcher {
    banner: egui::TextureHandle,
    art: theme::UiArt,
    servers: Vec<Server>,
    selected: usize,
    new_address: String,
    join_address: String,
    join_broker: String,
    join_broker_edited: bool,
    join_identity: String,
    join_handle: String,
    join_public_key: Option<String>,
    join_preview: bool,
    client: String,
    locale: String,
    settings: LaunchSettings,
    settings_page: bool,
    controller_on: bool,
    controller: Option<Controller>,
    controller_status: String,
    pending: Option<mpsc::Receiver<Result<(Child, Responder), String>>>,
    /// The asset responder lives exactly as long as the client it serves.
    child: Option<(Child, Responder)>,
    /// T771: the servers "Play locally" started; dropping it stops them when the client closes.
    local: Option<LocalRealm>,
    pending_local: Option<mpsc::Receiver<Result<(Child, Responder, LocalRealm), String>>>,
    /// The folder holding `server/` (the launcher's own), and where its releases come from.
    launcher_dir: Option<PathBuf>,
    release: ReleaseSource,
    server_job: Option<ServerJob>,
    message: String,
    screenshot: Option<PathBuf>,
    screenshot_requested: bool,
    capture_ok: Rc<Cell<bool>>,
    started: Instant,
    pub(crate) focus_targets: Vec<(FocusTarget, egui::Id)>,
    focus_play_on_start: bool,
    focus_play_requested: bool,
    test_vine_growth: Option<f32>,
    profiles: Vec<(String, String)>,
    patch_job: Option<PatchJob>,
    patch_progress: Arc<Mutex<Progress>>,
    patch_attempt: Option<PatchSelection>,
    patch_ready: Option<PatchSelection>,
    patch_preview: bool,
    /// L070: "Play locally" also hosts friends on `host_address` (this PC's LAN or VPN address).
    host: bool,
    host_choices: Vec<std::net::IpAddr>,
    host_address: Option<std::net::IpAddr>,
    /// L075: host on the internet (router port forwarding or a server host) at `host_public`.
    host_internet: bool,
    host_public: String,
    /// L070: the last PLAY stopped on a missing DirectX runtime; the installer link shows.
    directx_missing: bool,
    client_job: Option<ClientJob>,
}

impl Launcher {
    pub(crate) fn new(
        cc: &eframe::CreationContext<'_>,
        screenshot: Option<PathBuf>,
        settings_page: bool,
        capture_ok: Rc<Cell<bool>>,
        focus_play_on_start: bool,
        test_vine_growth: Option<f32>,
    ) -> Result<Self, String> {
        let banner = load_banner(&cc.egui_ctx)?;
        theme::install(&cc.egui_ctx);
        let art = theme::UiArt::load(&cc.egui_ctx)?;
        let (controller, controller_status) = match Controller::new() {
            Ok(controller) => (Some(controller), "Connect a controller to navigate.".into()),
            Err(error) => (None, error),
        };
        Ok(Self {
            banner,
            art,
            servers: vec![Server {
                name: "Local realm".into(),
                address: "127.0.0.1:20042".into(),
                profile_id: String::new(),
                patch: PatchSource::default(),
            }],
            selected: 0,
            new_address: String::new(),
            join_address: String::new(),
            join_broker: String::new(),
            join_broker_edited: false,
            join_identity: String::new(),
            join_handle: String::new(),
            join_public_key: None,
            join_preview: false,
            client: String::new(),
            locale: String::new(),
            settings: LaunchSettings::default(),
            settings_page,
            controller_on: true,
            controller,
            controller_status,
            pending: None,
            child: None,
            local: None,
            pending_local: None,
            launcher_dir: launcher_folder(),
            release: ReleaseSource::default(),
            server_job: None,
            message: "Choose your client copy in Settings, then select a realm.".into(),
            screenshot,
            screenshot_requested: false,
            capture_ok,
            started: Instant::now(),
            focus_targets: Vec::new(),
            focus_play_on_start,
            focus_play_requested: false,
            test_vine_growth,
            profiles: saved_profiles(),
            patch_job: None,
            patch_progress: Arc::new(Mutex::new(Progress::default())),
            patch_attempt: None,
            patch_ready: None,
            patch_preview: false,
            host: false,
            host_choices: Vec::new(),
            host_address: None,
            host_internet: false,
            host_public: String::new(),
            directx_missing: false,
            client_job: None,
        })
    }

    fn play(&mut self) {
        // A screenshot run cannot prepare files or launch a child, even by pad.
        if self.screenshot.is_some() || self.pending.is_some() || self.child.is_some() {
            return;
        }
        if self.client_job.is_some() {
            self.message = "Wait for the client download to finish, or stop it in Settings.".into();
            return;
        }
        if !self.patch_allows_play() {
            self.message = "Finish verifying the selected realm's patch before playing.".into();
            return;
        }
        if self.client.trim().is_empty() {
            self.settings_page = true;
            self.message = "Choose the client folder before playing.".into();
            return;
        }
        let client = PathBuf::from(&self.client);
        if self.needs_directx(&client) {
            return;
        }
        let locale = (!self.locale.trim().is_empty()).then(|| PathBuf::from(&self.locale));
        let server = &self.servers[self.selected];
        let address = server.address.clone();
        let profile_id = server.profile_id.trim().to_owned();
        let settings = self.settings;
        let content = self
            .launcher_dir
            .as_deref()
            .map(|root| LocalSettings::packaged(root).content);
        let (sender, receiver) = mpsc::channel();
        self.pending = Some(receiver);
        self.message = "Preparing client files and backups...".into();
        // One bounded preparation job; keep the window responsive during locale I/O.
        std::thread::spawn(move || {
            character_select_names(&client);
            if let Some(content) = &content {
                client_text(&client, content);
            }
            let mut login: Box<dyn LoginProvider + Send> = Box::new(RealmLogin {
                profile_id,
                broker: None,
            });
            // L071: a hosted realm (login on 20042) takes portraits on its gateway port, 20041.
            let gateway = address
                .parse::<std::net::SocketAddr>()
                .ok()
                .filter(|login| login.port() == osfr_launcher::local::LOGIN_PORT)
                .map(|login| {
                    std::net::SocketAddr::new(login.ip(), osfr_launcher::local::GATEWAY_PORT)
                });
            let result = LaunchPlan::build(
                &client,
                &address,
                locale.as_deref(),
                settings,
                login.as_mut(),
            )
            .and_then(|plan| match gateway {
                Some(gateway) => plan.with_portrait_upload(gateway),
                None => Ok(plan),
            })
            .and_then(|plan| {
                plan.apply()?;
                plan.launch()
            });
            let _ = sender.send(result);
        });
    }

    /// T771: start the server (`server/` beside the launcher), then PLAY against it. The first
    /// use, or a damaged install, downloads it from GitHub Releases instead and starts nothing.
    fn play_locally(&mut self) {
        if self.screenshot.is_some()
            || self.pending.is_some()
            || self.pending_local.is_some()
            || self.child.is_some()
        {
            return;
        }
        if self.client_job.is_some() {
            self.message = "Wait for the client download to finish, or stop it in Settings.".into();
            return;
        }
        let Some(root) = self.launcher_dir.clone() else {
            self.message = "Could not find the launcher's folder.".into();
            return;
        };
        if self.server_job.is_some() {
            self.message =
                "The local server is being checked or downloaded. Try again in a moment.".into();
            return;
        }
        let mut local = LocalSettings::packaged(&root);
        if !local.missing().is_empty() {
            self.start_server_job(true);
            return;
        }
        if self.host && self.host_internet {
            match self.host_public.trim().parse::<std::net::IpAddr>() {
                Ok(ip) if !ip.is_unspecified() && !ip.is_loopback() && !ip.is_multicast() => {
                    local.host = Some(std::net::IpAddr::from([0, 0, 0, 0]));
                    local.public = Some(ip);
                }
                _ => {
                    self.message = "Enter your public IP (search \"what is my IP\") to host on the internet.".into();
                    return;
                }
            }
        } else if self.host {
            let Some(address) = self.host_address else {
                self.message = "No LAN or VPN address found to host on. Join a network (or start Tailscale) first.".into();
                return;
            };
            local.host = Some(address);
        }
        if self.client.trim().is_empty() {
            self.settings_page = true;
            self.message = "Choose the client folder before playing.".into();
            return;
        }
        let client = PathBuf::from(&self.client);
        if self.needs_directx(&client) {
            return;
        }
        let locale = (!self.locale.trim().is_empty()).then(|| PathBuf::from(&self.locale));
        let settings = self.settings;
        let (sender, receiver) = mpsc::channel();
        self.pending_local = Some(receiver);
        self.message = "Starting the local realm...".into();
        std::thread::spawn(move || {
            let _ = sender.send(start_local(&local, &client, locale.as_deref(), settings));
        });
    }

    /// L070: stop before launch when the client's DirectX DLL is missing, and offer Microsoft's
    /// installer instead of a client that fails on start.
    fn needs_directx(&mut self, client: &Path) -> bool {
        self.directx_missing = osfr_launcher::directx_missing(client);
        if self.directx_missing {
            self.message = "Free Realms needs Microsoft's DirectX End-User Runtime (d3dx9_31.dll is missing). Install it, then press PLAY again.".into();
        }
        self.directx_missing
    }

    /// Install or update the local server from the latest GitHub release, off the UI thread.
    /// `force` re-verifies every file even when the installed release is current.
    fn start_server_job(&mut self, force: bool) {
        let Some(root) = self.launcher_dir.clone() else {
            return;
        };
        if self.screenshot.is_some() || self.server_job.is_some() {
            return;
        }
        let had_server = LocalSettings::packaged(&root).missing().is_empty();
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(Mutex::new(Progress::default()));
        let (sender, result) = mpsc::channel();
        let (stopping, state, source) = (cancel.clone(), progress.clone(), self.release.clone());
        std::thread::spawn(move || {
            let outcome = release::sync(&root.join("server"), &source, force, &stopping, |p| {
                if let Ok(mut state) = state.lock() {
                    *state = p.clone();
                }
            });
            let _ = sender.send(outcome);
        });
        self.server_job = Some(ServerJob {
            cancel,
            result,
            progress,
            had_server,
            shown: String::new(),
            before: self.message.clone(),
        });
    }

    /// On start, an installed server is checked for a newer release. Nothing is downloaded when
    /// there is no server yet (that waits for the first Play locally), and nothing is launched.
    fn check_server_on_start(&mut self) {
        let installed = self
            .launcher_dir
            .as_ref()
            .is_some_and(|root| LocalSettings::packaged(root).missing().is_empty());
        if installed {
            self.start_server_job(false);
        }
    }

    fn poll_server(&mut self) {
        let Some(job) = &mut self.server_job else {
            return;
        };
        let done = match job.result.try_recv() {
            Ok(result) => Some(result),
            Err(mpsc::TryRecvError::Disconnected) => Some(Err("The update worker stopped.".into())),
            Err(mpsc::TryRecvError::Empty) => None,
        };
        let progress = job.progress.lock().map(|p| p.clone()).unwrap_or_default();
        let text = server_message(job.had_server, &progress, done.as_ref());
        // Progress only rewrites the line while it still shows this job's own text; the final
        // result always lands. An up-to-date check puts back what was there before.
        let ours = job.shown.is_empty() || self.message == job.shown;
        match text {
            Some(text) if done.is_some() || (ours && text != self.message) => {
                job.shown = text.clone();
                self.message = text;
            }
            None if ours => self.message = job.before.clone(),
            _ => (),
        }
        if done.is_some() {
            self.server_job = None;
        }
    }

    /// L070: download (or repair) the client into `client/` beside the launcher from the
    /// community host; the client folder setting points there once every file checks out.
    fn start_client_download(&mut self) {
        let Some(root) = self.launcher_dir.clone() else {
            return;
        };
        if self.screenshot.is_some() || self.client_job.is_some() {
            return;
        }
        let folder = root.join("client");
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(Mutex::new(Progress::default()));
        let (sender, result) = mpsc::channel();
        let (target, stopping, state) = (folder.clone(), cancel.clone(), progress.clone());
        std::thread::spawn(move || {
            let outcome = patch::download_client(&target, &stopping, |p| {
                if let Ok(mut state) = state.lock() {
                    *state = p.clone();
                }
            });
            let _ = sender.send(outcome);
        });
        self.message = "Fetching the client list from the community host...".into();
        self.client_job = Some(ClientJob {
            folder,
            cancel,
            result,
            progress,
        });
    }

    fn poll_client(&mut self) {
        let Some(job) = &self.client_job else {
            return;
        };
        match job.result.try_recv() {
            Ok(Ok(report)) => {
                self.client = job.folder.display().to_string();
                self.message = format!(
                    "Client ready in {} ({} files downloaded).",
                    self.client, report.files_downloaded
                );
                self.client_job = None;
            }
            Ok(Err(error)) => {
                self.message = error;
                self.client_job = None;
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.message = "The client download stopped. Download again to resume.".into();
                self.client_job = None;
            }
            Err(mpsc::TryRecvError::Empty) => {
                let p = job.progress.lock().map(|p| p.clone()).unwrap_or_default();
                if p.bytes_total > 0 {
                    self.message = format!(
                        "{} the client: {} of {} files, {} of {} MB.",
                        p.phase,
                        p.files_done,
                        p.files_total,
                        p.bytes_done >> 20,
                        p.bytes_total >> 20
                    );
                }
            }
        }
    }

    fn patch_selection(&self) -> PatchSelection {
        (
            self.client.trim().to_owned(),
            self.selected,
            self.servers[self.selected].patch.clone(),
        )
    }

    fn patch_allows_play(&self) -> bool {
        self.patch_job.is_none()
            && !self.patch_preview
            && (self.servers[self.selected]
                .patch
                .manifest_url
                .trim()
                .is_empty()
                || self.patch_ready.as_ref() == Some(&self.patch_selection()))
    }

    fn patch_active(&self) -> bool {
        self.patch_job.is_some() || self.patch_preview
    }

    fn create_join_profile(&mut self) {
        if self.servers.len() >= 16 {
            self.message =
                "This launcher already has 16 realms. Remove one before adding another.".into();
            return;
        }
        let profile_id = next_profile_id(&self.profiles);
        match create_join_profile_with(
            self.join_address.trim(),
            self.join_broker.trim(),
            self.join_identity.trim(),
            self.join_handle.trim(),
            &profile_id,
            auth::new_key,
            auth::save_profile,
        ) {
            Ok(joined) => {
                self.selected =
                    register_joined_profile(&joined, &mut self.servers, &mut self.profiles);
                self.join_public_key = Some(joined.public_key);
                self.message =
                    "Key created. Send the public key shown here to your host for enrollment."
                        .into();
            }
            Err(error) => self.message = error,
        }
    }

    pub(crate) fn load_join_preview(&mut self) {
        self.settings_page = true;
        self.join_preview = true;
        self.join_address = "192.0.2.42:20042".into();
        self.join_broker = default_broker(&self.join_address);
        self.join_broker_edited = false;
        self.join_identity =
            "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff".into();
        self.join_handle = "your_handle".into();
        self.join_public_key = Some("ab".repeat(32));
        self.message = "Preview only. No key or profile was written.".into();
    }

    fn start_patch(&mut self) {
        if self.screenshot.is_some()
            || self.patch_job.is_some()
            || self.pending.is_some()
            || self.child.is_some()
        {
            return;
        }
        let selection = self.patch_selection();
        self.patch_ready = None;
        self.patch_attempt = Some(selection.clone());
        let (client, _, source) = selection.clone();
        if client.is_empty() || source.manifest_url.is_empty() {
            self.message = "Set the client folder, patch URL and trusted SHA-256 pin first.".into();
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let stopping = cancel.clone();
        let state = self.patch_progress.clone();
        if let Ok(mut progress) = state.lock() {
            *progress = Progress::default();
        }
        let (sender, result) = mpsc::channel();
        self.patch_job = Some(PatchJob {
            selection,
            cancel,
            result,
        });
        self.message = "Checking the selected realm's pinned patch manifest...".into();
        std::thread::spawn(move || {
            let result = patch::repair(Path::new(&client), &source, &stopping, |progress| {
                if let Ok(mut state) = state.lock() {
                    *state = progress.clone();
                }
            });
            let _ = sender.send(result);
        });
    }

    fn poll_patch(&mut self) {
        if let Some(job) = &self.patch_job {
            // Changing the target while downloading must never mark another
            // realm or folder ready. Cancel the old job before starting another.
            if job.selection != self.patch_selection() {
                job.cancel.store(true, Ordering::Relaxed);
            }
            match job.result.try_recv() {
                Ok(result) => {
                    let selection = job.selection.clone();
                    let was_cancelled = job.cancel.load(Ordering::Relaxed);
                    self.patch_job = None;
                    match result {
                        Ok(report) if !was_cancelled && selection == self.patch_selection() => {
                            self.patch_ready = Some(selection);
                            self.message = format!(
                                "Client verified. Repaired {} files ({} bytes downloaded).",
                                report.files_downloaded, report.bytes_downloaded
                            );
                        }
                        Ok(_) => {
                            self.message =
                                "Patching cancelled. Verify the selected client again.".into()
                        }
                        Err(error) => self.message = error,
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.patch_job = None;
                    self.message = "Patch worker stopped. Verify and repair to retry.".into();
                }
                Err(mpsc::TryRecvError::Empty) => (),
            }
        }
        let selection = self.patch_selection();
        if self.patch_job.is_none()
            && !selection.0.is_empty()
            && !selection.2.manifest_url.is_empty()
            && selection.2.manifest_sha256.len() == 64
            && self.patch_attempt.as_ref() != Some(&selection)
        {
            self.start_patch();
        }
    }

    fn patch_progress_ui(&mut self, ui: &mut egui::Ui) {
        let progress = self
            .patch_progress
            .lock()
            .map(|p| p.clone())
            .unwrap_or_default();
        if self.patch_active() {
            ui.horizontal(|ui| {
                let cancel = theme::button(
                    ui,
                    &self.art,
                    "Cancel",
                    Vec2::new(96.0, 36.0),
                    !self.patch_preview,
                    theme::ButtonSkin::Standard,
                );
                self.record_focus(FocusTarget::CancelPatch, &cancel);
                if cancel.clicked() {
                    if let Some(job) = &self.patch_job {
                        job.cancel.store(true, Ordering::Relaxed);
                    }
                }
                let fraction = if progress.bytes_total > 0 {
                    progress.bytes_done as f32 / progress.bytes_total as f32
                } else {
                    0.0
                };
                ui.add(
                    egui::ProgressBar::new(fraction)
                        .desired_width(ui.available_width())
                        .desired_height(24.0)
                        .fill(Color32::from_rgb(83, 211, 159))
                        .text(
                            RichText::new(format!(
                                "{}  {}/{} files | {}/{} bytes | {}",
                                progress.phase,
                                progress.files_done,
                                progress.files_total,
                                progress.bytes_done,
                                progress.bytes_total,
                                progress.current_file
                            ))
                            .size(13.0)
                            .color(Color32::from_rgb(10, 42, 46)),
                        ),
                );
            });
        } else if self.patch_ready.as_ref() == Some(&self.patch_selection()) {
            ui.small("Patch verified. Ready to play.");
        } else if self.servers[self.selected].patch.manifest_url.is_empty() {
            ui.small("Local client. Set a realm patch URL in Settings to receive updates.");
        } else {
            ui.small("Patch required. Use Verify and repair in Settings to retry.");
        }
    }

    fn record_focus(&mut self, target: FocusTarget, response: &egui::Response) {
        self.focus_targets.push((target, response.id));
    }

    fn focus_target(&self, ctx: &egui::Context, target: FocusTarget) {
        if let Some((_, id)) = self.focus_targets.iter().find(|(item, _)| *item == target) {
            ctx.memory_mut(|memory| memory.request_focus(*id));
        }
    }

    fn move_controller_focus(&self, ctx: &egui::Context, forward: bool) {
        let current = ctx.memory(|memory| {
            self.focus_targets
                .iter()
                .position(|(_, id)| memory.has_focus(*id))
        });
        if let Some(next) = next_focus_index(current, self.focus_targets.len(), forward) {
            let id = self.focus_targets[next].1;
            ctx.memory_mut(|memory| memory.request_focus(id));
        }
    }

    fn navigation(&mut self, ctx: &egui::Context) {
        let focused = ctx.input(|i| i.focused);
        let actions = self
            .controller
            .as_mut()
            .map(|pad| pad.poll(self.controller_on, focused))
            .unwrap_or_default();
        if let Some(pad) = &self.controller {
            self.controller_status = if !self.controller_on {
                "Controller navigation off."
            } else if pad.connected {
                "Controller connected."
            } else {
                "No controller connected. Keyboard and mouse are ready."
            }
            .into();
        }
        for action in actions {
            match action {
                Navigation::Previous | Navigation::Next | Navigation::Left | Navigation::Right => {
                    self.move_controller_focus(
                        ctx,
                        matches!(action, Navigation::Next | Navigation::Right),
                    );
                }
                Navigation::Back => {
                    self.settings_page = false;
                    self.focus_target(ctx, FocusTarget::RealmsTab);
                }
                Navigation::Settings => {
                    self.settings_page = !self.settings_page;
                    self.focus_target(
                        ctx,
                        if self.settings_page {
                            FocusTarget::SettingsTab
                        } else {
                            FocusTarget::RealmsTab
                        },
                    );
                }
                Navigation::Confirm => {
                    // egui's existing Space activation handles the focused widget.
                    ctx.input_mut(|i| {
                        for pressed in [true, false] {
                            i.events.push(egui::Event::Key {
                                key: egui::Key::Space,
                                physical_key: None,
                                pressed,
                                repeat: false,
                                modifiers: egui::Modifiers::NONE,
                            });
                        }
                    });
                }
            }
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.settings_page = false;
        }
    }

    fn home(&mut self, ui: &mut egui::Ui) {
        let compact = ui.ctx().content_rect().height() < 650.0;
        theme::heading(
            ui,
            "Your next adventure starts here",
            if compact {
                ""
            } else {
                "Choose a realm, then jump into Free Realms."
            },
        );
        if !compact {
            ui.horizontal(|ui| {
                let (icon, _) = ui.allocate_exact_size(Vec2::splat(27.0), egui::Sense::hover());
                self.art.icon(ui.painter(), 0, icon);
                ui.label(RichText::new("Realms").font(theme::display(22.0)));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.small("Status / players: not connected yet");
                });
            });
        }
        let row_height = if compact { 56.0 } else { 60.0 };
        let row_width = ui.available_width();
        for index in 0..self.servers.len() {
            let server = &self.servers[index];
            let name = if index == self.selected {
                format!("Selected: {}", server.name)
            } else {
                server.name.clone()
            };
            let address = server.address.clone();
            let row = theme::row_button(
                ui,
                &self.art,
                &name,
                &address,
                Vec2::new(row_width, row_height),
            );
            self.record_focus(FocusTarget::RealmRow(index), &row);
            if row.clicked() {
                self.selected = index;
            }
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let add_width = 142.0;
            let field_width = (ui.available_width() - add_width - 8.0).max(160.0);
            let field = theme::text_field(
                ui,
                &self.art,
                &mut self.new_address,
                "Add host:port",
                Vec2::new(field_width, if compact { 38.0 } else { 48.0 }),
                261,
            );
            self.record_focus(FocusTarget::AddAddress, &field);
            let add = theme::button(
                ui,
                &self.art,
                "Add realm",
                Vec2::new(add_width, if compact { 38.0 } else { 48.0 }),
                self.servers.len() < 16,
                theme::ButtonSkin::Standard,
            );
            self.record_focus(FocusTarget::AddRealm, &add);
            if add.clicked() {
                match osfr_launcher::validate_endpoint(&self.new_address) {
                    Ok(()) => {
                        self.servers.push(Server {
                            name: "Custom realm".into(),
                            address: self.new_address.clone(),
                            profile_id: String::new(),
                            patch: PatchSource::default(),
                        });
                        self.selected = self.servers.len() - 1;
                        self.new_address.clear();
                        self.message = "Realm added. Select a saved profile or Local login.".into();
                    }
                    Err(error) => self.message = error,
                }
            }
        });
        self.patch_progress_ui(ui);
        ui.add_space(if compact { 2.0 } else { 8.0 });
        ui.horizontal(|ui| {
            let label = if self.child.is_some() {
                "GAME RUNNING"
            } else if self.pending.is_some() {
                "PREPARING..."
            } else if self.patch_job.is_some() || self.patch_preview {
                "PATCHING..."
            } else {
                "PLAY NOW!"
            };
            let play = theme::button(
                ui,
                &self.art,
                label,
                if compact {
                    Vec2::new(208.0, 52.0)
                } else {
                    Vec2::new(224.0, 64.0)
                },
                self.pending.is_none()
                    && self.child.is_none()
                    && self.patch_allows_play()
                    && (self.screenshot.is_none() || self.focus_play_on_start),
                theme::ButtonSkin::Play,
            );
            self.record_focus(FocusTarget::Play, &play);
            if self.focus_play_on_start && !self.focus_play_requested {
                ui.ctx().memory_mut(|memory| memory.request_focus(play.id));
                self.focus_play_requested = true;
            }
            if self.focus_play_on_start && !play.has_focus() {
                for shape in theme::focus_shapes(play.rect) {
                    ui.painter().add(shape);
                }
            }
            if play.clicked() {
                self.play();
            }
            // T771: runs the server beside the launcher; the first click downloads it from GitHub.
            if let Some(root) = &self.launcher_dir {
                let label = if LocalSettings::packaged(root).missing().is_empty() {
                    "Play locally (no host needed)"
                } else {
                    "Play locally (downloads the server first)"
                };
                let idle = self.pending.is_none()
                    && self.pending_local.is_none()
                    && self.child.is_none()
                    && self.server_job.is_none();
                if ui.add_enabled(idle, egui::Button::new(label)).clicked() {
                    self.play_locally();
                }
            }
            if self.launcher_dir.is_some() {
                ui.vertical(|ui| self.host_ui(ui));
            }
            ui.vertical(|ui| {
                let id = &mut self.servers[self.selected].profile_id;
                let label = profile_label(id, &self.profiles);
                let profile = egui::ComboBox::from_id_salt("login-profile")
                    .selected_text(label)
                    .width(ui.available_width().min(300.0))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(id, String::new(), "Local login");
                        for (value, name) in &self.profiles {
                            ui.selectable_value(id, value.clone(), name);
                        }
                    });
                self.record_focus(FocusTarget::LoginProfile, &profile.response);
                ui.small("Saved key-pair profiles or loopback-only local login.");
            });
        });
    }

    /// L070: "Play locally" hosts friends on this PC's LAN or VPN address. The realm is open (a
    /// friend's first login makes their account) unless `server/state/whitelist.txt` lists handles.
    fn host_ui(&mut self, ui: &mut egui::Ui) {
        let idle = self.pending_local.is_none() && self.local.is_none();
        let toggle = ui.add_enabled(
            idle && self.screenshot.is_none(),
            egui::Checkbox::new(&mut self.host, "Host for friends (LAN or VPN)"),
        );
        if toggle.changed() && self.host {
            self.host_choices = osfr_launcher::local::host_addresses();
            self.host_address = self.host_choices.first().copied();
        }
        if self.host && idle {
            ui.checkbox(
                &mut self.host_internet,
                "On the internet (port forwarding or a server host)",
            );
            if self.host_internet {
                ui.label("Your public IP");
                // The themed field (dark text) under a layout-independent id: opening a touch
                // keyboard resizes the window, and an auto id would lose focus and close it.
                let (art, value) = (&self.art, &mut self.host_public);
                ui.scope_builder(egui::UiBuilder::new().id("host-public-ip"), |ui| {
                    theme::text_field(ui, art, value, "e.g. 203.0.113.7", Vec2::new(260.0, 44.0), 45)
                });
                ui.small(format!(
                    "Forward these ports to this PC on your router (or open them at your server host): UDP {g} and {l}, TCP {g} and {b}. Players join at your public IP, port {l}.",
                    g = osfr_launcher::local::GATEWAY_PORT,
                    l = osfr_launcher::local::LOGIN_PORT,
                    b = osfr_launcher::local::BROKER_PORT,
                ));
            }
        }
        if self.host && idle && !self.host_internet {
            if self.host_choices.len() > 1 {
                let shown = self
                    .host_address
                    .map(|ip| ip.to_string())
                    .unwrap_or_default();
                egui::ComboBox::from_id_salt("host-address")
                    .selected_text(shown)
                    .show_ui(ui, |ui| {
                        for ip in &self.host_choices {
                            ui.selectable_value(&mut self.host_address, Some(*ip), ip.to_string());
                        }
                    });
            }
            match self.host_address {
                Some(ip) => ui.small(format!(
                    "Friends join at {ip}:{}. Windows asks once to let the realm through its firewall: allow private networks.",
                    osfr_launcher::local::LOGIN_PORT
                )),
                None => ui.small("No LAN or VPN address found. Join a network or start Tailscale."),
            };
        }
        if let Some(realm) = &self.local {
            // L075: an internet-hosted realm is joined at the public IP, not this PC's loopback.
            let public = self.host_public.trim().parse::<std::net::IpAddr>().ok();
            let shown = match public {
                Some(ip) if self.host && self.host_internet => {
                    Some(std::net::SocketAddr::new(ip, osfr_launcher::local::LOGIN_PORT))
                }
                _ => (!realm.login.ip().is_loopback()).then_some(realm.login),
            };
            if let Some(address) = shown {
                ui.small(format!(
                    "Hosting at {} (fingerprint {})",
                    address,
                    short_fingerprint(&realm.identity)
                ));
                if ui.button("Copy join details").clicked() {
                    let identity: String =
                        realm.identity.iter().map(|b| format!("{b:02x}")).collect();
                    ui.ctx().copy_text(format!(
                        "Realm address: {address}\nServer identity: {identity}"
                    ));
                    self.message = "Join details copied. Send them to your friends.".into();
                }
            }
        }
    }

    pub(crate) fn join_realm_ui(&mut self, ui: &mut egui::Ui, compact: bool) {
        ui.label(
            RichText::new("Join a realm").font(theme::display(if compact { 22.0 } else { 25.0 })),
        );
        ui.small("Get the realm address from your host, then ask the realm for its identity or paste the one they send.");
        let height = if compact { 34.0 } else { 40.0 };
        let address = theme::text_field(
            ui,
            &self.art,
            &mut self.join_address,
            "Realm address, for example 192.0.2.42:20042",
            Vec2::new(ui.available_width(), height),
            261,
        );
        self.record_focus(FocusTarget::JoinAddress, &address);
        if address.changed() && !self.join_broker_edited {
            self.join_broker = default_broker(&self.join_address);
        }
        let broker = theme::text_field(
            ui,
            &self.art,
            &mut self.join_broker,
            "Login broker, for example 10.0.0.5:20043",
            Vec2::new(ui.available_width(), height),
            261,
        );
        self.record_focus(FocusTarget::JoinBroker, &broker);
        if broker.changed() {
            self.join_broker_edited = !self.join_broker.trim().is_empty();
        }
        let identity = theme::text_field(
            ui,
            &self.art,
            &mut self.join_identity,
            "Server identity (64 hexadecimal digits, from host)",
            Vec2::new(ui.available_width(), height),
            64,
        );
        self.record_focus(FocusTarget::JoinIdentity, &identity);
        if address.changed() || broker.changed() || identity.changed() {
            self.join_public_key = None;
        }
        // L070 trust on first use: the player compares the fingerprint with the host's.
        let ask = egui::Button::new("Ask the realm for its identity");
        if ui.add_enabled(self.screenshot.is_none(), ask).clicked() {
            // ponytail: one bounded exchange (2 s connect, 3 s reply) on the UI thread.
            match auth::fetch_identity(&self.join_broker) {
                Ok(identity) => {
                    self.join_identity = identity.iter().map(|b| format!("{b:02x}")).collect();
                    self.join_public_key = None;
                    self.message = format!(
                        "The realm answered with fingerprint {}. Check it matches the one your host sees.",
                        short_fingerprint(&identity)
                    );
                }
                Err(error) => self.message = auth_error_message(error),
            }
        }
        if let Some(identity) = parse_server_identity(&self.join_identity) {
            ui.small(format!(
                "Short fingerprint to compare: {}",
                short_fingerprint(&identity)
            ));
        } else {
            ui.small("Paste the host's 64-digit server identity to see its fingerprint.");
        }
        let handle = theme::text_field(
            ui,
            &self.art,
            &mut self.join_handle,
            "Your account handle (3-32 letters, digits, _ or -)",
            Vec2::new(ui.available_width(), height),
            32,
        );
        self.record_focus(FocusTarget::JoinHandle, &handle);
        if handle.changed() {
            self.join_public_key = None;
        }
        let create = theme::button(
            ui,
            &self.art,
            "Create my key",
            Vec2::new(178.0, if compact { 38.0 } else { 42.0 }),
            self.screenshot.is_none() && self.servers.len() < 16,
            theme::ButtonSkin::Standard,
        );
        self.record_focus(FocusTarget::CreateKey, &create);
        if create.clicked() {
            self.create_join_profile();
        }
        if let Some(public_key) = self.join_public_key.clone() {
            ui.horizontal(|ui| {
                let field_width = (ui.available_width() - 102.0).max(120.0);
                theme::read_only_text_field(
                    ui,
                    &self.art,
                    &public_key,
                    "Your public key",
                    Vec2::new(field_width, height),
                );
                let copy = theme::button(
                    ui,
                    &self.art,
                    "Copy",
                    Vec2::new(92.0, if compact { 38.0 } else { 42.0 }),
                    true,
                    theme::ButtonSkin::Standard,
                );
                self.record_focus(FocusTarget::CopyPublicKey, &copy);
                if copy.clicked() {
                    copy_public_key(&public_key, |text| ui.ctx().copy_text(text));
                    self.message = "Public key copied. Send it to your host for enrollment.".into();
                }
            });
            ui.small("An open realm makes your account on your first PLAY. If your host lists players, send them this key.");
            if self.join_preview {
                ui.small("Example only: a real profile is saved on this PC and appears in PLAY.");
            } else {
                ui.small("Your profile is saved on this PC and appears in the PLAY login list.");
            }
        }
        ui.small("Login goes to your host's broker on their LAN or VPN address; public addresses are refused.");
        ui.add_space(4.0);
        ui.separator();
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui) {
        let compact = ui.ctx().content_rect().height() < 650.0;
        theme::heading(
            ui,
            "Make yourself at home",
            if compact {
                ""
            } else {
                "Use a separate, writable copy of your supplied client."
            },
        );
        self.join_realm_ui(ui, compact);
        let field_height = if compact { 34.0 } else { 42.0 };
        let client = theme::text_field(
            ui,
            &self.art,
            &mut self.client,
            "Client folder containing FreeRealms.exe",
            Vec2::new(ui.available_width(), field_height),
            32768,
        );
        self.record_focus(FocusTarget::ClientFolder, &client);
        if self.launcher_dir.is_some() {
            ui.horizontal(|ui| {
                if let Some(job) = &self.client_job {
                    if ui.button("Stop the client download").clicked() {
                        job.cancel.store(true, Ordering::Relaxed);
                    }
                } else {
                    let idle = self.screenshot.is_none() && self.pending.is_none() && self.child.is_none();
                    if ui
                        .add_enabled(idle, egui::Button::new("Download the client (about 0.9 GB)"))
                        .clicked()
                    {
                        self.start_client_download();
                    }
                }
                ui.small("From the community host into client/ beside the launcher. It resumes and checks every file.");
            });
        }
        let locale = theme::text_field(
            ui,
            &self.art,
            &mut self.locale,
            "Locale overlay folder (optional)",
            Vec2::new(ui.available_width(), field_height),
            32768,
        );
        self.record_focus(FocusTarget::LocaleFolder, &locale);
        ui.horizontal(|ui| {
            let width = (ui.available_width() - 8.0) / 2.0;
            let field = theme::text_field(
                ui,
                &self.art,
                &mut self.servers[self.selected].patch.manifest_url,
                "Realm patch manifest URL (HTTPS)",
                Vec2::new(width, field_height),
                2048,
            );
            self.record_focus(FocusTarget::PatchUrl, &field);
            let mut mirror = self.servers[self.selected]
                .patch
                .mirror_url
                .clone()
                .unwrap_or_default();
            let field = theme::text_field(
                ui,
                &self.art,
                &mut mirror,
                "Mirror manifest URL (optional)",
                Vec2::new(width, field_height),
                2048,
            );
            self.servers[self.selected].patch.mirror_url = (!mirror.is_empty()).then_some(mirror);
            self.record_focus(FocusTarget::PatchMirror, &field);
        });
        let pin = theme::text_field(
            ui,
            &self.art,
            &mut self.servers[self.selected].patch.manifest_sha256,
            "Trusted manifest SHA-256 (64 hex digits, from realm operator)",
            Vec2::new(ui.available_width(), field_height),
            64,
        );
        self.record_focus(FocusTarget::PatchPin, &pin);
        self.patch_progress_ui(ui);
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Game window").strong());
            let width = ui.add_sized(
                Vec2::new(84.0, 36.0),
                egui::DragValue::new(&mut self.settings.width)
                    .range(640..=3840)
                    .suffix(" px"),
            );
            self.record_focus(FocusTarget::WindowWidth, &width);
            theme::focus_outline(ui, &width);
            ui.label("x");
            let height = ui.add_sized(
                Vec2::new(84.0, 36.0),
                egui::DragValue::new(&mut self.settings.height)
                    .range(480..=2160)
                    .suffix(" px"),
            );
            self.record_focus(FocusTarget::WindowHeight, &height);
            theme::focus_outline(ui, &height);
            for (target, label, width, height) in [
                (FocusTarget::ResolutionSmall, "1024 x 768", 1024, 768),
                (FocusTarget::ResolutionLarge, "1280 x 720", 1280, 720),
            ] {
                let preset = theme::button(
                    ui,
                    &self.art,
                    label,
                    Vec2::new(122.0, 38.0),
                    true,
                    theme::ButtonSkin::Standard,
                );
                self.record_focus(target, &preset);
                if preset.clicked() {
                    self.settings = LaunchSettings { width, height };
                }
            }
        });
        ui.horizontal(|ui| {
            let controller = ui.checkbox(&mut self.controller_on,"Controller navigation")
                .on_hover_text("D-pad/stick: move; A: select; B: back; LB: next; Start: settings. Launcher navigation only.");
            self.record_focus(FocusTarget::ControllerToggle,&controller);
            theme::focus_outline(ui,&controller);
            let verify = theme::button(ui,&self.art,"Verify and repair",Vec2::new(166.0,40.0),
                self.patch_job.is_none() && self.pending.is_none() && self.child.is_none() && self.screenshot.is_none(),theme::ButtonSkin::Standard);
            self.record_focus(FocusTarget::VerifyPatch,&verify);
            if verify.clicked() { self.start_patch(); }
            let back = theme::button(ui,&self.art,"Back to realms",Vec2::new(160.0,40.0),true,theme::ButtonSkin::Standard);
            self.record_focus(FocusTarget::Back,&back);
            if back.clicked() { self.settings_page = false; }
        });
        if !compact {
            ui.small(&self.controller_status);
            ui.small("Settings are session-only. Play keeps configuration and locale backups.");
        }
    }
}

impl eframe::App for Launcher {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        let elapsed = self.started.elapsed();
        let t = self
            .test_vine_growth
            .unwrap_or_else(|| (elapsed.as_secs_f32() / 1.5).clamp(0.0, 1.0));
        let growth = 1.0 - (1.0 - t).powi(3);
        self.art.repaint_vines(growth);
        let growing = self.test_vine_growth.is_none() && elapsed < Duration::from_millis(1500);
        let capture_waiting = self.screenshot.is_some() && !self.screenshot_requested;
        if self.controller_on
            || self.pending.is_some()
            || self.patch_job.is_some()
            || self.server_job.is_some()
            || self.client_job.is_some()
            || growing
            || capture_waiting
        {
            ctx.request_repaint_after(Duration::from_millis(33));
        }
        self.navigation(ctx);
        self.poll_patch();
        self.poll_server();
        self.poll_client();
        if let Some(receiver) = &self.pending_local {
            match receiver.try_recv() {
                Ok(result) => {
                    self.pending_local = None;
                    match result {
                        Ok((child, responder, realm)) => {
                            self.child = Some((child, responder));
                            self.local = Some(realm);
                            self.message =
                                "Playing on this PC. The local realm stops when the client closes."
                                    .into();
                        }
                        Err(error) => self.message = error,
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.pending_local = None;
                    self.message =
                        "The local realm stopped starting. Restart the launcher to try again."
                            .into();
                }
                Err(mpsc::TryRecvError::Empty) => (),
            }
        }
        if let Some(receiver) = &self.pending {
            match receiver.try_recv() {
                Ok(result) => {
                    self.pending = None;
                    match result {
                        Ok(child) => {
                            self.child = Some(child);
                            self.message = "Client started. Keep the launcher open while you play. Backups are in its .friend-launcher-backups folder.".into();
                        }
                        Err(error) => self.message = error,
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.pending = None;
                    self.message =
                        "Client preparation stopped. Restart the launcher to try again.".into();
                }
                Err(mpsc::TryRecvError::Empty) => (),
            }
        }
        if let Some((child, _)) = self.child.as_mut() {
            match child.try_wait() {
                Ok(Some(_)) => {
                    self.child = None;
                    self.local = None;
                    self.message = "The client has closed.".into();
                }
                // Keep the client and its responder: it may still be running.
                Err(_) => self.message = "Could not read client status.".into(),
                Ok(None) => (),
            }
        }
        let rendered_settings = self.settings_page;
        let rendered_realms = self.servers.len();
        self.focus_targets.clear();
        self.art.draw_backdrop(
            &ctx.layer_painter(egui::LayerId::background()),
            ctx.content_rect(),
        );
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(Color32::TRANSPARENT).inner_margin(10))
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    let width = ui.available_width().min(1080.0);
                    ui.set_max_width(width);
                    let compact = ctx.content_rect().height() < 650.0;
                    if compact {
                        ui.spacing_mut().item_spacing.y = 4.0;
                    }
                    let banner_height = (width / 3.0).min(if compact {
                        if rendered_settings { 28.0 } else { 62.0 }
                    } else if rendered_settings {
                        118.0
                    } else {
                        220.0
                    });
                    ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
                        let [logo_w, logo_h] = self.banner.size();
                        let banner_rect = ui
                            .allocate_exact_size(
                                Vec2::new(banner_height * logo_w as f32 / logo_h as f32, banner_height),
                                egui::Sense::hover(),
                            )
                            .0;
                        ui.painter().image(
                            self.banner.id(),
                            banner_rect,
                            theme::unit_uv(),
                            Color32::WHITE,
                        );
                    });
                    ui.add_space(2.0);
                    let panel_slot = ui.painter().add(egui::Shape::Noop);
                    let panel = theme::panel()
                        .inner_margin(egui::Margin {left:32,right:32,top:12,bottom:12})
                        .show(ui, |ui| {
                            ui.set_min_width(ui.available_width());
                            ui.horizontal(|ui| {
                                let realms = theme::button(
                                    ui,
                                    &self.art,
                                    "Realms",
                                    Vec2::new(132.0, if compact { 40.0 } else { 48.0 }),
                                    true,
                                    theme::ButtonSkin::Tab { selected: !rendered_settings },
                                );
                                self.record_focus(FocusTarget::RealmsTab, &realms);
                                self.art.icon(
                                    ui.painter(),
                                    0,
                                    egui::Rect::from_center_size(
                                        egui::pos2(realms.rect.left() + 23.0, realms.rect.center().y),
                                        Vec2::new(22.0, 24.0),
                                    ),
                                );
                                if realms.clicked() {
                                    self.settings_page = false;
                                }
                                let settings = theme::button(
                                    ui,
                                    &self.art,
                                    "Settings",
                                    Vec2::new(142.0, if compact { 40.0 } else { 48.0 }),
                                    true,
                                    theme::ButtonSkin::Tab { selected: rendered_settings },
                                );
                                self.record_focus(FocusTarget::SettingsTab, &settings);
                                self.art.icon(
                                    ui.painter(),
                                    1,
                                    egui::Rect::from_center_size(
                                        egui::pos2(settings.rect.left() + 23.0, settings.rect.center().y),
                                        Vec2::new(21.0, 24.0),
                                    ),
                                );
                                if settings.clicked() {
                                    self.settings_page = true;
                                }
                            });
                            ui.add_space(if compact { 1.0 } else { 5.0 });
                            if rendered_settings {
                                self.settings_ui(ui);
                            } else {
                                self.home(ui);
                            }
                            ui.add_space(if compact { 2.0 } else { 8.0 });
                            theme::plank(ui, &self.art, &self.message, if compact { 44.0 } else { 48.0 });
                            if self.directx_missing {
                                ui.hyperlink_to(
                                    "Get the DirectX End-User Runtime from Microsoft",
                                    osfr_launcher::DIRECTX_RUNTIME_URL,
                                );
                            }
                        });
                    ui.painter().set(
                        panel_slot,
                        theme::panel_shape(&self.art, panel.response.rect),
                    );
                    let actual: Vec<_> = self.focus_targets.iter().map(|(target, _)| *target).collect();
                    debug_assert!(focus_sequence_matches(
                        &actual,
                        rendered_settings,
                        rendered_realms,
                        self.patch_active(),
                        self.join_public_key.is_some(),
                    ));
                    if !compact {
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new("A non-commercial fan revival | not affiliated with Daybreak or Sony.")
                                .size(12.0)
                                .color(Color32::from_rgb(225, 236, 244)),
                        );
                    }
                });
            });
        if let Some(path) = &self.screenshot {
            let capture_delay = if self.test_vine_growth.is_some() {
                600
            } else {
                1700
            };
            if !self.screenshot_requested
                && self.started.elapsed() > Duration::from_millis(capture_delay)
            {
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
                self.screenshot_requested = true;
            }
            let screenshot = ctx.input(|i| {
                i.events.iter().find_map(|e| {
                    if let egui::Event::Screenshot { image, .. } = e {
                        Some(image.clone())
                    } else {
                        None
                    }
                })
            });
            if let Some(screenshot) = screenshot {
                match save_capture(path, &screenshot) {
                    Ok(()) => {
                        self.capture_ok.set(true);
                        log::info!(
                            "Screenshot saved: {}x{}",
                            screenshot.width(),
                            screenshot.height()
                        );
                    }
                    Err(error) => {
                        log::error!("Launcher screenshot failed: {error}");
                        eprintln!("Launcher screenshot failed: {error}");
                    }
                }
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            } else if self.started.elapsed() > Duration::from_secs(15) {
                log::error!("Launcher screenshot timed out");
                eprintln!("Launcher screenshot timed out.");
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }
}

impl Drop for Launcher {
    fn drop(&mut self) {
        if let Some(job) = &self.patch_job {
            job.cancel.store(true, Ordering::Relaxed);
        }
        if let Some(job) = &self.server_job {
            job.cancel.store(true, Ordering::Relaxed);
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let raw_args: Vec<_> = std::env::args_os().skip(1).collect();
    if let Some(result) =
        osfr_launcher::key_command(raw_args.iter().map(|arg| arg.to_str().unwrap_or("")))
    {
        match result {
            Ok(output) => {
                println!("{output}");
                return Ok(());
            }
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(2);
            }
        }
    }
    diagnostics::init()?;
    let mut screenshot = None;
    let mut settings_page = false;
    let mut focus_play = false;
    let mut test_vine_growth = None;
    let mut patch_preview = false;
    let mut join_preview = false;
    let mut window_size = [940.0, 790.0];
    let mut args = raw_args.into_iter();
    while let Some(arg) = args.next() {
        if arg == "--screenshot" {
            screenshot = args.next().map(PathBuf::from);
            if screenshot.is_none() {
                return Err("--screenshot requires a PNG output path.".into());
            }
        } else if arg == "--settings-page" {
            settings_page = true;
        } else if arg == "--test-focus-play" {
            focus_play = true;
        } else if arg == "--test-vine-growth-ms" {
            let millis = args
                .next()
                .and_then(|s| s.into_string().ok())
                .ok_or("--test-vine-growth-ms requires a value from 0 to 1500.")?
                .parse::<u64>()?;
            if millis > 1500 {
                return Err("--test-vine-growth-ms requires a value from 0 to 1500.".into());
            }
            test_vine_growth = Some(millis as f32 / 1500.0);
        } else if arg == "--test-patch-progress" {
            patch_preview = true;
        } else if arg == "--test-join-flow" {
            join_preview = true;
        } else if arg == "--window-size" {
            let size = args
                .next()
                .and_then(|s| s.into_string().ok())
                .ok_or("--window-size requires WIDTHxHEIGHT in logical points.")?;
            let (width, height) = size
                .split_once('x')
                .ok_or("--window-size requires WIDTHxHEIGHT in logical points.")?;
            let settings = LaunchSettings {
                width: width.parse()?,
                height: height.parse()?,
            };
            settings.validate()?;
            window_size = [settings.width as f32, settings.height as f32];
        } else {
            return Err(
                "Usage: osfr-launcher [--settings-page] [--window-size WIDTHxHEIGHT] [--screenshot existing-folder/output.png]"
                    .into(),
            );
        }
    }
    if patch_preview && screenshot.is_none() {
        return Err(
            "--test-patch-progress requires --screenshot; it performs no downloads.".into(),
        );
    }
    if join_preview && screenshot.is_none() {
        return Err("--test-join-flow requires --screenshot; it creates no profile or key.".into());
    }
    if let Some(path) = &screenshot {
        if path.extension().is_none_or(|ext| ext != "png")
            || !path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."))
                .is_dir()
            || path.exists()
        {
            return Err("Screenshot needs an unused PNG path in an existing output folder.".into());
        }
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(window_size)
            .with_min_inner_size([640.0, 480.0])
            .with_title("Free Realms Expanded - Launcher"),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    let capture_requested = screenshot.is_some();
    let capture_ok = Rc::new(Cell::new(false));
    let captured = capture_ok.clone();
    let result = eframe::run_native(
        "Free Realms Expanded - Launcher",
        options,
        Box::new(move |cc| {
            let mut launcher = Launcher::new(
                cc,
                screenshot,
                settings_page,
                captured,
                focus_play,
                test_vine_growth,
            )
            .map_err(std::io::Error::other)?;
            launcher.patch_preview = patch_preview;
            launcher.check_server_on_start();
            if join_preview {
                launcher.load_join_preview();
            }
            if patch_preview {
                if let Ok(mut progress) = launcher.patch_progress.lock() {
                    *progress = Progress {
                        files_done: 12,
                        files_total: 22,
                        bytes_done: 5242880,
                        bytes_total: 12582912,
                        current_file: "data/client.pack".into(),
                        phase: "Downloading",
                    };
                }
                launcher.message =
                    "Updating client files. PLAY unlocks after SHA-256 verification.".into();
            }
            Ok(Box::new(launcher))
        }),
    );
    if let Err(error) = &result {
        log::error!("run_native failed: {error}");
    } else {
        log::info!("run_native returned successfully");
    }
    result?;
    if capture_requested && !capture_ok.get() {
        return Err("Launcher closed without saving its screenshot.".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// First use: Play locally without `server/` downloads it and starts nothing. Here GitHub is
    /// a closed loopback port, so the download fails and the message says so.
    #[test]
    fn play_locally_without_the_server_downloads_it_and_never_launches() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = Launcher::new(
            &cc,
            None,
            false,
            Rc::new(Cell::new(false)),
            false,
            Some(1.0),
        )
        .unwrap();
        app.screenshot = None;
        let dir = std::env::temp_dir().join(format!("osfr-first-use-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        app.launcher_dir = Some(dir.clone());
        let closed = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        app.release = ReleaseSource {
            api_url: format!("http://{closed}/latest"),
            download_base: format!("http://{closed}/download"),
            ..Default::default()
        };
        app.play_locally();
        assert!(
            app.server_job.is_some(),
            "the first click starts the download"
        );
        app.play_locally();
        assert!(
            app.message.contains("Try again in a moment"),
            "a second click waits: {}",
            app.message
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        while app.server_job.is_some() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
            app.poll_server();
        }
        assert!(app.server_job.is_none());
        assert!(
            app.message.contains("Could not download the local server"),
            "{}",
            app.message
        );
        assert!(app.pending_local.is_none() && app.local.is_none() && app.child.is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn server_job_messages_cover_install_update_offline_and_current() {
        let outcome = |installed| release::Outcome {
            tag: "v2".into(),
            installed,
            files_downloaded: 0,
            bytes_downloaded: 0,
        };
        let mut progress = Progress::default();
        let say = |had, progress: &Progress, done: Option<Result<release::Outcome, String>>| {
            server_message(had, progress, done.as_ref())
        };
        progress.phase = "Update available";
        assert_eq!(
            say(true, &progress, None).unwrap(),
            "A server update is available."
        );
        progress.phase = "Downloading";
        (progress.files_done, progress.files_total) = (1, 4);
        (progress.bytes_done, progress.bytes_total) = (1 << 20, 4 << 20);
        assert_eq!(
            say(true, &progress, None).unwrap(),
            "A server update is available. Downloading: 1/4 files, 1.0 MB of 4.0 MB"
        );
        assert!(
            say(false, &progress, None)
                .unwrap()
                .starts_with("Downloading the local server:")
        );
        assert_eq!(say(true, &progress, Some(Ok(outcome(false)))), None);
        assert!(
            say(true, &progress, Some(Ok(outcome(true))))
                .unwrap()
                .contains("updated to v2")
        );
        assert!(
            say(false, &progress, Some(Ok(outcome(true))))
                .unwrap()
                .contains("v2 installed")
        );
        let offline = say(
            true,
            &progress,
            Some(Err("Patch connection failed.".into())),
        )
        .unwrap();
        assert!(offline.contains("Using the installed server"), "{offline}");
        let first = say(
            false,
            &progress,
            Some(Err("Patch connection failed.".into())),
        )
        .unwrap();
        assert!(
            first.starts_with("Could not download the local server"),
            "{first}"
        );
    }

    #[test]
    fn patch_ui_routes_repair_and_gates_play_for_the_verified_selection() {
        use std::io::{BufRead, BufReader, Write};
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = Launcher::new(
            &cc,
            None,
            false,
            Rc::new(Cell::new(false)),
            false,
            Some(1.0),
        )
        .unwrap();
        let folder = std::env::temp_dir()
            .join("osfr-launcher-test-work")
            .join(format!("ui-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let file = b"verified fixture";
        let manifest = serde_json::to_vec(&serde_json::json!({"version":1,"files":[{
            "path":"a.bin","size":file.len(),"sha256":patch::sha256(file).unwrap()
        }]}))
        .unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        app.client = folder.to_string_lossy().into_owned();
        app.servers[0].patch = PatchSource {
            manifest_url: format!("http://{}/manifest.json", listener.local_addr().unwrap()),
            manifest_sha256: patch::sha256(&manifest).unwrap(),
            mirror_url: None,
        };
        let server = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut count = 0;
            while count < 2 && Instant::now() < deadline {
                match listener.accept() {
                    Ok((mut socket, _)) => {
                        socket
                            .set_read_timeout(Some(Duration::from_secs(2)))
                            .unwrap();
                        let mut reader = BufReader::new(socket.try_clone().unwrap());
                        loop {
                            let mut line = String::new();
                            if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                                break;
                            }
                        }
                        let body = if count == 0 {
                            manifest.as_slice()
                        } else {
                            file
                        };
                        write!(
                            socket,
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        )
                        .unwrap();
                        socket.write_all(body).unwrap();
                        count += 1;
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5))
                    }
                    Err(e) => panic!("{e}"),
                }
            }
            assert_eq!(count, 2);
        });
        assert!(!app.patch_allows_play());
        app.play();
        assert!(
            app.pending.is_none(),
            "PLAY must not reach client preparation before patch verification"
        );
        app.poll_patch(); // Automatic main-screen route, also used by update().
        assert!(app.patch_job.is_some());
        assert!(!app.patch_allows_play());
        let deadline = Instant::now() + Duration::from_secs(10);
        while app.patch_job.is_some() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
            app.poll_patch();
        }
        server.join().unwrap();
        assert!(app.patch_allows_play(), "{}", app.message);
        assert_eq!(std::fs::read(folder.join("a.bin")).unwrap(), file);
        assert_eq!(app.patch_progress.lock().unwrap().phase, "Ready");
        app.client.push_str("-other");
        assert!(
            !app.patch_allows_play(),
            "verification must belong to this client folder"
        );
        app.client = folder.to_string_lossy().into_owned();
        app.servers[0].patch.manifest_sha256 = "00".repeat(32);
        assert!(
            !app.patch_allows_play(),
            "changed pin must invalidate readiness"
        );
        std::fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    fn play_profile_login_calls_authenticator_and_maps_each_error_message() {
        let profile = auth::Profile::new(
            "fixture",
            "Fixture realm",
            "127.0.0.1:20043",
            [42; 32],
            "acct_001",
        )
        .unwrap();
        let cases = [
            (
                AuthError::ServerUnreachable,
                "Server unreachable. Check the broker or secure tunnel.",
            ),
            (
                AuthError::WrongServerIdentity,
                "Wrong server identity. Check the trusted server profile.",
            ),
            (
                AuthError::Refused,
                "Login refused. Another player may hold that handle, or the realm only takes listed players: make a profile with another handle, or send the host your public key from Settings.",
            ),
            (
                AuthError::TryLater,
                "Server closed the login request. Try again later.",
            ),
            (
                AuthError::Timeout,
                "Login timed out. Check the broker or try again later.",
            ),
            (
                AuthError::InvalidReply,
                "Server returned an invalid login reply.",
            ),
            (
                AuthError::InvalidProfile,
                "Invalid server profile. Check the trusted profile settings.",
            ),
            (
                AuthError::KeyStorage,
                "Cannot access the private profile files. Check their permissions.",
            ),
            (
                AuthError::RandomUnavailable,
                "The operating system could not generate a secure key.",
            ),
        ];
        for (error, message) in cases {
            let mut called = false;
            let mut loaded = false;
            let login = RealmLogin {
                profile_id: "fixture".into(),
                broker: None,
            };
            let result = login.session_with(
                "127.0.0.1:20042",
                |id| {
                    loaded = true;
                    assert_eq!(id, "fixture");
                    Ok(profile.clone())
                },
                |selected| {
                    called = true;
                    assert_eq!(selected, &profile);
                    Err(error)
                },
            );
            assert!(loaded, "PLAY must load the selected realm's profile");
            assert!(called, "PLAY must call the selected profile's login");
            match result {
                Err(error) => assert_eq!(error, message),
                Ok(_) => panic!("PLAY unexpectedly accepted an authentication error"),
            }
        }
    }

    #[test]
    fn controller_focus_walk_covers_every_home_and_settings_control() {
        let cases = [
            (
                false,
                2,
                false,
                false,
                vec![
                    FocusTarget::RealmsTab,
                    FocusTarget::SettingsTab,
                    FocusTarget::RealmRow(0),
                    FocusTarget::RealmRow(1),
                    FocusTarget::AddAddress,
                    FocusTarget::AddRealm,
                    FocusTarget::Play,
                    FocusTarget::LoginProfile,
                ],
            ),
            (
                false,
                2,
                true,
                false,
                vec![
                    FocusTarget::RealmsTab,
                    FocusTarget::SettingsTab,
                    FocusTarget::RealmRow(0),
                    FocusTarget::RealmRow(1),
                    FocusTarget::AddAddress,
                    FocusTarget::AddRealm,
                    FocusTarget::CancelPatch,
                    FocusTarget::Play,
                    FocusTarget::LoginProfile,
                ],
            ),
            (
                true,
                2,
                false,
                false,
                vec![
                    FocusTarget::RealmsTab,
                    FocusTarget::SettingsTab,
                    FocusTarget::JoinAddress,
                    FocusTarget::JoinBroker,
                    FocusTarget::JoinIdentity,
                    FocusTarget::JoinHandle,
                    FocusTarget::CreateKey,
                    FocusTarget::ClientFolder,
                    FocusTarget::LocaleFolder,
                    FocusTarget::PatchUrl,
                    FocusTarget::PatchMirror,
                    FocusTarget::PatchPin,
                    FocusTarget::WindowWidth,
                    FocusTarget::WindowHeight,
                    FocusTarget::ResolutionSmall,
                    FocusTarget::ResolutionLarge,
                    FocusTarget::ControllerToggle,
                    FocusTarget::VerifyPatch,
                    FocusTarget::Back,
                ],
            ),
            (
                true,
                2,
                true,
                true,
                vec![
                    FocusTarget::RealmsTab,
                    FocusTarget::SettingsTab,
                    FocusTarget::JoinAddress,
                    FocusTarget::JoinBroker,
                    FocusTarget::JoinIdentity,
                    FocusTarget::JoinHandle,
                    FocusTarget::CreateKey,
                    FocusTarget::CopyPublicKey,
                    FocusTarget::ClientFolder,
                    FocusTarget::LocaleFolder,
                    FocusTarget::PatchUrl,
                    FocusTarget::PatchMirror,
                    FocusTarget::PatchPin,
                    FocusTarget::CancelPatch,
                    FocusTarget::WindowWidth,
                    FocusTarget::WindowHeight,
                    FocusTarget::ResolutionSmall,
                    FocusTarget::ResolutionLarge,
                    FocusTarget::ControllerToggle,
                    FocusTarget::VerifyPatch,
                    FocusTarget::Back,
                ],
            ),
        ];
        for (settings, realm_count, patch_active, public_key_ready, expected) in cases {
            assert_eq!(
                focus_order(settings, realm_count, patch_active, public_key_ready),
                expected
            );
            let mut broken = expected.clone();
            broken.pop();
            assert!(!focus_sequence_matches(
                &broken,
                settings,
                realm_count,
                patch_active,
                public_key_ready
            ));
            let mut reordered = expected.clone();
            reordered.swap(0, 1);
            assert!(!focus_sequence_matches(
                &reordered,
                settings,
                realm_count,
                patch_active,
                public_key_ready
            ));
            if settings {
                let mut missing_new_control = expected.clone();
                missing_new_control.retain(|target| *target != FocusTarget::CreateKey);
                assert!(!focus_sequence_matches(
                    &missing_new_control,
                    settings,
                    realm_count,
                    patch_active,
                    public_key_ready
                ));
            }

            let ctx = egui::Context::default();
            let mut actual = Vec::new();
            let mut path = String::new();
            let mut locale = String::new();
            let mut width = 1024;
            let mut height = 768;
            let mut controller = false;
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        Vec2::new(900.0, 700.0),
                    )),
                    focused: true,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        for target in &expected {
                            let response = match target {
                                FocusTarget::AddAddress
                                | FocusTarget::JoinAddress
                                | FocusTarget::JoinBroker
                                | FocusTarget::JoinIdentity
                                | FocusTarget::JoinHandle
                                | FocusTarget::LoginProfile
                                | FocusTarget::ClientFolder => {
                                    ui.add(egui::TextEdit::singleline(&mut path))
                                }
                                FocusTarget::LocaleFolder => {
                                    ui.add(egui::TextEdit::singleline(&mut locale))
                                }
                                FocusTarget::WindowWidth => {
                                    ui.add(egui::DragValue::new(&mut width))
                                }
                                FocusTarget::WindowHeight => {
                                    ui.add(egui::DragValue::new(&mut height))
                                }
                                FocusTarget::ControllerToggle => {
                                    ui.checkbox(&mut controller, "Controller navigation")
                                }
                                target => ui.add(egui::Button::new(format!("{target:?}"))),
                            };
                            actual.push((*target, response.id));
                        }
                    });
                },
            );
            assert_eq!(
                actual.iter().map(|(target, _)| *target).collect::<Vec<_>>(),
                expected
            );
            let mut current = None;
            for index in 0..actual.len() {
                let next = next_focus_index(current, actual.len(), true).unwrap();
                assert_eq!(next, index);
                ctx.memory_mut(|memory| memory.request_focus(actual[next].1));
                assert!(ctx.memory(|memory| memory.has_focus(actual[next].1)));
                current = Some(next);
            }
            let previous = next_focus_index(Some(0), actual.len(), false).unwrap();
            assert_eq!(actual[previous].0, *expected.last().unwrap());
        }
    }

    #[test]
    fn patch_cancel_is_rendered_only_while_patch_is_active() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = Launcher::new(
            &cc,
            None,
            false,
            Rc::new(Cell::new(false)),
            false,
            Some(1.0),
        )
        .unwrap();
        let frame = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                Vec2::new(900.0, 120.0),
            )),
            ..Default::default()
        };
        let _ = ctx.run(frame.clone(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| app.patch_progress_ui(ui));
        });
        assert!(
            !app.focus_targets
                .iter()
                .any(|(target, _)| *target == FocusTarget::CancelPatch)
        );

        app.focus_targets.clear();
        app.patch_preview = true;
        let _ = ctx.run(frame, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| app.patch_progress_ui(ui));
        });
        assert!(
            app.focus_targets
                .iter()
                .any(|(target, _)| *target == FocusTarget::CancelPatch)
        );
    }

    #[test]
    fn bundled_banner_fits_startup_texture_limit() {
        // CreationContext still has egui's default limit before its first frame.
        let ctx = egui::Context::default();
        let banner = load_banner(&ctx).unwrap();
        let limit = ctx.input(|input| input.max_texture_side);
        let [width, height] = banner.size();
        assert!(width > 0 && height > 0 && width <= limit && height <= limit);
        // Redesign 2026-10-30: the wordmark (869x300) replaces the 3:1 banner.
        assert!((width as f32 / height as f32 - 869.0 / 300.0).abs() < 0.01);
    }

    #[test]
    fn capture_refuses_existing_files() {
        let path = std::env::temp_dir().join(format!("osfr-capture-{}.png", std::process::id()));
        let _owned = File::options()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        let image = egui::ColorImage::filled([2, 2], Color32::GREEN);
        assert!(save_capture(&path, &image).is_err());
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 0);
        drop(_owned);
        std::fs::remove_file(path).unwrap();
    }
}
