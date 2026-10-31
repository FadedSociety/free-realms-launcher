//! The launcher window as the approved redesign mockup (2026-10-30, v0.3.1): a navy top bar with the
//! small logo and plank tabs; Play = news cards beside a vine-framed hero scene with the play dock on it;
//! Host = mode cards and fields beside a setup check; Settings in the same purple panel.
use super::*;
use theme::Piece;

const INK: Color32 = theme::DARK_INK;
const TEAL_INK: Color32 = Color32::from_rgb(14, 95, 125);
const CHIP_H: f32 = 40.0;
const PLAY_H: f32 = 78.0;
const NEWS_H: f32 = 32.0;
const GAP: f32 = 10.0;
/// Inside a purple panel: its rim ends 26 points in (52 px on panel.png at the 0.5 art scale), plus air.
const PANEL_PAD: f32 = 36.0;
/// The play dock: realm chip, PLAY and the latest-news pill.
const DOCK_H: f32 = CHIP_H + GAP + PLAY_H + GAP + NEWS_H;

impl Launcher {
    pub(super) fn redesigned_ui(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("rd-top")
            .frame(egui::Frame::new().inner_margin(egui::Margin {
                left: 12,
                right: 12,
                top: 10,
                bottom: 4,
            }))
            .show(ctx, |ui| self.top_bar(ui));
        egui::TopBottomPanel::bottom("rd-status")
            .frame(egui::Frame::new().inner_margin(egui::Margin {
                left: 18,
                right: 18,
                top: 2,
                bottom: 8,
            }))
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    let guide = theme::button(
                        ui,
                        &self.art,
                        "Get started",
                        Vec2::new(150.0, 44.0),
                        true,
                        theme::ButtonSkin::Standard,
                    );
                    if guide.clicked() {
                        self.news_open = Some(guide_item());
                    }
                    // The intro line is what the guide explains; real news (errors, progress) still shows.
                    if self.message != INTRO {
                        ui.label(RichText::new(&self.message).color(Color32::WHITE));
                    }
                    if self.directx_missing {
                        ui.hyperlink_to(
                            "Get the DirectX End-User Runtime",
                            osfr_launcher::DIRECTX_RUNTIME_URL,
                        );
                    }
                });
            });
        egui::CentralPanel::default()
            .frame(egui::Frame::new().inner_margin(egui::Margin {
                left: 14,
                right: 14,
                top: 6,
                bottom: 4,
            }))
            .show(ctx, |ui| {
                if self.settings_page {
                    let rect = ui.available_rect_before_wrap();
                    ui.painter().add(theme::panel_shape(&self.art, rect));
                    ui.scope_builder(
                        egui::UiBuilder::new().max_rect(rect.shrink(PANEL_PAD)),
                        |ui| {
                            egui::ScrollArea::vertical().show(ui, |ui| self.settings_ui(ui));
                        },
                    );
                } else if self.servers_page {
                    self.servers_screen(ui);
                } else if self.host_page {
                    self.host_screen(ui);
                } else {
                    self.play_screen(ui);
                }
            });
        self.notes_window(ctx);
        self.join_window(ctx);
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let (rect, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 96.0), egui::Sense::hover());
        self.art.piece(ui.painter(), Piece::TopBar, rect);
        // top_bar.png's channel runs from below its upper rail (47 px down) to above its lip (18 px up):
        // 23.5 and 9 points at the 0.5 art scale. Logo and tabs centre in it.
        let inner = egui::Rect::from_min_max(
            rect.min + Vec2::new(26.0, 23.5),
            rect.max - Vec2::new(26.0, 9.0),
        );
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(inner)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
            |ui| {
                let [w, h] = self.banner.size();
                let (logo, _) = ui.allocate_exact_size(
                    Vec2::new(52.0 * w as f32 / h as f32, 52.0),
                    egui::Sense::hover(),
                );
                ui.painter()
                    .image(self.banner.id(), logo, theme::unit_uv(), Color32::WHITE);
                ui.add_space(14.0);
                let current = if self.settings_page {
                    2
                } else if self.servers_page {
                    3
                } else if self.host_page {
                    1
                } else {
                    0
                };
                for (page, label) in [(0, "Play"), (1, "Host"), (3, "Servers"), (2, "Settings")] {
                    let enabled = page != 1 || self.launcher_dir.is_some();
                    let tab = theme::button(
                        ui,
                        &self.art,
                        label,
                        Vec2::new(if page >= 2 { 150.0 } else { 124.0 }, 52.0),
                        enabled,
                        theme::ButtonSkin::Tab {
                            selected: current == page,
                        },
                    );
                    match page {
                        0 => self.record_focus(FocusTarget::RealmsTab, &tab),
                        2 => self.record_focus(FocusTarget::SettingsTab, &tab),
                        _ => {}
                    }
                    if tab.clicked() {
                        self.settings_page = page == 2;
                        self.host_page = page == 1;
                        self.servers_page = page == 3;
                        if page == 3 {
                            self.refresh_servers(ui.ctx());
                        }
                    }
                }
            },
        );
    }

    fn play_screen(&mut self, ui: &mut egui::Ui) {
        let full = ui.available_rect_before_wrap();
        ui.allocate_rect(full, egui::Sense::hover());
        if full.width() >= 760.0 {
            let left = (full.width() * 0.33).clamp(280.0, 400.0);
            self.news_panel(
                ui,
                egui::Rect::from_min_size(full.min, Vec2::new(left, full.height())),
            );
            self.hero_window(
                ui,
                egui::Rect::from_min_max(
                    egui::pos2(full.min.x + left + 16.0, full.min.y),
                    full.max,
                ),
            );
        } else {
            self.hero_window(ui, full);
        }
    }

    fn news_panel(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        ui.painter().add(theme::panel_shape(&self.art, rect));
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(rect.shrink(PANEL_PAD)),
            |ui| {
                panel_title(ui, "News & updates");
                ui.add_space(6.0);
                if self.news.is_empty() {
                    ui.label(
                        RichText::new("News shows here when you are online.").color(Color32::WHITE),
                    );
                }
                // The cards scroll between the title and the GitHub bubble on the panel's own scroll bar.
                let list_h = ui.available_height() - 36.0 - 12.0;
                ui.scope(|ui| {
                    themed_scroll(ui);
                    egui::ScrollArea::vertical()
                        .max_height(list_h)
                        .auto_shrink([false, true])
                        .show(ui, |ui| self.news_cards(ui));
                });
                if self.patch_active() {
                    self.patch_progress_ui(ui);
                }
                self.github_bubble(ui);
            },
        );
    }

    /// Look up this PC's public IP in the background; the Host tab's field fills in when it answers.
    fn lookup_public_ip(&mut self, ctx: &egui::Context) {
        if self.ip_rx.is_some() || self.screenshot.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(osfr_launcher::release::public_ip().ok());
            ctx.request_repaint();
        });
        self.ip_rx = Some(rx);
    }

    /// v0.3.2: the choices players expect back next time, in `%APPDATA%/FreeRealmsExpanded/launcher.json`.
    /// v0.3.4: through auth, so the folder is made private (and repaired) before launcher.json.
    fn prefs_path() -> Option<std::path::PathBuf> {
        auth::ensure_app_directory()
            .ok()
            .map(|app| app.join("launcher.json"))
    }

    fn prefs_json(&self) -> String {
        serde_json::json!({
            "client": self.client.trim(),
            "host": self.host,
            "host_internet": self.host_internet,
            "host_public": self.host_public.trim(),
            "list_realm": self.list_realm,
            "realm_name": self.realm_name.trim(),
            "realm_description": self.realm_description.trim(),
            "favorites": self.favorites,
            "directory_url": self.directory_url.trim(),
        })
        .to_string()
    }

    pub(super) fn load_prefs(&mut self) {
        if self.screenshot.is_some() {
            return;
        }
        let Some(text) = Self::prefs_path().and_then(|p| std::fs::read_to_string(p).ok()) else {
            return;
        };
        let Ok(prefs) = serde_json::from_str::<serde_json::Value>(&text) else {
            return;
        };
        let text_of = |key: &str| {
            prefs
                .get(key)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_owned()
        };
        self.client = text_of("client");
        // v0.3.4: a moved launcher folder takes its own client/ when the saved one is gone.
        if let Some(beside) = self.launcher_dir.as_ref().map(|root| root.join("client")) {
            let beside = beside.display().to_string();
            if !client_folder_ok(&self.client) && client_folder_ok(&beside) {
                self.client = beside;
            }
        }
        self.host_public = text_of("host_public");
        self.list_realm = prefs
            .get("list_realm")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        self.realm_name = text_of("realm_name");
        self.realm_description = text_of("realm_description");
        self.directory_url = text_of("directory_url");
        self.favorites = prefs
            .get("favorites")
            .and_then(|v| v.as_array())
            .map(|ids| {
                ids.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        self.host = prefs.get("host").and_then(|v| v.as_bool()).unwrap_or(false);
        self.host_internet = self.host
            && prefs
                .get("host_internet")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
        if self.host && !self.host_internet {
            self.host_choices = osfr_launcher::local::host_addresses();
            self.host_address = self.host_choices.first().copied();
        }
        self.prefs_saved = self.prefs_json();
    }

    /// Write the remembered choices when they change (a small file, replaced in one rename).
    pub(super) fn save_prefs(&mut self) {
        if self.screenshot.is_some() {
            return;
        }
        let json = self.prefs_json();
        if json == self.prefs_saved {
            return;
        }
        self.prefs_saved = json.clone();
        if let Some(path) = Self::prefs_path() {
            let temporary = path.with_extension("json.tmp");
            let _ = std::fs::write(&temporary, json)
                .and_then(|()| std::fs::rename(&temporary, &path));
        }
    }

    fn news_cards(&mut self, ui: &mut egui::Ui) {
        for item in self.news.clone() {
            let (card, response) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), 70.0), egui::Sense::click());
            self.art.piece(ui.painter(), Piece::Card, card);
            let ring = egui::Rect::from_center_size(
                egui::pos2(card.left() + 36.0, card.center().y),
                Vec2::splat(46.0),
            );
            self.art.icon(ui.painter(), 2, ring.shrink(9.0));
            self.art.piece(ui.painter(), Piece::ThumbRing, ring);
            let text = egui::pos2(card.left() + 68.0, card.top() + 14.0);
            let clip = ui.painter().with_clip_rect(card.shrink(10.0));
            let title = short_title(&item.title);
            let tag = format!("{} \u{b7} {}", title.to_uppercase(), item.date);
            clip.text(
                text,
                egui::Align2::LEFT_TOP,
                tag,
                egui::FontId::proportional(11.5),
                TEAL_INK,
            );
            let line = if item.headline.is_empty() {
                title
            } else {
                item.headline.clone()
            };
            let mut job = egui::text::LayoutJob::simple_singleline(
                line,
                egui::FontId::proportional(15.0),
                INK,
            );
            job.wrap = egui::text::TextWrapping::truncate_at_width(card.right() - text.x - 16.0);
            clip.galley(text + Vec2::new(0.0, 18.0), clip.layout_job(job), INK);
            if response.clicked() {
                self.news_open = Some(item);
            }
            response.on_hover_cursor(egui::CursorIcon::PointingHand);
            ui.add_space(6.0);
        }
    }

    /// A bubble sized to its contents, pinned to the bottom of the panel: the launcher's GitHub page.
    fn github_bubble(&mut self, ui: &mut egui::Ui) {
        let label = ui
            .painter()
            .layout_no_wrap("GitHub".into(), theme::display(17.0), INK);
        let size = Vec2::new(14.0 + 20.0 + 8.0 + label.size().x + 16.0, 36.0);
        let area = ui.max_rect();
        let pill = egui::Rect::from_min_size(
            egui::pos2(area.center().x - size.x / 2.0, area.bottom() - size.y),
            size,
        );
        let link = ui.interact(pill, ui.id().with("github-updates"), egui::Sense::click());
        let stroke = if link.hovered() || link.has_focus() {
            3.0_f32
        } else {
            2.0_f32
        };
        let painter = ui.painter();
        painter.rect(
            pill,
            size.y / 2.0,
            Color32::WHITE,
            egui::Stroke::new(stroke, TEAL_INK),
            egui::StrokeKind::Inside,
        );
        painter.image(
            self.art.github_mark().id(),
            egui::Rect::from_center_size(
                egui::pos2(pill.left() + 24.0, pill.center().y),
                Vec2::splat(20.0),
            ),
            theme::unit_uv(),
            INK,
        );
        painter.galley(
            egui::pos2(pill.left() + 42.0, pill.center().y - label.size().y / 2.0),
            label,
            INK,
        );
        if link.clicked() {
            ui.ctx().open_url(egui::OpenUrl::new_tab(
                "https://github.com/FadedSociety/free-realms-launcher",
            ));
        }
        link.on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text("Take me to the GitHub page");
    }

    /// Join a friend's realm: paste the host's join details (or an address), check the fingerprint,
    /// pick a name. The same key-pair join as Settings, in one window.
    fn join_window(&mut self, ctx: &egui::Context) {
        if !self.join_open {
            return;
        }
        let screen = ctx.content_rect();
        let mut close = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        egui::Area::new(egui::Id::new("rd-join"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                let scrim = ui.allocate_rect(screen, egui::Sense::click());
                ui.painter()
                    .rect_filled(screen, 0.0, Color32::from_black_alpha(170));
                close |= scrim.clicked();
                let panel = egui::Rect::from_center_size(
                    screen.center(),
                    Vec2::new(
                        (screen.width() - 80.0).min(640.0),
                        (screen.height() - 60.0).min(430.0),
                    ),
                );
                ui.allocate_rect(panel, egui::Sense::click());
                ui.painter().add(theme::panel_shape(&self.art, panel));
                ui.scope_builder(
                    egui::UiBuilder::new().max_rect(panel.shrink(PANEL_PAD)),
                    |ui| {
                        panel_title(ui, "Join a friend's realm");
                        ui.label(
                            RichText::new(
                                "Paste the join details your host copied, then pick your name there.",
                            )
                            .color(Color32::WHITE),
                        );
                        ui.add_space(8.0);
                        let width = ui.available_width();
                        let address = theme::text_field(
                            ui,
                            &self.art,
                            &mut self.join_address,
                            "Join details or realm address, e.g. 203.0.113.7:20042",
                            Vec2::new(width, 42.0),
                            261,
                        );
                        self.record_focus(FocusTarget::AddAddress, &address);
                        if address.changed() {
                            split_join_details(&mut self.join_address, &mut self.join_identity);
                            if !self.join_broker_edited {
                                self.join_broker = default_broker(&self.join_address);
                            }
                            self.join_public_key = None;
                        }
                        ui.add_space(6.0);
                        theme::text_field(
                            ui,
                            &self.art,
                            &mut self.join_identity,
                            "Server identity (from the join details, or Join asks the realm)",
                            Vec2::new(width, 42.0),
                            64,
                        );
                        let note = match parse_server_identity(&self.join_identity) {
                            Some(identity) => format!(
                                "Fingerprint {}: check it matches the one your host sees.",
                                short_fingerprint(&identity)
                            ),
                            None => "No identity yet: Join asks the realm for it.".into(),
                        };
                        ui.label(RichText::new(note).color(Color32::from_rgb(222, 222, 250)));
                        ui.add_space(6.0);
                        theme::text_field(
                            ui,
                            &self.art,
                            &mut self.join_handle,
                            "Your name on that realm (3-32 letters, digits, _ or -)",
                            Vec2::new(width, 42.0),
                            32,
                        );
                        ui.add_space(12.0);
                        ui.horizontal(|ui| {
                            ui.add_space(((ui.available_width() - 160.0 - 12.0 - 130.0) / 2.0).max(0.0));
                            let ready = !self.join_address.trim().is_empty()
                                && self.join_handle.trim().len() >= 3
                                && self.screenshot.is_none();
                            if theme::button(
                                ui,
                                &self.art,
                                "Join",
                                Vec2::new(160.0, 50.0),
                                ready,
                                theme::ButtonSkin::Standard,
                            )
                            .clicked()
                                && self.join_realm()
                            {
                                close = true;
                            }
                            close |= theme::button(
                                ui,
                                &self.art,
                                "Cancel",
                                Vec2::new(130.0, 50.0),
                                true,
                                theme::ButtonSkin::Standard,
                            )
                            .clicked();
                        });
                    },
                );
            });
        if close {
            self.join_open = false;
        }
    }

    /// Ask the realm for its identity when none was pasted, then make this PC's key for it. A bare
    /// address entry for the same realm (from older launchers) is replaced by the joined one.
    fn join_realm(&mut self) -> bool {
        if parse_server_identity(&self.join_identity).is_none() {
            // ponytail: one bounded exchange (2 s connect, 3 s reply) on the UI thread, as in Settings.
            match auth::fetch_identity(&self.join_broker) {
                Ok(identity) => {
                    self.join_identity = identity.iter().map(|b| format!("{b:02x}")).collect();
                }
                Err(error) => {
                    self.message = auth_error_message(error);
                    return false;
                }
            }
        }
        let address = self.join_address.trim().to_owned();
        let identity = parse_server_identity(&self.join_identity);
        let known = self.servers.iter().position(|s| {
            !s.profile_id.is_empty()
                && s.address == address
                && auth::load_profile(&s.profile_id)
                    .is_ok_and(|p| Some(p.server_identity) == identity)
        });
        if let Some(index) = known {
            self.selected = index;
        } else {
            self.servers
                .retain(|s| !(s.profile_id.is_empty() && s.address == address));
            self.selected = self.selected.min(self.servers.len().saturating_sub(1));
            self.join_public_key = None;
            self.create_join_profile();
            if self.join_public_key.is_none() {
                return false;
            }
        }
        // v0.3.4: Join goes straight in; the first login makes the account there.
        self.play();
        true
    }

    fn directory(&self) -> String {
        match self.directory_url.trim() {
            "" => osfr_launcher::directory::DIRECTORY_URL.to_owned(),
            url => url.to_owned(),
        }
    }

    /// Announce the hosted realm every minute while it runs (Internet hosting with List my realm on);
    /// the directory checks it through the realm's broker. Stopping takes it off the list.
    pub(super) fn start_announcer(&mut self) {
        use std::sync::atomic::{AtomicBool, Ordering};
        self.stop_announcer();
        let Some(realm) = self
            .local
            .as_ref()
            .filter(|_| self.list_realm && self.host_internet)
        else {
            return;
        };
        let Ok(public) = self.host_public.trim().parse::<std::net::IpAddr>() else {
            return;
        };
        let Some(seed) =
            listing_key_path().and_then(|path| osfr_launcher::local::server_identity(&path).ok())
        else {
            return;
        };
        let key = ed25519_dalek::SigningKey::from_bytes(&seed);
        let identity: String = realm.identity.iter().map(|b| format!("{b:02x}")).collect();
        let name = match self.realm_name.trim() {
            n if n.len() >= 3 => n.to_owned(),
            _ => "Free Realms realm".to_owned(),
        };
        let description = self.realm_description.trim().to_owned();
        let address =
            std::net::SocketAddr::new(public, osfr_launcher::local::LOGIN_PORT).to_string();
        let broker =
            std::net::SocketAddr::new(public, osfr_launcher::local::BROKER_PORT).to_string();
        let base = self.directory();
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let listing = osfr_launcher::directory::Announce {
                name: &name,
                description: &description,
                address: &address,
                broker: &broker,
                identity: &identity,
            };
            let mut last = None;
            while !stopped.load(Ordering::SeqCst) {
                let now = osfr_launcher::directory::announce(&base, &key, &listing).err();
                if now != last {
                    let _ = tx.send(match &now {
                        None => "Your realm is listed in the Servers tab.".to_owned(),
                        Some(error) => format!("Server list: {error}"),
                    });
                    last = now;
                }
                for _ in 0..60 {
                    if stopped.load(Ordering::SeqCst) {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_secs(1));
                }
            }
            let _ = osfr_launcher::directory::delist(&base, &key, &identity);
        });
        self.announcer = Some(stop);
        self.announce_rx = Some(rx);
    }

    pub(super) fn stop_announcer(&mut self) {
        if let Some(stop) = self.announcer.take() {
            stop.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }

    fn refresh_servers(&mut self, ctx: &egui::Context) {
        if self.browser_rx.is_some() || self.screenshot.is_some() {
            return;
        }
        let (base, ctx) = (self.directory(), ctx.clone());
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(osfr_launcher::directory::servers(&base));
            ctx.request_repaint();
        });
        self.browser_rx = Some(rx);
    }

    /// The Servers tab: listed realms, favourites first, search, Join through the Join window.
    fn servers_screen(&mut self, ui: &mut egui::Ui) {
        if let Some(Ok(result)) = self.browser_rx.as_ref().map(|rx| rx.try_recv()) {
            self.browser_rx = None;
            match result {
                Ok(list) => self.browser = list,
                Err(error) => self.message = format!("Server list: {error}"),
            }
        }
        let rect = ui.available_rect_before_wrap();
        ui.allocate_rect(rect, egui::Sense::hover());
        ui.painter().add(theme::panel_shape(&self.art, rect));
        ui.scope_builder(egui::UiBuilder::new().max_rect(rect.shrink(PANEL_PAD)), |ui| {
            panel_title(ui, "Servers");
            ui.horizontal(|ui| {
                let width = (ui.available_width() - 140.0).max(160.0);
                theme::text_field(ui, &self.art, &mut self.browser_search, "Search realms", Vec2::new(width, 40.0), 40);
                let refresh = theme::button(
                    ui,
                    &self.art,
                    "Refresh",
                    Vec2::new(124.0, 40.0),
                    self.browser_rx.is_none(),
                    theme::ButtonSkin::Standard,
                );
                if refresh.clicked() {
                    self.refresh_servers(ui.ctx());
                }
            });
            ui.add_space(8.0);
            let needle = self.browser_search.trim().to_lowercase();
            let mut rows: Vec<_> = self
                .browser
                .iter()
                .filter(|l| {
                    needle.is_empty()
                        || l.name.to_lowercase().contains(&needle)
                        || l.description.to_lowercase().contains(&needle)
                })
                .cloned()
                .collect();
            rows.sort_by_key(|l| (!self.favorites.contains(&l.identity), l.name.to_lowercase()));
            if rows.is_empty() {
                let note = if self.browser_rx.is_some() {
                    "Loading realms..."
                } else if self.browser.is_empty() {
                    "No realms are listed right now. Host one on the Host tab and tick List my realm."
                } else {
                    "No realms match your search."
                };
                ui.label(RichText::new(note).color(Color32::WHITE));
            }
            ui.scope(|ui| {
                themed_scroll(ui);
                egui::ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| {
                    for listing in rows {
                        let (row, _) =
                            ui.allocate_exact_size(Vec2::new(ui.available_width(), 64.0), egui::Sense::hover());
                        self.art.piece(ui.painter(), Piece::Card, row);
                        let favorite = self.favorites.contains(&listing.identity);
                        let dot = egui::Rect::from_center_size(
                            egui::pos2(row.left() + 32.0, row.center().y),
                            Vec2::splat(32.0),
                        );
                        let star = ui.interact(dot, ui.id().with(("favorite", &listing.identity)), egui::Sense::click());
                        let gold = Color32::from_rgb(196, 132, 0);
                        ui.painter().circle(
                            dot.center(),
                            9.0,
                            if favorite { gold } else { Color32::TRANSPARENT },
                            egui::Stroke::new(2.5_f32, gold),
                        );
                        if star.clicked() {
                            if favorite {
                                self.favorites.retain(|id| id != &listing.identity);
                            } else {
                                self.favorites.push(listing.identity.clone());
                            }
                        }
                        star.on_hover_text(if favorite { "Remove from favorites" } else { "Add to favorites" });
                        let clip = ui.painter().with_clip_rect(row.shrink(10.0));
                        clip.text(
                            egui::pos2(row.left() + 58.0, row.top() + 12.0),
                            egui::Align2::LEFT_TOP,
                            &listing.name,
                            theme::display(17.0),
                            INK,
                        );
                        let detail = if listing.description.is_empty() { &listing.address } else { &listing.description };
                        clip.text(
                            egui::pos2(row.left() + 58.0, row.top() + 37.0),
                            egui::Align2::LEFT_TOP,
                            detail,
                            egui::FontId::proportional(13.0),
                            TEAL_INK,
                        );
                        let join_rect = egui::Rect::from_min_size(
                            egui::pos2(row.right() - 134.0, row.center().y - 21.0),
                            Vec2::new(116.0, 42.0),
                        );
                        let join = ui.put(join_rect, |ui: &mut egui::Ui| {
                            theme::button(ui, &self.art, "Join", Vec2::new(116.0, 42.0), true, theme::ButtonSkin::Standard)
                        });
                        if join.clicked() {
                            self.join_address = listing.address.clone();
                            self.join_identity = listing.identity.clone();
                            self.join_broker = listing.broker.clone();
                            self.join_broker_edited = true;
                            self.join_open = true;
                        }
                        ui.add_space(6.0);
                    }
                });
            });
        });
    }

    /// A news card's full release notes over the launcher. Esc, Close or a click outside closes it.
    fn notes_window(&mut self, ctx: &egui::Context) {
        let Some(item) = self.news_open.clone() else {
            return;
        };
        let screen = ctx.content_rect();
        let mut close = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        egui::Area::new(egui::Id::new("rd-notes"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                let scrim = ui.allocate_rect(screen, egui::Sense::click());
                ui.painter()
                    .rect_filled(screen, 0.0, Color32::from_black_alpha(170));
                close |= scrim.clicked();
                let panel = egui::Rect::from_center_size(
                    screen.center(),
                    Vec2::new(
                        (screen.width() - 80.0).min(760.0),
                        (screen.height() - 80.0).min(640.0),
                    ),
                );
                // The panel takes its own clicks so only the scrim closes the window.
                ui.allocate_rect(panel, egui::Sense::click());
                ui.painter().add(theme::panel_shape(&self.art, panel));
                ui.scope_builder(
                    egui::UiBuilder::new().max_rect(panel.shrink(PANEL_PAD)),
                    |ui| {
                        panel_title(ui, &short_title(&item.title));
                        if !item.date.is_empty() {
                            ui.vertical_centered(|ui| {
                                ui.label(
                                    RichText::new(&item.date)
                                        .color(Color32::from_rgb(222, 222, 250)),
                                );
                            });
                        }
                        ui.add_space(8.0);
                        let body_h = ui.available_height() - 52.0 - 12.0;
                        ui.scope(|ui| {
                            themed_scroll(ui);
                            egui::ScrollArea::vertical()
                                .max_height(body_h)
                                .auto_shrink([false, false])
                                .show(ui, |ui| notes_text(ui, &item.body));
                        });
                        ui.add_space(12.0);
                        ui.horizontal(|ui| {
                            let github = !item.url.is_empty();
                            let width = if github { 190.0 + 12.0 + 130.0 } else { 130.0 };
                            ui.add_space(((ui.available_width() - width) / 2.0).max(0.0));
                            if github
                                && theme::button(
                                    ui,
                                    &self.art,
                                    "Open on GitHub",
                                    Vec2::new(190.0, 50.0),
                                    true,
                                    theme::ButtonSkin::Standard,
                                )
                                .clicked()
                            {
                                ui.ctx().open_url(egui::OpenUrl::new_tab(&item.url));
                            }
                            close |= theme::button(
                                ui,
                                &self.art,
                                "Close",
                                Vec2::new(130.0, 50.0),
                                true,
                                theme::ButtonSkin::Standard,
                            )
                            .clicked();
                        });
                    },
                );
            });
        if close {
            self.news_open = None;
        }
    }

    fn hero_window(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        // The art is cropped to the middle of the frame's vine bands, so no edge shows outside them
        // (bands measured on hero_frame.png, drawn at the 0.5 art scale).
        let scene = egui::Rect::from_min_max(
            rect.min + Vec2::new(46.0, 54.0),
            rect.max - Vec2::new(46.0, 40.0),
        );
        self.art
            .draw_backdrop(&ui.painter().with_clip_rect(scene), scene);
        let [tw, th] = self.art.hero().size();
        let height = scene.height() * 0.86;
        let size = Vec2::new(height * tw as f32 / th as f32, height);
        let cast = egui::Rect::from_min_size(
            egui::pos2(scene.center().x - size.x / 2.0, scene.bottom() - height),
            size,
        );
        ui.painter().with_clip_rect(scene).image(
            self.art.hero().id(),
            cast,
            theme::unit_uv(),
            Color32::WHITE,
        );
        // Scrim: a soft dark gradient under the dock so it sits on calm ground, not the busy art.
        let scrim = egui::Rect::from_min_max(
            egui::pos2(scene.left(), scene.bottom() - DOCK_H - 80.0),
            scene.max,
        );
        let mut mesh = egui::Mesh::default();
        for (pos, alpha) in [
            (scrim.left_top(), 0),
            (scrim.right_top(), 0),
            (scrim.right_bottom(), 150),
            (scrim.left_bottom(), 150),
        ] {
            mesh.colored_vertex(pos, Color32::from_black_alpha(alpha));
        }
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(0, 2, 3);
        ui.painter().with_clip_rect(scene).add(mesh);
        self.art.piece(ui.painter(), Piece::HeroFrame, rect);
        // The frame's bottom vine ends 54 points above `rect`; keep 14 points clear.
        let dock_w = (scene.width() * 0.62).clamp(260.0, 440.0);
        let bottom = rect.bottom() - 68.0;
        let dock = egui::Rect::from_min_max(
            egui::pos2(scene.center().x - dock_w / 2.0, bottom - DOCK_H),
            egui::pos2(scene.center().x + dock_w / 2.0, bottom),
        );
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(dock)
                .layout(egui::Layout::top_down(egui::Align::Center)),
            |ui| self.play_dock_v2(ui, dock_w),
        );
        if self.realm_popup {
            self.realm_chooser(ui, scene);
        }
    }

    fn play_dock_v2(&mut self, ui: &mut egui::Ui, width: f32) {
        let name = self
            .servers
            .get(self.selected)
            .map(|s| s.name.clone())
            .unwrap_or_default();
        // Realm chip: a white pill sized to its text, 14 points of side padding (contrast.py).
        let mut job = egui::text::LayoutJob::simple_singleline(name, theme::display(16.0), INK);
        job.wrap = egui::text::TextWrapping::truncate_at_width(width - 130.0);
        let name = ui.painter().layout_job(job);
        let change = ui
            .painter()
            .layout_no_wrap("Change".into(), theme::display(16.0), TEAL_INK);
        let chip_w = 14.0 + 16.0 + 8.0 + name.size().x + 24.0 + change.size().x + 16.0;
        let (chip, _) = ui.allocate_exact_size(Vec2::new(chip_w, CHIP_H), egui::Sense::hover());
        let painter = ui.painter().clone();
        painter.rect(
            chip,
            CHIP_H / 2.0,
            Color32::WHITE,
            egui::Stroke::new(2.0_f32, TEAL_INK),
            egui::StrokeKind::Inside,
        );
        self.art.pip(
            &painter,
            0,
            egui::Rect::from_center_size(
                egui::pos2(chip.left() + 22.0, chip.center().y),
                Vec2::splat(16.0),
            ),
        );
        painter.galley(
            egui::pos2(chip.left() + 38.0, chip.center().y - name.size().y / 2.0),
            name,
            INK,
        );
        let link = egui::Rect::from_min_max(
            egui::pos2(chip.right() - change.size().x - 28.0, chip.top()),
            chip.right_bottom(),
        );
        let response = ui.interact(link, ui.id().with("realm-change"), egui::Sense::click());
        let at = egui::pos2(link.left() + 12.0, chip.center().y - change.size().y / 2.0);
        if response.hovered() || response.has_focus() {
            let y = at.y + change.size().y;
            painter.line_segment(
                [egui::pos2(at.x, y), egui::pos2(at.x + change.size().x, y)],
                egui::Stroke::new(2.0_f32, TEAL_INK),
            );
        }
        painter.galley(at, change, TEAL_INK);
        if response.clicked() {
            self.realm_popup = !self.realm_popup;
        }
        ui.add_space(GAP);
        let label = if self.child.is_some() {
            "GAME RUNNING"
        } else if self.pending.is_some() || self.pending_local.is_some() {
            "PREPARING..."
        } else if self.patch_job.is_some() || self.patch_preview {
            "PATCHING..."
        } else {
            "PLAY NOW!"
        };
        let ready = self.pending.is_none()
            && self.pending_local.is_none()
            && self.child.is_none()
            && self.server_job.is_none()
            && self.patch_allows_play()
            && (self.screenshot.is_none() || self.focus_play_on_start);
        let play = theme::button(
            ui,
            &self.art,
            label,
            Vec2::new(width, PLAY_H),
            ready,
            theme::ButtonSkin::Play,
        );
        self.record_focus(FocusTarget::Play, &play);
        if self.focus_play_on_start && !self.focus_play_requested {
            ui.ctx().memory_mut(|memory| memory.request_focus(play.id));
            self.focus_play_requested = true;
        }
        if play.clicked() {
            // The local realm (this PC) runs the bundled server; any other realm launches the client at it.
            let local = self
                .servers
                .get(self.selected)
                .is_some_and(|s| s.address.starts_with("127.0.0.1"));
            if local && self.launcher_dir.is_some() {
                self.play_locally();
            } else {
                self.play();
            }
        }
        ui.add_space(GAP);
        let news = self
            .news
            .first()
            .map(|n| {
                if n.headline.is_empty() {
                    n.title.clone()
                } else {
                    n.headline.clone()
                }
            })
            .unwrap_or_else(|| "Welcome to Free Realms Expanded".into());
        let mut job = egui::text::LayoutJob::default();
        job.append(
            "Latest news:  ",
            0.0,
            egui::TextFormat::simple(theme::display(15.0), Color32::from_rgb(188, 221, 62)),
        );
        job.append(
            &news,
            0.0,
            egui::TextFormat::simple(egui::FontId::proportional(15.0), Color32::WHITE),
        );
        job.wrap = egui::text::TextWrapping::truncate_at_width(width - 32.0);
        let galley = ui.painter().layout_job(job);
        let (pill, _) = ui.allocate_exact_size(
            Vec2::new(galley.size().x + 32.0, NEWS_H),
            egui::Sense::hover(),
        );
        ui.painter().rect(
            pill,
            NEWS_H / 2.0,
            Color32::from_rgb(13, 34, 56),
            egui::Stroke::new(2.0_f32, Color32::from_rgb(58, 107, 152)),
            egui::StrokeKind::Inside,
        );
        ui.painter()
            .galley(pill.center() - galley.size() / 2.0, galley, Color32::WHITE);
    }

    fn realm_chooser(&mut self, ui: &mut egui::Ui, scene: egui::Rect) {
        let rect = egui::Rect::from_min_size(
            scene.min + Vec2::new(16.0, 16.0),
            Vec2::new(scene.width().min(420.0) - 32.0, 300.0),
        );
        egui::Area::new(egui::Id::new("rd-realms"))
            .fixed_pos(rect.min)
            .show(ui.ctx(), |ui| {
                ui.painter().add(theme::panel_shape(&self.art, rect));
                ui.scope_builder(
                    egui::UiBuilder::new().max_rect(rect.shrink(PANEL_PAD)),
                    |ui| {
                        panel_title(ui, "Choose a realm");
                        for index in 0..self.servers.len() {
                            let text = format!(
                                "{}  ·  {}",
                                self.servers[index].name, self.servers[index].address
                            );
                            let row = ui.selectable_label(
                                index == self.selected,
                                RichText::new(text).color(Color32::WHITE),
                            );
                            self.record_focus(FocusTarget::RealmRow(index), &row);
                            if row.clicked() {
                                self.selected = index;
                            }
                        }
                        let join = theme::button(
                            ui,
                            &self.art,
                            "Join a friend's realm",
                            Vec2::new(ui.available_width().min(280.0), 44.0),
                            self.servers.len() < 16,
                            theme::ButtonSkin::Standard,
                        );
                        self.record_focus(FocusTarget::AddRealm, &join);
                        if join.clicked() {
                            self.join_open = true;
                            self.realm_popup = false;
                        }
                        if let Some(server) = self.servers.get_mut(self.selected) {
                            let label =
                                profile_label(&server.profile_id, &self.profiles).to_owned();
                            let profile = egui::ComboBox::from_id_salt("login-profile")
                                .selected_text(label)
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(
                                        &mut server.profile_id,
                                        String::new(),
                                        "Local login",
                                    );
                                    for (value, name) in &self.profiles {
                                        ui.selectable_value(
                                            &mut server.profile_id,
                                            value.clone(),
                                            name,
                                        );
                                    }
                                });
                            self.record_focus(FocusTarget::LoginProfile, &profile.response);
                        }
                        if ui.button("Done").clicked() {
                            self.realm_popup = false;
                        }
                    },
                );
            });
    }

    fn host_screen(&mut self, ui: &mut egui::Ui) {
        let full = ui.available_rect_before_wrap();
        ui.allocate_rect(full, egui::Sense::hover());
        let wide = full.width() >= 760.0;
        let (left, right) = if wide {
            let half = (full.width() - 16.0) / 2.0;
            (
                egui::Rect::from_min_size(full.min, Vec2::new(half, full.height())),
                egui::Rect::from_min_max(
                    egui::pos2(full.min.x + half + 16.0, full.min.y),
                    full.max,
                ),
            )
        } else {
            let top = full.height() * 0.55;
            (
                egui::Rect::from_min_size(full.min, Vec2::new(full.width(), top)),
                egui::Rect::from_min_max(egui::pos2(full.min.x, full.min.y + top + 12.0), full.max),
            )
        };
        ui.painter().add(theme::panel_shape(&self.art, left));
        ui.painter().add(theme::panel_shape(&self.art, right));
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(left.shrink(PANEL_PAD)),
            |ui| self.host_setup(ui),
        );
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(right.shrink(PANEL_PAD)),
            |ui| self.host_check(ui),
        );
    }

    fn host_setup(&mut self, ui: &mut egui::Ui) {
        panel_title(ui, "Host a realm");
        ui.add_space(8.0);
        let mode = if !self.host {
            0
        } else if self.host_internet {
            2
        } else {
            1
        };
        for (index, title, detail) in [
            (0, "Just me", "Play on this PC."),
            (1, "Friends nearby", "Same Wi-Fi or Tailscale."),
            (2, "Internet", "Port forwarding or a server host."),
        ] {
            let (card, response) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), 64.0), egui::Sense::click());
            let on = mode == index;
            self.art.piece(
                ui.painter(),
                if on {
                    Piece::ModeCardOn
                } else {
                    Piece::ModeCard
                },
                card,
            );
            let color = if on {
                Color32::from_rgb(0, 99, 46)
            } else {
                INK
            };
            let text = ui.painter().with_clip_rect(card.shrink(8.0));
            text.text(
                card.left_top() + Vec2::new(20.0, 11.0),
                egui::Align2::LEFT_TOP,
                title,
                theme::display(18.0),
                color,
            );
            text.text(
                card.left_top() + Vec2::new(20.0, 36.0),
                egui::Align2::LEFT_TOP,
                detail,
                egui::FontId::proportional(14.0),
                color,
            );
            if response.clicked() {
                self.host = index > 0;
                self.host_internet = index == 2;
                if index == 2 && self.host_public.trim().is_empty() {
                    self.lookup_public_ip(ui.ctx());
                }
                if index == 1 && self.host_choices.is_empty() {
                    self.host_choices = osfr_launcher::local::host_addresses();
                    self.host_address = self.host_choices.first().copied();
                }
            }
            ui.add_space(6.0);
        }
        ui.add_space(10.0);
        if self.host_internet {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Public IP").strong().color(Color32::WHITE));
                let (art, value) = (&self.art, &mut self.host_public);
                ui.scope_builder(egui::UiBuilder::new().id("host-public-ip"), |ui| {
                    theme::text_field(
                        ui,
                        art,
                        value,
                        "e.g. 203.0.113.7",
                        Vec2::new(200.0, 42.0),
                        45,
                    )
                });
                if theme::button(
                    ui,
                    &self.art,
                    "Find it",
                    Vec2::new(96.0, 40.0),
                    true,
                    theme::ButtonSkin::Standard,
                )
                .clicked()
                {
                    self.lookup_public_ip(ui.ctx());
                }
            });
            let ports = format!(
                "UDP {g}, {l} · TCP {g}, {b}",
                g = osfr_launcher::local::GATEWAY_PORT,
                l = osfr_launcher::local::LOGIN_PORT,
                b = osfr_launcher::local::BROKER_PORT
            );
            ui.horizontal(|ui| {
                ui.label(RichText::new("Forward").strong().color(Color32::WHITE));
                let mut shown = ports.clone();
                theme::read_only_text_field(ui, &self.art, &shown, "", Vec2::new(240.0, 42.0));
                if theme::button(
                    ui,
                    &self.art,
                    "Copy",
                    Vec2::new(86.0, 40.0),
                    true,
                    theme::ButtonSkin::Standard,
                )
                .clicked()
                {
                    ui.ctx().copy_text(std::mem::take(&mut shown));
                }
            });
            ui.label(RichText::new("Your router sends these ports to this PC. Not sure how? Search your router's name + \"port forwarding\".").color(Color32::WHITE));
            ui.add_space(8.0);
            ui.checkbox(
                &mut self.list_realm,
                RichText::new("List my realm in the Servers tab").color(Color32::WHITE),
            );
            if self.list_realm {
                let width = ui.available_width().min(440.0);
                theme::text_field(
                    ui,
                    &self.art,
                    &mut self.realm_name,
                    "Realm name, e.g. Housing Party",
                    Vec2::new(width, 40.0),
                    32,
                );
                theme::text_field(
                    ui,
                    &self.art,
                    &mut self.realm_description,
                    "Short description (optional)",
                    Vec2::new(width, 40.0),
                    100,
                );
            }
        } else if self.host {
            match self.host_address {
                Some(ip) => ui.label(
                    RichText::new(format!(
                        "Friends join at {ip}:{}.",
                        osfr_launcher::local::LOGIN_PORT
                    ))
                    .color(Color32::WHITE),
                ),
                None => ui.label(
                    RichText::new(
                        "No LAN or VPN address found. Join a network or start Tailscale.",
                    )
                    .color(Color32::WHITE),
                ),
            };
        }
    }

    fn host_check(&mut self, ui: &mut egui::Ui) {
        panel_title(ui, "Setup check");
        ui.add_space(8.0);
        let server_ok = self
            .launcher_dir
            .as_ref()
            .is_some_and(|root| LocalSettings::packaged(root).missing().is_empty());
        let ip = self.host_public.trim().to_owned();
        let ip_ok = ip.parse::<std::net::IpAddr>().is_ok();
        let client_ok = client_folder_ok(&self.client);
        // Each row: passed, what it checks, and what it found (or what to do).
        let rows = [
            (
                server_ok,
                "Server installed",
                if server_ok {
                    "ready"
                } else {
                    "first play downloads it"
                }
                .to_owned(),
            ),
            (
                client_ok,
                "Client folder chosen",
                if client_ok {
                    "ready"
                } else {
                    "choose it in Settings"
                }
                .to_owned(),
            ),
            (
                self.host,
                "Hosting chosen",
                match (self.host, self.host_internet) {
                    (false, _) => "pick Friends nearby or Internet",
                    (true, true) => "Internet",
                    (true, false) => "Friends nearby",
                }
                .to_owned(),
            ),
            (
                !self.host_internet || ip_ok,
                "Public IP entered",
                match (self.host_internet, ip_ok) {
                    (false, _) => "not needed".to_owned(),
                    (true, true) => ip,
                    (true, false) => "needed".to_owned(),
                },
            ),
        ];
        for (ok, text, note) in rows {
            let (row, _) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), 44.0), egui::Sense::hover());
            self.art.piece(ui.painter(), Piece::Card, row);
            let dot = egui::pos2(row.left() + 24.0, row.center().y);
            self.art.pip(
                ui.painter(),
                if ok { 0 } else { 1 },
                egui::Rect::from_center_size(dot, Vec2::splat(18.0)),
            );
            if ok {
                // A tick in the green ring: passed checks read at a glance.
                ui.painter().line(
                    vec![
                        dot + Vec2::new(-4.5, 0.0),
                        dot + Vec2::new(-1.0, 3.5),
                        dot + Vec2::new(5.0, -4.0),
                    ],
                    egui::Stroke::new(2.5_f32, Color32::from_rgb(0, 120, 60)),
                );
            }
            ui.painter().text(
                egui::pos2(row.left() + 42.0, row.center().y),
                egui::Align2::LEFT_CENTER,
                text,
                egui::FontId::proportional(14.0),
                INK,
            );
            ui.painter().text(
                egui::pos2(row.right() - 16.0, row.center().y),
                egui::Align2::RIGHT_CENTER,
                note,
                egui::FontId::proportional(12.0),
                Color32::from_rgb(74, 90, 112),
            );
            ui.add_space(6.0);
        }
        ui.add_space(6.0);
        let idle = self.pending.is_none()
            && self.pending_local.is_none()
            && self.child.is_none()
            && self.server_job.is_none();
        ui.horizontal(|ui| {
            let go = theme::button(
                ui,
                &self.art,
                "HOST & PLAY",
                Vec2::new(250.0, 70.0),
                idle && self.host,
                theme::ButtonSkin::Play,
            );
            if go.clicked() {
                self.play_locally();
            }
            let details = self.local.as_ref().map(|realm| {
                let address = match self.host_public.trim().parse::<std::net::IpAddr>() {
                    Ok(ip) if self.host_internet => {
                        std::net::SocketAddr::new(ip, osfr_launcher::local::LOGIN_PORT)
                    }
                    _ => realm.login,
                };
                let identity: String = realm.identity.iter().map(|b| format!("{b:02x}")).collect();
                format!("Realm address: {address}\nServer identity: {identity}")
            });
            let (rect, response) =
                ui.allocate_exact_size(Vec2::new(170.0, 44.0), egui::Sense::click());
            self.art.piece(ui.painter(), Piece::Teal, rect);
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Copy join details",
                theme::display(15.0),
                Color32::from_rgb(4, 30, 42),
            );
            if response.clicked() {
                match details {
                    Some(text) => {
                        ui.ctx().copy_text(text);
                        self.message = "Join details copied. Send them to your players.".into();
                    }
                    None => {
                        self.message =
                            "Start hosting first (HOST & PLAY), then copy the join details.".into()
                    }
                }
            }
        });
    }
}

/// A purple panel's title, centred below its corner ornaments.
fn panel_title(ui: &mut egui::Ui, text: &str) {
    ui.vertical_centered(|ui| {
        ui.label(
            RichText::new(text)
                .font(theme::display(21.0))
                .color(Color32::WHITE),
        );
    });
}

/// A release name for cards and the notes title: "Server v0.1.4", no "(early preview)" or subtitle.
fn short_title(title: &str) -> String {
    title
        .split(':')
        .next()
        .unwrap_or_default()
        .replace(" (early preview)", "")
}

/// The purple panels' scroll bar: always-solid, a dark purple track and a lavender handle.
fn themed_scroll(ui: &mut egui::Ui) {
    let style = ui.style_mut();
    style.spacing.scroll = egui::style::ScrollStyle::solid();
    style.spacing.scroll.bar_width = 10.0;
    style.spacing.scroll.dormant_background_opacity = 1.0;
    style.spacing.scroll.active_background_opacity = 1.0;
    style.spacing.scroll.interact_background_opacity = 1.0;
    style.spacing.scroll.dormant_handle_opacity = 1.0;
    style.spacing.scroll.active_handle_opacity = 1.0;
    style.visuals.extreme_bg_color = Color32::from_rgb(52, 44, 92);
    style.visuals.widgets.inactive.bg_fill = Color32::from_rgb(186, 180, 240);
    style.visuals.widgets.hovered.bg_fill = Color32::from_rgb(214, 210, 250);
    style.visuals.widgets.active.bg_fill = Color32::WHITE;
}

/// Release notes as the launcher shows them: `#` headings, `-` bullets and paragraphs, in white on the
/// purple panel (7:1), with 6 points between lines.
fn notes_text(ui: &mut egui::Ui, body: &str) {
    ui.spacing_mut().item_spacing.y = 6.0;
    for line in body.lines() {
        let line = plain(line.trim());
        let heading = ["### ", "## ", "# "]
            .iter()
            .find_map(|h| line.strip_prefix(*h));
        let bullet = ["- ", "* "].iter().find_map(|b| line.strip_prefix(*b));
        if line.is_empty() {
            ui.add_space(4.0);
        } else if let Some(heading) = heading {
            ui.add_space(4.0);
            ui.label(
                RichText::new(heading)
                    .font(theme::display(18.0))
                    .color(Color32::WHITE),
            );
        } else if let Some(bullet) = bullet {
            ui.label(
                RichText::new(format!("\u{2022}  {bullet}"))
                    .size(15.0)
                    .color(Color32::WHITE),
            );
        } else {
            ui.label(RichText::new(line).size(15.0).color(Color32::WHITE));
        }
    }
}

/// Markdown marks dropped: `[text](url)` keeps its text, `**` and backticks go.
fn plain(line: &str) -> String {
    let mut out = String::new();
    let mut rest = line;
    while let Some(open) = rest.find('[') {
        let Some(mid) = rest[open..].find("](").map(|m| open + m) else {
            break;
        };
        let Some(close) = rest[mid..].find(')').map(|c| mid + c) else {
            break;
        };
        out.push_str(&rest[..open]);
        out.push_str(&rest[open + 1..mid]);
        rest = &rest[close + 1..];
    }
    out.push_str(rest);
    out.replace("**", "").replace('`', "")
}

#[cfg(test)]
mod notes_tests {
    use super::plain;

    #[test]
    fn plain_keeps_link_text_and_drops_markdown_marks() {
        assert_eq!(
            plain("See [v0.2.2](https://github.com/x) **now**, run `osfr-gateway`."),
            "See v0.2.2 now, run osfr-gateway."
        );
        assert_eq!(plain("a [b] c"), "a [b] c");
    }
}

/// The Get started guide, shown in the notes window from the button in the status bar.
pub(super) fn guide_item() -> osfr_launcher::release::NewsItem {
    osfr_launcher::release::NewsItem {
        title: "Getting started".into(),
        headline: String::new(),
        body: GUIDE.into(),
        date: String::new(),
        url: "https://github.com/FadedSociety/free-realms-launcher#readme".into(),
    }
}

const GUIDE: &str = "\
## 1. Your game client
Free Realms Expanded runs on your own copy of the original Free Realms client (version 1.910.1.530630). Open **Settings** and choose the folder with FreeRealms.exe, or press **Download the client** to fetch it from the community host into a client folder beside the launcher.

## 2. Play on this PC
Press **Change** above PLAY NOW!, pick **Local realm** and press **PLAY NOW!**. The first time, the launcher downloads the server and checks every file before it starts. Your first login makes your account.

## 3. Host a realm for friends
Open the **Host** tab and pick who can join:
- **Just me**: only this PC.
- **Friends nearby**: friends on the same Wi-Fi or on Tailscale join at the address the Host tab shows.
- **Internet**: forward UDP 20041 and 20042 and TCP 20041 and 20043 on your router to this PC, then enter your public IP (**Find it** looks it up). If your router's internet address differs from it or starts with 100.64 to 100.127, your provider shares one address between homes and forwarding will not work: use Tailscale instead.
Press **HOST & PLAY** and allow the Windows firewall prompt for private networks. Then press **Copy join details** and send the realm address and server identity to your players. The realm runs while your game is open. To choose who can join, list their handles in server/state/whitelist.txt.

## 4. Join a friend's realm
Press **Change** above PLAY NOW!, then **Join a friend's realm**. Paste the join details your host copied (realm address and server identity), check the fingerprint matches theirs, pick your name and press **Join**. Your key stays on this PC and no password is ever sent. Then press **PLAY NOW!**; your first login makes your account there.

## Controllers and handhelds
The launcher scales to any window. A controller moves between the tabs, PLAY NOW! and the Settings fields; the Host tab is touch or mouse for now.

## Need more help?
Click any news card for its full release notes, or open the launcher's GitHub page for guides and bug reports.
";

/// The host's Copy join details text ("Realm address: A" then "Server identity: H", on two lines or
/// run together when a single-line field drops the newline) pasted into the address field: split it.
fn split_join_details(address: &mut String, identity: &mut String) {
    let text = address.clone();
    let Some(at) = text.find("Server identity:") else {
        if let Some(rest) = text.trim().strip_prefix("Realm address:") {
            *address = rest.trim().to_owned();
        }
        return;
    };
    let (head, tail) = text.split_at(at);
    *identity = tail["Server identity:".len()..]
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .take(64)
        .collect();
    *address = head
        .trim()
        .trim_start_matches("Realm address:")
        .trim()
        .to_owned();
}

#[cfg(test)]
mod join_tests {
    use super::split_join_details;

    #[test]
    fn pasted_join_details_fill_address_and_identity() {
        let hex = "ab".repeat(32);
        for pasted in [
            format!("Realm address: 203.0.113.7:20042\nServer identity: {hex}"),
            format!("Realm address: 203.0.113.7:20042Server identity: {hex}"),
        ] {
            let (mut address, mut identity) = (pasted, String::new());
            split_join_details(&mut address, &mut identity);
            assert_eq!(address, "203.0.113.7:20042");
            assert_eq!(identity, hex);
        }
        let (mut address, mut identity) = ("198.51.100.2:20042".to_owned(), String::new());
        split_join_details(&mut address, &mut identity);
        assert_eq!(
            (address.as_str(), identity.as_str()),
            ("198.51.100.2:20042", "")
        );
    }
}
