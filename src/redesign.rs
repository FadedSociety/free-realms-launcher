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
                    ui.label(RichText::new(&self.message).color(Color32::WHITE));
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
                    ui.scope_builder(egui::UiBuilder::new().max_rect(rect.shrink(22.0)), |ui| {
                        egui::ScrollArea::vertical().show(ui, |ui| self.settings_ui(ui));
                    });
                } else if self.host_page {
                    self.host_screen(ui);
                } else {
                    self.play_screen(ui);
                }
            });
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let (rect, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 72.0), egui::Sense::hover());
        self.art.piece(ui.painter(), Piece::TopBar, rect);
        let inner = rect.shrink2(Vec2::new(26.0, 6.0));
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(inner)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
            |ui| {
                let [w, h] = self.banner.size();
                let (logo, _) = ui.allocate_exact_size(
                    Vec2::new(48.0 * w as f32 / h as f32, 48.0),
                    egui::Sense::hover(),
                );
                ui.painter()
                    .image(self.banner.id(), logo, theme::unit_uv(), Color32::WHITE);
                ui.add_space(14.0);
                let current = if self.settings_page {
                    2
                } else if self.host_page {
                    1
                } else {
                    0
                };
                for (page, label) in [(0, "Play"), (1, "Host"), (2, "Settings")] {
                    let enabled = page != 1 || self.launcher_dir.is_some();
                    let tab = theme::button(
                        ui,
                        &self.art,
                        label,
                        Vec2::new(if page == 2 { 150.0 } else { 124.0 }, 56.0),
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
        ui.scope_builder(egui::UiBuilder::new().max_rect(rect.shrink(20.0)), |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("News & updates")
                        .font(theme::display(21.0))
                        .color(Color32::WHITE),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new("GITHUB")
                            .size(11.0)
                            .color(Color32::from_rgb(222, 222, 250)),
                    );
                });
            });
            ui.add_space(6.0);
            if self.news.is_empty() {
                ui.label(
                    RichText::new("News shows here when you are online.").color(Color32::WHITE),
                );
            }
            for item in self.news.clone() {
                let (card, response) = ui.allocate_exact_size(
                    Vec2::new(ui.available_width(), 70.0),
                    egui::Sense::click(),
                );
                self.art.piece(ui.painter(), Piece::Card, card);
                let ring = egui::Rect::from_center_size(
                    egui::pos2(card.left() + 36.0, card.center().y),
                    Vec2::splat(46.0),
                );
                self.art.icon(ui.painter(), 2, ring.shrink(9.0));
                self.art.piece(ui.painter(), Piece::ThumbRing, ring);
                let text = egui::pos2(card.left() + 68.0, card.top() + 14.0);
                let clip = ui.painter().with_clip_rect(card.shrink(10.0));
                let title = item
                    .title
                    .split(':')
                    .next()
                    .unwrap_or_default()
                    .replace(" (early preview)", "");
                let tag = format!("{} \u{b7} {}", title.to_uppercase(), item.date);
                clip.text(
                    text,
                    egui::Align2::LEFT_TOP,
                    tag,
                    egui::FontId::proportional(11.5),
                    Color32::from_rgb(14, 95, 125),
                );
                let line = if item.headline.is_empty() {
                    &title
                } else {
                    &item.headline
                };
                let mut job = egui::text::LayoutJob::simple_singleline(
                    line.clone(),
                    egui::FontId::proportional(15.0),
                    INK,
                );
                job.wrap =
                    egui::text::TextWrapping::truncate_at_width(card.right() - text.x - 16.0);
                clip.galley(text + Vec2::new(0.0, 18.0), clip.layout_job(job), INK);
                if response.clicked() && !item.url.is_empty() {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(&item.url));
                }
                ui.add_space(6.0);
            }
            ui.hyperlink_to(
                RichText::new("All updates on GitHub").color(Color32::WHITE),
                "https://github.com/FadedSociety/free-realms-expanded/releases",
            );
            self.patch_progress_ui(ui);
        });
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
                ui.scope_builder(egui::UiBuilder::new().max_rect(rect.shrink(18.0)), |ui| {
                    ui.label(
                        RichText::new("Choose a realm")
                            .font(theme::display(19.0))
                            .color(Color32::WHITE),
                    );
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
                    ui.horizontal(|ui| {
                        let field = theme::text_field(
                            ui,
                            &self.art,
                            &mut self.new_address,
                            "Add host:port",
                            Vec2::new(220.0, 40.0),
                            261,
                        );
                        self.record_focus(FocusTarget::AddAddress, &field);
                        let add = theme::button(
                            ui,
                            &self.art,
                            "Add",
                            Vec2::new(90.0, 40.0),
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
                                    self.message =
                                        "Realm added. Select a saved profile or Local login."
                                            .into();
                                }
                                Err(error) => self.message = error,
                            }
                        }
                    });
                    if let Some(server) = self.servers.get_mut(self.selected) {
                        let label = profile_label(&server.profile_id, &self.profiles).to_owned();
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
                });
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
        ui.scope_builder(egui::UiBuilder::new().max_rect(left.shrink(20.0)), |ui| {
            self.host_setup(ui)
        });
        ui.scope_builder(egui::UiBuilder::new().max_rect(right.shrink(20.0)), |ui| {
            self.host_check(ui)
        });
    }

    fn host_setup(&mut self, ui: &mut egui::Ui) {
        ui.label(
            RichText::new("Host a realm")
                .font(theme::display(21.0))
                .color(Color32::WHITE),
        );
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
                    ui.ctx().open_url(egui::OpenUrl::new_tab(
                        "https://duckduckgo.com/?q=what+is+my+ip",
                    ));
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
        ui.label(
            RichText::new("Setup check")
                .font(theme::display(21.0))
                .color(Color32::WHITE),
        );
        ui.add_space(8.0);
        let server_ok = self
            .launcher_dir
            .as_ref()
            .is_some_and(|root| LocalSettings::packaged(root).missing().is_empty());
        let ip_ok = self.host_public.trim().parse::<std::net::IpAddr>().is_ok();
        let rows = [
            (
                server_ok,
                "Server installed",
                if server_ok {
                    "ready"
                } else {
                    "first play downloads it"
                },
            ),
            (
                !self.client.trim().is_empty(),
                "Client folder chosen",
                "Settings",
            ),
            (
                self.host,
                "Hosting chosen",
                if self.host_internet {
                    "Internet"
                } else {
                    "Friends nearby"
                },
            ),
            (
                !self.host_internet || ip_ok,
                "Public IP entered",
                if self.host_internet {
                    "needed"
                } else {
                    "not needed"
                },
            ),
        ];
        for (ok, text, note) in rows {
            let (row, _) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), 44.0), egui::Sense::hover());
            self.art.piece(ui.painter(), Piece::Card, row);
            self.art.pip(
                ui.painter(),
                if ok { 0 } else { 1 },
                egui::Rect::from_center_size(
                    egui::pos2(row.left() + 24.0, row.center().y),
                    Vec2::splat(18.0),
                ),
            );
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
