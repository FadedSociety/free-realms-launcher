//! (R) Launcher art direction and image layout, not recovered historical UI code.
use eframe::egui::{self, Color32, FontId, RichText, Stroke, Vec2};
use std::collections::VecDeque;

pub const INK: Color32 = Color32::from_rgb(243, 249, 255);
pub const DARK_INK: Color32 = Color32::from_rgb(38, 50, 66);
pub const BLUE: Color32 = Color32::from_rgb(7, 42, 91);
pub const FOCUS_GLOW: Color32 = Color32::from_rgb(113, 237, 255);

/// The UI art ships at twice its on-screen size (redesign 2026-10-30).
const ART_SCALE: f32 = 0.5;

pub fn unit_uv() -> egui::Rect {
    egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0))
}

pub fn display(size: f32) -> FontId {
    FontId::new(size, egui::FontFamily::Name("Lilita One".into()))
}

/// Redesign v0.3.1 art pieces drawn by [`UiArt::piece`].
#[derive(Clone, Copy, Debug)]
pub enum Piece {
    TopBar,
    HeroFrame,
    Card,
    ModeCard,
    ModeCardOn,
    Teal,
    ThumbRing,
}

#[derive(Clone, Copy)]
pub enum ButtonSkin {
    Standard,
    Play,
    Tab { selected: bool },
}

struct GrowingVine {
    texture: egui::TextureHandle,
    source: egui::ColorImage,
    order: Vec<u32>,
    last_cutoff: Option<u32>,
}

pub const VINE_ROOTS: [[usize; 2]; 4] = [[10, 161], [11, 180], [8, 161], [9, 189]];
pub struct UiArt {
    backdrop: egui::TextureHandle,
    buttons: [egui::TextureHandle; 4],
    play: [egui::TextureHandle; 4],
    tabs: [egui::TextureHandle; 2],
    panel: egui::TextureHandle,
    list_row: egui::TextureHandle,
    text_field: egui::TextureHandle,
    pips: [egui::TextureHandle; 4],
    hero: egui::TextureHandle,
    top_bar: egui::TextureHandle,
    hero_frame: egui::TextureHandle,
    mode_card: [egui::TextureHandle; 2],
    teal: egui::TextureHandle,
    thumb_ring: egui::TextureHandle,
    icons: [egui::TextureHandle; 3],
    vines: Vec<GrowingVine>,
}

impl UiArt {
    pub fn load(ctx: &egui::Context) -> Result<Self, String> {
        macro_rules! texture {
            ($name:literal) => {
                load_texture(ctx, $name, include_bytes!(concat!("../assets/ui/", $name)))?
            };
        }
        let vines = vec![
            load_vine(
                ctx,
                "vine 1",
                include_bytes!("../assets/ui/vine_1.png"),
                VINE_ROOTS[0],
            )?,
            load_vine(
                ctx,
                "vine 2",
                include_bytes!("../assets/ui/vine_2.png"),
                VINE_ROOTS[1],
            )?,
            load_vine(
                ctx,
                "vine 3",
                include_bytes!("../assets/ui/vine_3.png"),
                VINE_ROOTS[2],
            )?,
            load_vine(
                ctx,
                "vine 4",
                include_bytes!("../assets/ui/vine_4.png"),
                VINE_ROOTS[3],
            )?,
        ];
        Ok(Self {
            backdrop: texture!("backdrop.png"),
            buttons: [
                texture!("button_normal.png"),
                texture!("button_hover.png"),
                texture!("button_pressed.png"),
                texture!("button_disabled.png"),
            ],
            play: [
                texture!("play_normal.png"),
                texture!("play_hover.png"),
                texture!("play_pressed.png"),
                texture!("play_disabled.png"),
            ],
            tabs: [texture!("tab_selected.png"), texture!("tab_unselected.png")],
            panel: texture!("panel.png"),
            list_row: texture!("list_row.png"),
            text_field: texture!("text_field.png"),
            pips: [
                texture!("pip_online.png"),
                texture!("pip_pending.png"),
                texture!("pip_error.png"),
                texture!("pip_disabled.png"),
            ],
            hero: texture!("hero_cast.png"),
            top_bar: texture!("top_bar.png"),
            hero_frame: texture!("hero_frame.png"),
            mode_card: [texture!("mode_card.png"), texture!("mode_card_on.png")],
            teal: texture!("teal_button.png"),
            thumb_ring: texture!("thumb_ring.png"),
            icons: [
                texture!("icon_leaf_green.png"),
                texture!("icon_leaf_blue.png"),
                texture!("icon_sprig_blue.png"),
            ],
            vines,
        })
    }

    pub fn repaint_vines(&mut self, growth: f32) -> bool {
        let mut changed = false;
        for vine in &mut self.vines {
            let cutoff = growth_cutoff(&vine.order, growth);
            if vine.last_cutoff == Some(cutoff) {
                continue;
            }
            let mut image = vine.source.clone();
            for (pixel, order) in image.pixels.iter_mut().zip(&vine.order) {
                if *order == u32::MAX || *order >= cutoff {
                    *pixel = Color32::TRANSPARENT;
                }
            }
            vine.texture.set(image, egui::TextureOptions::LINEAR);
            vine.last_cutoff = Some(cutoff);
            changed = true;
        }
        changed
    }

    pub fn draw_backdrop(&self, painter: &egui::Painter, rect: egui::Rect) {
        let [width, height] = self.backdrop.size();
        let uv = cover_uv([width, height], rect);
        painter.image(self.backdrop.id(), rect, uv, Color32::WHITE);
        painter.rect_filled(rect, 0, Color32::from_rgba_unmultiplied(5, 20, 43, 102));
    }

    pub fn icon(&self, painter: &egui::Painter, index: usize, rect: egui::Rect) {
        if let Some(texture) = self.icons.get(index) {
            painter.image(texture.id(), rect, unit_uv(), Color32::WHITE);
        }
    }

    /// Redesign v0.3.1: draw one piece of the new art (9-slice unless it is a fixed ring).
    pub fn piece(&self, painter: &egui::Painter, piece: Piece, rect: egui::Rect) {
        let (texture, cx, cy) = match piece {
            Piece::TopBar => (&self.top_bar, 60.0, 58.0),
            Piece::HeroFrame => (&self.hero_frame, 170.0, 170.0),
            Piece::Card => (&self.list_row, 56.0, 56.0),
            Piece::ModeCard => (&self.mode_card[0], 56.0, 56.0),
            Piece::ModeCardOn => (&self.mode_card[1], 60.0, 60.0),
            Piece::Teal => (&self.teal, 48.0, 46.0),
            Piece::ThumbRing => {
                painter.image(self.thumb_ring.id(), rect, unit_uv(), Color32::WHITE);
                return;
            }
        };
        painter.add(nine_slice(texture, rect, cx, cy, None));
    }

    /// The cast that stands over the play dock (redesign 2026-10-30).
    pub fn hero(&self) -> &egui::TextureHandle {
        &self.hero
    }

    pub fn pip(&self, painter: &egui::Painter, index: usize, rect: egui::Rect) {
        if let Some(texture) = self.pips.get(index) {
            painter.image(texture.id(), rect, unit_uv(), Color32::WHITE);
        }
    }
}

fn load_texture(
    ctx: &egui::Context,
    name: &str,
    bytes: &[u8],
) -> Result<egui::TextureHandle, String> {
    let image = image::load_from_memory(bytes)
        .map_err(|_| format!("The bundled {name} art could not be decoded."))?
        .into_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    Ok(ctx.load_texture(
        name,
        egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw()),
        egui::TextureOptions::LINEAR,
    ))
}

fn load_vine(
    ctx: &egui::Context,
    name: &str,
    bytes: &[u8],
    root: [usize; 2],
) -> Result<GrowingVine, String> {
    let rgba = image::load_from_memory(bytes)
        .map_err(|_| format!("The bundled {name} art could not be decoded."))?
        .into_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    let source = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
    let order = growth_order(&source, root);
    let texture = ctx.load_texture(
        name,
        egui::ColorImage::filled(size, Color32::TRANSPARENT),
        egui::TextureOptions::LINEAR,
    );
    Ok(GrowingVine {
        texture,
        source,
        order,
        last_cutoff: Some(0),
    })
}

fn growth_order(image: &egui::ColorImage, root: [usize; 2]) -> Vec<u32> {
    let [width, height] = image.size;
    let mut order = vec![u32::MAX; width * height];
    if root[0] >= width || root[1] >= height {
        return order;
    }
    let root_index = root[1] * width + root[0];
    if image.pixels[root_index].a() == 0 {
        return order;
    }
    order[root_index] = 0;
    let mut queue = VecDeque::from([root_index]);
    while let Some(index) = queue.pop_front() {
        let x = index % width;
        let y = index / width;
        let next_order = order[index] + 1;
        for dy in -1_i32..=1 {
            for dx in -1_i32..=1 {
                if dx == 0 && dy == 0 {
                    continue;
                }
                let nx = x as i32 + dx;
                let ny = y as i32 + dy;
                if nx < 0 || ny < 0 || nx >= width as i32 || ny >= height as i32 {
                    continue;
                }
                let next = ny as usize * width + nx as usize;
                if image.pixels[next].a() > 0 && order[next] == u32::MAX {
                    order[next] = next_order;
                    queue.push_back(next);
                }
            }
        }
    }
    order
}

fn growth_cutoff(order: &[u32], growth: f32) -> u32 {
    if !growth.is_finite() || growth <= 0.0 {
        return 0;
    }
    if growth >= 1.0 {
        return u32::MAX;
    }
    let max = order
        .iter()
        .copied()
        .filter(|d| *d != u32::MAX)
        .max()
        .unwrap_or(0);
    ((max + 1) as f32 * growth).ceil() as u32
}

fn add_quad(mesh: &mut egui::Mesh, texture_size: [usize; 2], dst: egui::Rect, src: egui::Rect) {
    if dst.width() <= 0.0 || dst.height() <= 0.0 || src.width() <= 0.0 || src.height() <= 0.0 {
        return;
    }
    let uv = egui::Rect::from_min_max(
        egui::pos2(
            src.left() / texture_size[0] as f32,
            src.top() / texture_size[1] as f32,
        ),
        egui::pos2(
            src.right() / texture_size[0] as f32,
            src.bottom() / texture_size[1] as f32,
        ),
    );
    mesh.add_rect_with_uv(dst, uv, Color32::WHITE);
}

fn tile_horizontal(
    mesh: &mut egui::Mesh,
    texture_size: [usize; 2],
    dst: egui::Rect,
    src: egui::Rect,
    bead_period: f32,
) {
    if dst.width() <= 0.0 || src.width() <= 0.0 || bead_period <= 0.0 {
        return;
    }
    let (tiles, periods_per_tile) = horizontal_tile_layout(dst.width(), src.width(), bead_period);
    let source_width = periods_per_tile as f32 * bead_period;
    let source_left =
        (src.center().x - source_width * 0.5).clamp(0.0, texture_size[0] as f32 - source_width);
    let tile_src = egui::Rect::from_min_max(
        egui::pos2(source_left, src.top()),
        egui::pos2(source_left + source_width, src.bottom()),
    );
    let tile_width = dst.width() / tiles as f32;
    for index in 0..tiles {
        add_quad(
            mesh,
            texture_size,
            egui::Rect::from_min_size(
                egui::pos2(dst.left() + tile_width * index as f32, dst.top()),
                Vec2::new(tile_width, dst.height()),
            ),
            tile_src,
        );
    }
}

fn horizontal_tile_layout(dst_width: f32, src_width: f32, bead_period: f32) -> (usize, usize) {
    let source_periods = (src_width / bead_period).round().max(1.0) as usize;
    let destination_periods = (dst_width / bead_period).round().max(1.0) as usize;
    let tiles = destination_periods.div_ceil(source_periods);
    let ideal = dst_width / (tiles as f32 * bead_period);
    let lower = (ideal.floor() as usize).clamp(1, source_periods);
    let upper = (ideal.ceil() as usize).clamp(1, source_periods);
    let lower_error = (dst_width / (tiles * lower) as f32 - bead_period).abs();
    let upper_error = (dst_width / (tiles * upper) as f32 - bead_period).abs();
    let periods_per_tile = if lower_error <= upper_error {
        lower
    } else {
        upper
    };
    (tiles, periods_per_tile)
}

fn cover_uv(texture_size: [usize; 2], rect: egui::Rect) -> egui::Rect {
    let source_aspect = texture_size[0] as f32 / texture_size[1] as f32;
    let target_aspect = rect.width() / rect.height().max(1.0);
    if source_aspect > target_aspect {
        let visible = target_aspect / source_aspect;
        egui::Rect::from_min_max(
            egui::pos2((1.0 - visible) * 0.5, 0.0),
            egui::pos2((1.0 + visible) * 0.5, 1.0),
        )
    } else {
        let visible = source_aspect / target_aspect;
        egui::Rect::from_min_max(
            egui::pos2(0.0, (1.0 - visible) * 0.5),
            egui::pos2(1.0, (1.0 + visible) * 0.5),
        )
    }
}

/// Fixed corners, repeated edge strips, and a stretched centre keep bump spacing stable.
fn nine_slice(
    texture: &egui::TextureHandle,
    rect: egui::Rect,
    corner_x: f32,
    corner_y: f32,
    bead_period: Option<f32>,
) -> egui::Shape {
    let [width, height] = texture.size();
    let sw = width as f32;
    let sh = height as f32;
    // The art is drawn at 2x: corners show at half their source size, scaled down evenly (never
    // squashed) when the rect is too small to hold them.
    let cx = corner_x.min(sw * 0.5 - 1.0);
    let cy = corner_y.min(sh * 0.5 - 1.0);
    let k = ART_SCALE
        .min(rect.width() * 0.5 / cx)
        .min(rect.height() * 0.5 / cy);
    let (dcx, dcy) = (cx * k, cy * k);
    let sx = [0.0, cx, sw - cx, sw];
    let sy = [0.0, cy, sh - cy, sh];
    let dx = [
        rect.left(),
        rect.left() + dcx,
        rect.right() - dcx,
        rect.right(),
    ];
    let dy = [
        rect.top(),
        rect.top() + dcy,
        rect.bottom() - dcy,
        rect.bottom(),
    ];
    let mut mesh = egui::Mesh::with_texture(texture.id());
    for (row, col) in [(0, 0), (0, 2), (2, 0), (2, 2)] {
        add_quad(
            &mut mesh,
            [width, height],
            egui::Rect::from_min_max(
                egui::pos2(dx[col], dy[row]),
                egui::pos2(dx[col + 1], dy[row + 1]),
            ),
            egui::Rect::from_min_max(
                egui::pos2(sx[col], sy[row]),
                egui::pos2(sx[col + 1], sy[row + 1]),
            ),
        );
    }
    let top_dst = egui::Rect::from_min_max(egui::pos2(dx[1], dy[0]), egui::pos2(dx[2], dy[1]));
    let top_src = egui::Rect::from_min_max(egui::pos2(sx[1], sy[0]), egui::pos2(sx[2], sy[1]));
    let bottom_dst = egui::Rect::from_min_max(egui::pos2(dx[1], dy[2]), egui::pos2(dx[2], dy[3]));
    let bottom_src = egui::Rect::from_min_max(egui::pos2(sx[1], sy[2]), egui::pos2(sx[2], sy[3]));
    if let Some(period) = bead_period {
        tile_horizontal(&mut mesh, [width, height], top_dst, top_src, period);
        tile_horizontal(&mut mesh, [width, height], bottom_dst, bottom_src, period);
    } else {
        add_quad(&mut mesh, [width, height], top_dst, top_src);
        add_quad(&mut mesh, [width, height], bottom_dst, bottom_src);
    }
    add_quad(
        &mut mesh,
        [width, height],
        egui::Rect::from_min_max(egui::pos2(dx[0], dy[1]), egui::pos2(dx[1], dy[2])),
        egui::Rect::from_min_max(egui::pos2(sx[0], sy[1]), egui::pos2(sx[1], sy[2])),
    );
    add_quad(
        &mut mesh,
        [width, height],
        egui::Rect::from_min_max(egui::pos2(dx[2], dy[1]), egui::pos2(dx[3], dy[2])),
        egui::Rect::from_min_max(egui::pos2(sx[2], sy[1]), egui::pos2(sx[3], sy[2])),
    );
    add_quad(
        &mut mesh,
        [width, height],
        egui::Rect::from_min_max(egui::pos2(dx[1], dy[1]), egui::pos2(dx[2], dy[2])),
        egui::Rect::from_min_max(egui::pos2(sx[1], sy[1]), egui::pos2(sx[2], sy[2])),
    );
    egui::Shape::mesh(mesh)
}

pub fn focus_shapes(rect: egui::Rect) -> [egui::Shape; 2] {
    [
        egui::Shape::rect_stroke(
            rect.expand(4.0),
            18,
            Stroke::new(4.0_f32, FOCUS_GLOW),
            egui::StrokeKind::Outside,
        ),
        egui::Shape::rect_stroke(
            rect.expand(8.0),
            20,
            Stroke::new(1.5_f32, Color32::WHITE),
            egui::StrokeKind::Outside,
        ),
    ]
}

pub fn focus_outline(ui: &egui::Ui, response: &egui::Response) {
    if response.has_focus() {
        for shape in focus_shapes(response.rect) {
            ui.painter().add(shape);
        }
    }
}

fn button_texture<'a>(
    art: &'a UiArt,
    skin: ButtonSkin,
    enabled: bool,
    hovered: bool,
    pressed: bool,
) -> (&'a egui::TextureHandle, f32, f32, Option<f32>) {
    // Corners in source pixels of the 2x art (rounded ends are half the art's height).
    let state = if !enabled {
        3
    } else if pressed {
        2
    } else {
        usize::from(hovered)
    };
    match skin {
        ButtonSkin::Tab { selected: true } => (&art.tabs[0], 56.0, 54.0, None),
        ButtonSkin::Tab { selected: false } => (&art.tabs[1], 48.0, 46.0, None),
        ButtonSkin::Play => (&art.play[state], 160.0, 88.0, None),
        ButtonSkin::Standard => (&art.buttons[state], 48.0, 46.0, None),
    }
}

/// Keep egui's real Button for keyboard focus, Space/Enter, accesskit and disabled semantics.
pub fn button(
    ui: &mut egui::Ui,
    art: &UiArt,
    text: &str,
    size: Vec2,
    enabled: bool,
    skin: ButtonSkin,
) -> egui::Response {
    let painter = ui.painter().clone();
    let slot = painter.add(egui::Shape::Noop);
    let font = display(if matches!(skin, ButtonSkin::Play) {
        30.0
    } else {
        19.0
    });
    let response = ui
        .allocate_ui_with_layout(
            size,
            egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
            |ui| {
                ui.visuals_mut().disabled_alpha = 1.0;
                ui.add_enabled(
                    enabled,
                    egui::Button::new(
                        RichText::new(text)
                            .font(font.clone())
                            .color(Color32::TRANSPARENT),
                    )
                    .frame(false)
                    .min_size(size),
                )
            },
        )
        .inner;
    let (texture, cx, cy, bead_period) = button_texture(
        art,
        skin,
        enabled,
        response.hovered(),
        response.is_pointer_button_down_on(),
    );
    // The selected tab's image has a leaf above a shorter body: grow its box so the green body covers the
    // plank's footprint, `rect.shrink(2)` (margins measured on both tab images at the 0.5 art scale).
    let art_rect = match skin {
        ButtonSkin::Tab { selected: true } => egui::Rect::from_min_max(
            response.rect.min - Vec2::new(0.0, 11.5),
            response.rect.max + Vec2::new(7.0, -1.0),
        ),
        _ => response.rect.shrink(1.0),
    };
    let mut shapes = vec![nine_slice(texture, art_rect, cx, cy, bead_period)];
    if response.has_focus() {
        shapes.extend(focus_shapes(response.rect));
    }
    let at = response.rect.center();
    // A dark outline keeps the white caption clear on both bright and shaded green pixels.
    let outline_color = Color32::from_rgb(7, 18, 15);
    let caption = painter.layout_no_wrap(text.to_owned(), font.clone(), Color32::WHITE);
    let outline = painter.layout_no_wrap(text.to_owned(), font, outline_color);
    for offset in [
        Vec2::new(-1.2, -1.2),
        Vec2::new(0.0, -1.5),
        Vec2::new(1.2, -1.2),
        Vec2::new(-1.5, 0.0),
        Vec2::new(1.5, 0.0),
        Vec2::new(-1.2, 1.2),
        Vec2::new(0.0, 1.5),
        Vec2::new(1.2, 1.2),
    ] {
        shapes.push(egui::Shape::galley(
            at - outline.size() / 2.0 + offset,
            outline.clone(),
            outline_color,
        ));
    }
    shapes.push(egui::Shape::galley(
        at - caption.size() / 2.0,
        caption,
        Color32::WHITE,
    ));
    painter.set(slot, egui::Shape::Vec(shapes));
    response
}

pub fn text_field(
    ui: &mut egui::Ui,
    art: &UiArt,
    value: &mut String,
    hint: &str,
    size: Vec2,
    max_chars: usize,
) -> egui::Response {
    text_field_impl(ui, art, value, hint, size, max_chars, true, false)
}

pub fn read_only_text_field(
    ui: &mut egui::Ui,
    art: &UiArt,
    value: &str,
    hint: &str,
    size: Vec2,
) -> egui::Response {
    let mut displayed = value.to_owned();
    text_field_impl(
        ui,
        art,
        &mut displayed,
        hint,
        size,
        value.chars().count(),
        false,
        true,
    )
}

fn text_field_impl(
    ui: &mut egui::Ui,
    art: &UiArt,
    value: &mut String,
    hint: &str,
    size: Vec2,
    max_chars: usize,
    interactive: bool,
    monospace: bool,
) -> egui::Response {
    let painter = ui.painter().clone();
    let slot = painter.add(egui::Shape::Noop);
    let inner = ui.allocate_ui_with_layout(
        size,
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.add_space(18.0);
            ui.visuals_mut().override_text_color = Some(DARK_INK);
            let mut editor = egui::TextEdit::singleline(value)
                .hint_text(hint)
                .vertical_align(egui::Align::Center)
                .char_limit(max_chars)
                .frame(false)
                .interactive(interactive);
            if monospace {
                editor = editor.font(egui::TextStyle::Monospace);
            }
            ui.add_sized(
                Vec2::new((size.x - 36.0).max(40.0), (size.y - 12.0).max(32.0)),
                editor,
            )
        },
    );
    let response = inner.inner;
    let rect = inner.response.rect;
    let mut shapes = vec![nine_slice(
        &art.text_field,
        rect.shrink(1.0),
        44.0,
        44.0,
        None,
    )];
    painter.set(slot, egui::Shape::Vec(shapes));
    if response.has_focus() {
        shapes = focus_shapes(rect).into();
        for shape in shapes {
            painter.add(shape);
        }
    }
    response
}

pub fn panel_shape(art: &UiArt, rect: egui::Rect) -> egui::Shape {
    nine_slice(&art.panel, rect, 70.0, 70.0, None)
}

/// The client's `UI/Resource/Fonts/FreeRealms.ttf`, the game's own UI font, when the player's copy has
/// the original file (size and SHA-256 pinned, so a damaged or swapped file is never parsed).
pub fn game_font(client: &std::path::Path) -> Option<Vec<u8>> {
    const SIZE: u64 = 44_344;
    const SHA256: &str = "3fd6fc16574286fd6f95b133d40e6cc8f5c108c3b16570fa1a2ea22c71f8d97c";
    let path = client
        .join("UI")
        .join("Resource")
        .join("Fonts")
        .join("FreeRealms.ttf");
    if std::fs::metadata(&path).ok()?.len() != SIZE {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    let hash = osfr_launcher::patch::sha256(&bytes).ok()?;
    hash.eq_ignore_ascii_case(SHA256).then_some(bytes)
}

/// Lilita One for display text; body text in the game's font when the client has it, else egui's default.
pub fn set_fonts(ctx: &egui::Context, game_font: Option<Vec<u8>>) {
    let mut fonts = egui::FontDefinitions::default();
    if let Some(bytes) = game_font {
        fonts.font_data.insert(
            "FreeRealms".into(),
            egui::FontData::from_owned(bytes).into(),
        );
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, "FreeRealms".into());
    }
    fonts.font_data.insert(
        "Lilita One".into(),
        egui::FontData::from_static(include_bytes!("../assets/fonts/LilitaOne-Regular.ttf")).into(),
    );
    fonts.families.insert(
        egui::FontFamily::Name("Lilita One".into()),
        vec!["Lilita One".into(), "Ubuntu-Light".into()],
    );
    ctx.set_fonts(fonts);
}

pub fn install(ctx: &egui::Context) {
    set_fonts(ctx, None);
    let mut style = (*ctx.style()).clone();
    style.visuals = egui::Visuals::dark();
    style.visuals.override_text_color = Some(INK);
    style.visuals.panel_fill = BLUE;
    style.visuals.window_fill = BLUE;
    style.visuals.extreme_bg_color = Color32::from_rgb(246, 249, 255);
    style.visuals.selection.bg_fill = Color32::from_rgb(34, 126, 215);
    style.visuals.selection.stroke = Stroke::new(1.5_f32, Color32::WHITE);
    style.visuals.hyperlink_color = Color32::from_rgb(164, 229, 255);
    for widget in [
        &mut style.visuals.widgets.inactive,
        &mut style.visuals.widgets.noninteractive,
    ] {
        widget.bg_fill = Color32::from_rgba_unmultiplied(18, 82, 143, 190);
        widget.weak_bg_fill = widget.bg_fill;
        widget.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(77, 170, 238));
        widget.corner_radius = 8.into();
    }
    style.visuals.widgets.hovered.bg_fill = Color32::from_rgba_unmultiplied(33, 111, 181, 210);
    style.visuals.widgets.hovered.bg_stroke = Stroke::new(2.0_f32, FOCUS_GLOW);
    style.visuals.widgets.active.bg_fill = Color32::from_rgba_unmultiplied(12, 72, 130, 230);
    style.visuals.widgets.active.bg_stroke = Stroke::new(2.0_f32, Color32::WHITE);
    style.visuals.widgets.hovered.corner_radius = 8.into();
    style.visuals.widgets.active.corner_radius = 8.into();
    style.spacing.item_spacing = Vec2::new(8.0, 6.0);
    style.spacing.button_padding = Vec2::new(14.0, 8.0);
    style.spacing.interact_size = Vec2::new(44.0, 44.0);
    style.spacing.scroll = egui::style::ScrollStyle::solid();
    style
        .text_styles
        .insert(egui::TextStyle::Body, FontId::proportional(16.0));
    style
        .text_styles
        .insert(egui::TextStyle::Small, FontId::proportional(13.0));
    style
        .text_styles
        .insert(egui::TextStyle::Button, FontId::proportional(18.0));
    style
        .text_styles
        .insert(egui::TextStyle::Heading, display(30.0));
    ctx.set_style_of(egui::Theme::Dark, style.clone());
    ctx.set_style_of(egui::Theme::Light, style);
}

pub fn heading(ui: &mut egui::Ui, title: &str, detail: &str) {
    let size = if ui.ctx().content_rect().height() < 650.0 {
        26.0
    } else {
        30.0
    };
    ui.heading(RichText::new(title).font(display(size)).color(INK));
    if !detail.is_empty() {
        ui.label(RichText::new(detail).color(INK));
    }
    ui.add_space(2.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_glow_is_painted_over_the_button_skin() {
        let ctx = egui::Context::default();
        install(&ctx);
        let art = UiArt::load(&ctx).unwrap();
        let mut response = None;
        let draw = |ctx: &egui::Context, response: &mut Option<egui::Response>| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        Vec2::new(720.0, 620.0),
                    )),
                    focused: true,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        *response = Some(button(
                            ui,
                            &art,
                            "PLAY NOW!",
                            Vec2::new(224.0, 56.0),
                            true,
                            ButtonSkin::Play,
                        ));
                    });
                },
            )
        };
        let _ = draw(&ctx, &mut response);
        ctx.memory_mut(|memory| memory.request_focus(response.as_ref().unwrap().id));
        let output = draw(&ctx, &mut response);
        assert!(response.as_ref().unwrap().has_focus());
        assert!(output.shapes.iter().any(|shape| match &shape.shape {
            egui::Shape::Vec(shapes) => shapes.iter().any(|shape| matches!(shape,
                egui::Shape::Rect(rect) if rect.stroke.color == FOCUS_GLOW && rect.stroke.width == 4.0)),
            _ => false,
        }), "controller focus must draw the bright glow above the image skin");
    }

    #[test]
    fn two_x_corners_draw_at_half_size_and_shrink_evenly() {
        let ctx = egui::Context::default();
        let texture = ctx.load_texture(
            "plank",
            egui::ColorImage::new([200, 100], vec![Color32::WHITE; 200 * 100]),
            egui::TextureOptions::LINEAR,
        );
        let corner_width = |rect: egui::Rect| match nine_slice(&texture, rect, 40.0, 40.0, None) {
            egui::Shape::Mesh(mesh) => {
                let left = rect.left();
                mesh.vertices
                    .iter()
                    .map(|v| v.pos.x - left)
                    .filter(|x| *x > 0.5)
                    .fold(f32::MAX, f32::min)
            }
            _ => panic!("nine_slice draws a mesh"),
        };
        let big = egui::Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(400.0, 200.0));
        assert!(
            (corner_width(big) - 20.0).abs() < 0.01,
            "half of the 40 px source corner"
        );
        let small = egui::Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(400.0, 20.0));
        assert!(
            (corner_width(small) - 10.0).abs() < 0.01,
            "both axes scale by the same factor"
        );
    }

    #[test]
    fn backdrop_image_covers_the_full_viewport_with_cover_fit() {
        let ctx = egui::Context::default();
        let art = UiArt::load(&ctx).unwrap();
        let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(1280.0, 800.0));
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(viewport),
                ..Default::default()
            },
            |ctx| {
                art.draw_backdrop(
                    &ctx.layer_painter(egui::LayerId::background()),
                    ctx.content_rect(),
                );
            },
        );
        let bounds = output.shapes.iter().find_map(|shape| match &shape.shape {
            egui::Shape::Mesh(mesh) => {
                let min_x = mesh
                    .vertices
                    .iter()
                    .map(|v| v.pos.x)
                    .fold(f32::INFINITY, f32::min);
                let min_y = mesh
                    .vertices
                    .iter()
                    .map(|v| v.pos.y)
                    .fold(f32::INFINITY, f32::min);
                let max_x = mesh
                    .vertices
                    .iter()
                    .map(|v| v.pos.x)
                    .fold(f32::NEG_INFINITY, f32::max);
                let max_y = mesh
                    .vertices
                    .iter()
                    .map(|v| v.pos.y)
                    .fold(f32::NEG_INFINITY, f32::max);
                Some(egui::Rect::from_min_max(
                    egui::pos2(min_x, min_y),
                    egui::pos2(max_x, max_y),
                ))
            }
            _ => None,
        });
        assert_eq!(bounds, Some(viewport));
        let uv = cover_uv([1280, 720], viewport);
        assert!(uv.min.x > 0.0 && uv.max.x < 1.0);
        assert_eq!(uv.min.y, 0.0);
        assert_eq!(uv.max.y, 1.0);
    }
}
