//! Dynamic tray-icon rendering (`tiny-skia` + a built-in 5×7 bitmap font) and
//! the native context menu (Tauri `Menu`). Because Windows/Linux trays are icon-only, the
//! percentage and warning state are drawn *into* the icon bitmap; the tooltip
//! carries the textual detail on all OSes.

use anyhow::Result;
use tauri::image::Image;
use tauri::menu::{CheckMenuItemBuilder, Menu, MenuBuilder, MenuItemBuilder, SubmenuBuilder};
use tauri::{AppHandle, Wry};
use tiny_skia::{Color, LineCap, LineJoin, Paint, PathBuilder, Pixmap, Rect, Stroke, Transform};

use crate::app::AppState;
use crate::prefs::{IndicatorPrefs, IndicatorStyle};
use crate::provider::Provider;

/// Rendered icon size (px). Rendered larger than the tray slot so the OS
/// downscales for crisp text; the percentage font scales to fit this width.
const ICON: u32 = 64;

/// Brand coral.
const CORAL: (u8, u8, u8) = (0xD9, 0x77, 0x57);

/// Inputs that determine what the tray icon should look like right now.
pub struct TrayVisual {
    /// Headline figure for the square icon (per the metric pref).
    pub utilization: Option<f64>,
    /// Per-window figures for the wide (macOS) two-bar layout.
    pub five_hour: Option<f64>,
    pub seven_day: Option<f64>,
    pub stale: bool,
    pub prefs: IndicatorPrefs,
}

impl TrayVisual {
    /// Derive the visual from current state for the active account.
    pub fn from_state(state: &AppState) -> Self {
        let report = state.primary_key().and_then(|k| state.usage.get(&k));
        TrayVisual {
            utilization: state.indicator_utilization(),
            five_hour: report.and_then(|r| r.five_hour.utilization),
            seven_day: report.and_then(|r| r.seven_day.utilization),
            stale: state.active_is_stale(),
            prefs: state.prefs,
        }
    }
}

/// Warning tier driving the badge color.
fn warning_color(pct: f64) -> Option<Color> {
    if pct >= 90.0 {
        Some(Color::from_rgba8(0xE5, 0x3E, 0x3E, 255)) // red
    } else if pct >= 75.0 {
        Some(Color::from_rgba8(0xF5, 0xA1, 0x23, 255)) // orange
    } else {
        None
    }
}

/// Render the tray icon to an RGBA `Image`, adapting to the platform: macOS menu
/// bar items accept a variable-width image, so they get the wide two-bar layout;
/// Windows and Linux trays are square, so they get the gauge/percent icon.
///
/// `cfg!` (not `#[cfg]`) keeps both renderers compiled and referenced on every
/// platform — the dead branch is dropped by the optimizer, not flagged unused.
pub fn render_icon(v: &TrayVisual) -> Result<Image<'static>> {
    if cfg!(target_os = "macos") {
        render_rectangular(v)
    } else {
        render_square(v)
    }
}

/// Square gauge/percent icon for Windows & Linux trays (and the macOS dock).
fn render_square(v: &TrayVisual) -> Result<Image<'static>> {
    let mut pixmap = Pixmap::new(ICON, ICON).expect("nonzero pixmap");
    let alpha = if v.stale { 0.45 } else { 1.0 };

    let pct = v.utilization.map(|u| (u * 100.0).round());

    // Gauge / checkered motif background (skipped in percentOnly).
    if v.prefs.style.shows_gauge() {
        draw_gauge(&mut pixmap, pct, alpha);
    }

    // Percentage pill.
    if v.prefs.style.shows_percent() {
        let label = match pct {
            Some(p) => format!("{}%", p as i64),
            None => "–".to_string(),
        };
        draw_percent_pill(&mut pixmap, &label, pct, alpha);
    }

    // Warning badge dot in the corner.
    if let Some(p) = pct {
        if let Some(color) = warning_color(p) {
            draw_badge(&mut pixmap, color, alpha);
        }
    }

    Ok(Image::new_owned(pixmap.take(), ICON, ICON))
}

/// Wide indicator size (px). Rendered well above the macOS menu-bar height so
/// the system downscales it crisply; ~2.75:1 leaves room for two bars + labels.
const RECT_W: u32 = 176;
const RECT_H: u32 = 64;

/// Wide two-bar indicator for the macOS menu bar: the 5-hour and weekly windows
/// as horizontal progress bars with their percentages, color-coded by warning
/// tier. Background stays transparent so the menu bar shows through.
fn render_rectangular(v: &TrayVisual) -> Result<Image<'static>> {
    let mut pixmap = Pixmap::new(RECT_W, RECT_H).expect("nonzero pixmap");
    let alpha = if v.stale { 0.45 } else { 1.0 };

    let cell = 2_i32;
    let glyph_w = GLYPH_W as i32 * cell;
    let glyph_h = GLYPH_H as i32 * cell;
    let gap = cell;

    let row_h = RECT_H as i32 / 2;
    let left = 6;
    let right = 6;
    let label_w = 2 * glyph_w + gap; // "5h" / "7d"
    let pct_w = 4 * glyph_w + 3 * gap; // room for up to "100%"
    let bar_x = left + label_w + 6;
    let bar_w = (RECT_W as i32 - right - pct_w - 6 - bar_x).max(8);
    let bar_h = 8_i32;

    for (i, (label, util)) in [("5h", v.five_hour), ("7d", v.seven_day)]
        .into_iter()
        .enumerate()
    {
        let row_y0 = i as i32 * row_h;
        let text_y = row_y0 + (row_h - glyph_h) / 2;
        let pct = util.map(|u| (u * 100.0).round());

        // Bar/percent color follows the warning tier; coral otherwise.
        let (cr, cg, cb) = match pct.and_then(warning_color) {
            Some(c) => (
                (c.red() * 255.0) as u8,
                (c.green() * 255.0) as u8,
                (c.blue() * 255.0) as u8,
            ),
            None => CORAL,
        };

        // Window label in muted gray.
        let mut lx = left;
        for ch in label.chars() {
            draw_glyph(
                &mut pixmap,
                glyph_for(ch),
                lx,
                text_y,
                cell,
                (0xB0, 0xB0, 0xB0),
                alpha,
            );
            lx += glyph_w + gap;
        }

        // Bar track, then proportional fill.
        let by = row_y0 + (row_h - bar_h) / 2;
        if let Some(rect) = Rect::from_xywh(bar_x as f32, by as f32, bar_w as f32, bar_h as f32) {
            let mut track = Paint::default();
            track.set_color(Color::from_rgba8(0x88, 0x88, 0x88, (90.0 * alpha) as u8));
            track.anti_alias = true;
            pixmap.fill_rect(rect, &track, Transform::identity(), None);
        }
        if let Some(p) = pct {
            let frac = (p / 100.0).clamp(0.0, 1.0) as f32;
            let fw = (bar_w as f32 * frac).max(2.0);
            if let Some(rect) = Rect::from_xywh(bar_x as f32, by as f32, fw, bar_h as f32) {
                let mut fill = Paint::default();
                fill.set_color(Color::from_rgba8(cr, cg, cb, (255.0 * alpha) as u8));
                fill.anti_alias = true;
                pixmap.fill_rect(rect, &fill, Transform::identity(), None);
            }
        }

        // Percentage, right-aligned, in the tier color.
        let text = match pct {
            Some(p) => format!("{}%", p as i64),
            None => "–".to_string(),
        };
        let chars: Vec<char> = text.chars().collect();
        let text_w = chars.len() as i32 * glyph_w + (chars.len() as i32 - 1).max(0) * gap;
        let mut px = RECT_W as i32 - right - text_w;
        for ch in chars {
            draw_glyph(
                &mut pixmap,
                glyph_for(ch),
                px,
                text_y,
                cell,
                (cr, cg, cb),
                alpha,
            );
            px += glyph_w + gap;
        }
    }

    Ok(Image::new_owned(pixmap.take(), RECT_W, RECT_H))
}

fn draw_gauge(pixmap: &mut Pixmap, pct: Option<f64>, alpha: f32) {
    let cx = ICON as f32 / 2.0;
    let cy = ICON as f32 / 2.0;
    let r = ICON as f32 * 0.42;

    // Track ring.
    let mut track = Paint::default();
    track.set_color(Color::from_rgba8(0x88, 0x88, 0x88, (255.0 * alpha) as u8));
    track.anti_alias = true;
    if let Some(circle) = PathBuilder::from_circle(cx, cy, r) {
        let stroke = Stroke {
            width: 6.0,
            ..Default::default()
        };
        pixmap.stroke_path(&circle, &track, &stroke, Transform::identity(), None);
    }

    // Fill arc proportional to utilization.
    if let Some(p) = pct {
        let frac = (p / 100.0).clamp(0.0, 1.0) as f32;
        let mut paint = Paint::default();
        let (cr, cg, cb) = CORAL;
        paint.set_color(Color::from_rgba8(cr, cg, cb, (255.0 * alpha) as u8));
        paint.anti_alias = true;
        let mut pb = PathBuilder::new();
        let steps = (frac * 64.0).ceil().max(1.0) as usize;
        for i in 0..=steps {
            let t = i as f32 / 64.0;
            let ang = -std::f32::consts::FRAC_PI_2 + t * std::f32::consts::TAU;
            let x = cx + r * ang.cos();
            let y = cy + r * ang.sin();
            if i == 0 {
                pb.move_to(x, y);
            } else {
                pb.line_to(x, y);
            }
        }
        if let Some(path) = pb.finish() {
            let stroke = Stroke {
                width: 6.5,
                line_cap: LineCap::Round,
                line_join: LineJoin::Round,
                ..Default::default()
            };
            pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
        }
    }
}

/// Draw the `NN%` / `–` label centered, using a built-in 5×7 bitmap font (no
/// external font asset, so the build stays hermetic). A high-contrast pill sits
/// behind the digits so they stay legible on any taskbar ink.
fn draw_percent_pill(pixmap: &mut Pixmap, label: &str, pct: Option<f64>, alpha: f32) {
    let chars: Vec<char> = label.chars().collect();
    let n = chars.len() as i32;

    // Pick the largest cell (pixel) size whose laid-out width still fits the
    // icon with room for the pill padding. A glyph block is 5 cells wide plus a
    // 1-cell gap, so the label width is `cell * (6n - 1)`.
    let pill_pad: i32 = 3;
    let avail = ICON as i32 - 2 * pill_pad - 4;
    let denom = (6 * n - 1).max(1);
    let cell: i32 = (avail / denom).clamp(2, 6);

    let glyph_w = GLYPH_W as i32 * cell;
    let glyph_h = GLYPH_H as i32 * cell;
    let gap = cell; // inter-glyph spacing

    let total_w = n * glyph_w + (n - 1).max(0) * gap;
    let x0 = (ICON as i32 - total_w) / 2;
    let y0 = (ICON as i32 - glyph_h) / 2;

    // Pill background.
    let pad = pill_pad as f32;
    if let Some(rect) = Rect::from_xywh(
        x0 as f32 - pad,
        y0 as f32 - pad,
        total_w as f32 + pad * 2.0,
        glyph_h as f32 + pad * 2.0,
    ) {
        let mut bg = Paint::default();
        bg.set_color(Color::from_rgba8(0x16, 0x16, 0x16, (210.0 * alpha) as u8));
        bg.anti_alias = true;
        pixmap.fill_rect(rect, &bg, Transform::identity(), None);
    }

    // Text color follows the warning tier.
    let (tr, tg, tb) = match pct {
        Some(p) if p >= 90.0 => (0xFF, 0x6B, 0x6B),
        Some(p) if p >= 75.0 => (0xFF, 0xC1, 0x6B),
        _ => (0xFF, 0xFF, 0xFF),
    };

    let mut pen_x = x0;
    for c in chars {
        draw_glyph(pixmap, glyph_for(c), pen_x, y0, cell, (tr, tg, tb), alpha);
        pen_x += glyph_w + gap;
    }
}

/// Blit one 5×7 glyph at `cell`-pixel scale.
fn draw_glyph(
    pixmap: &mut Pixmap,
    glyph: [u8; GLYPH_H],
    ox: i32,
    oy: i32,
    cell: i32,
    (r, g, b): (u8, u8, u8),
    alpha: f32,
) {
    let a = (255.0 * alpha) as u8;
    let (w, h) = (pixmap.width() as i32, pixmap.height() as i32);
    for (row, bits) in glyph.iter().enumerate() {
        for col in 0..GLYPH_W {
            // Bit 4 (0x10) is the leftmost column.
            if bits & (1 << (GLYPH_W - 1 - col)) != 0 {
                for dy in 0..cell {
                    for dx in 0..cell {
                        let px = ox + col as i32 * cell + dx;
                        let py = oy + row as i32 * cell + dy;
                        if px >= 0 && py >= 0 && px < w && py < h {
                            blend_pixel(pixmap, px as u32, py as u32, r, g, b, a);
                        }
                    }
                }
            }
        }
    }
}

const GLYPH_W: usize = 5;
const GLYPH_H: usize = 7;

/// 5×7 bitmap glyphs for digits, `%`, and `–`. Each row's low 5 bits are the
/// pixels, bit 4 leftmost. Unknown chars render blank.
fn glyph_for(c: char) -> [u8; GLYPH_H] {
    match c {
        '0' => [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
        '1' => [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
        '2' => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F],
        '3' => [0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E],
        '4' => [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],
        '5' => [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E],
        '6' => [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E],
        '7' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],
        '9' => [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C],
        '%' => [0x18, 0x19, 0x02, 0x04, 0x08, 0x13, 0x03],
        '–' | '-' => [0x00, 0x00, 0x00, 0x1F, 0x00, 0x00, 0x00],
        // Lowercase letters for the wide indicator's window labels (5h / 7d).
        'h' => [0x10, 0x10, 0x10, 0x1C, 0x12, 0x12, 0x12],
        'd' => [0x02, 0x02, 0x02, 0x0E, 0x12, 0x12, 0x0E],
        _ => [0x00; GLYPH_H],
    }
}

fn draw_badge(pixmap: &mut Pixmap, color: Color, alpha: f32) {
    let r = ICON as f32 * 0.16;
    let cx = ICON as f32 - r - 1.0;
    let cy = r + 1.0;
    if let Some(circle) = PathBuilder::from_circle(cx, cy, r) {
        let mut paint = Paint::default();
        let c = color;
        paint.set_color(Color::from_rgba(c.red(), c.green(), c.blue(), alpha).unwrap());
        paint.anti_alias = true;
        pixmap.fill_path(
            &circle,
            &paint,
            tiny_skia::FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

/// Alpha-blend a single pixel onto the pixmap (premultiplied-safe enough for
/// the small glyph coverage we draw).
fn blend_pixel(pixmap: &mut Pixmap, x: u32, y: u32, r: u8, g: u8, b: u8, a: u8) {
    let idx = (y * pixmap.width() + x) as usize * 4;
    let data = pixmap.data_mut();
    let af = a as f32 / 255.0;
    let inv = 1.0 - af;
    data[idx] = (r as f32 * af + data[idx] as f32 * inv) as u8;
    data[idx + 1] = (g as f32 * af + data[idx + 1] as f32 * inv) as u8;
    data[idx + 2] = (b as f32 * af + data[idx + 2] as f32 * inv) as u8;
    data[idx + 3] = data[idx + 3].max(a);
}

/// Tooltip detail: `"<email> (Provider) — <win> NN% · …"` (+ stale), where the
/// windows are whatever the provider reports (e.g. `5h`/`7d`, or `Monthly`).
pub fn tooltip(state: &AppState) -> String {
    let (Some(key), Some((provider, email))) = (state.primary_key(), state.active_primary.clone())
    else {
        return "PitStopX — no active account".to_string();
    };
    match state.usage.get(&key) {
        Some(r) => {
            let parts: Vec<String> = [&r.five_hour, &r.seven_day]
                .into_iter()
                .filter_map(|w| {
                    Some(format!(
                        "{} {}",
                        w.label()?,
                        crate::format::percent(w.utilization)
                    ))
                })
                .collect();
            let body = if parts.is_empty() {
                "loading…".to_string()
            } else {
                parts.join(" · ")
            };
            let stale = if state.active_is_stale() {
                " · stale"
            } else {
                ""
            };
            format!("{email} ({}) — {body}{stale}", provider.display_name())
        }
        None => format!("{email} ({}) — loading…", provider.display_name()),
    }
}

// ---------------------------------------------------------------------------
// Native context menu
// ---------------------------------------------------------------------------

/// Menu item ids (stable strings matched in the event handler).
pub mod ids {
    /// Open the popover panel — the reliable way to reach the UI on Linux, where
    /// the appindicator tray shows its menu on click and never emits a left-click
    /// event for `on_tray_icon_event`.
    pub const SHOW: &str = "show";
    pub const SAVE_CURRENT: &str = "save_current";
    pub const REFRESH_NOW: &str = "refresh_now";
    pub const LAUNCH_AT_LOGIN: &str = "launch_at_login";
    pub const QUIT: &str = "quit";
    pub const UPDATED_INFO: &str = "updated_info";
    /// Prefixes for dynamic items: `remove::<email>`, `login::<provider_id>`,
    /// `style::<key>`, `metric::<key>`.
    pub const REMOVE_PREFIX: &str = "remove::";
    pub const LOGIN_PREFIX: &str = "login::";
    pub const STYLE_PREFIX: &str = "style::";
    pub const METRIC_PREFIX: &str = "metric::";
    /// `tray_account::<key>` (or `tray_account::auto`) — which account the icon shows.
    pub const TRAY_ACCOUNT_PREFIX: &str = "tray_account::";
}

/// A `Send` snapshot of the state the native menu needs. Extracted under the
/// async lock so the actual (non-`Send`) menu construction can run on the main
/// thread without holding `AppState`.
pub struct MenuModel {
    /// (provider, email, is_active) for every saved account, in display order.
    pub accounts: Vec<(Provider, String, bool)>,
    pub prefs: IndicatorPrefs,
    pub last_refresh: Option<chrono::DateTime<chrono::Utc>>,
    /// Global open-popover hotkey, shown as the "Open PitStopX" accelerator.
    pub shortcut: String,
    /// Account key pinned to the tray icon, or `None` for auto.
    pub tray_account: Option<String>,
}

impl MenuModel {
    pub fn from_state(state: &AppState) -> Self {
        MenuModel {
            accounts: state
                .profiles
                .iter()
                .map(|p| {
                    (
                        p.provider,
                        p.email.clone(),
                        state.active_keys.contains(&p.key()),
                    )
                })
                .collect(),
            prefs: state.prefs,
            last_refresh: state.last_refresh,
            shortcut: state.shortcut.clone(),
            tray_account: state.tray_account.clone(),
        }
    }
}

/// Human-readable accelerator for the Linux menu label (no native accelerator
/// rendering there): `CmdOrCtrl` → `Ctrl`, leaving the rest as-is.
#[cfg(target_os = "linux")]
fn humanize_accelerator(accel: &str) -> String {
    accel
        .split('+')
        .map(|p| match p {
            "CmdOrCtrl" | "CommandOrControl" => "Ctrl",
            "Cmd" | "Command" => "Super",
            other => other,
        })
        .collect::<Vec<_>>()
        .join("+")
}

/// Build the native context menu reflecting current state.
pub fn build_menu(app: &AppHandle, model: &MenuModel, launch_at_login: bool) -> Result<Menu<Wry>> {
    let prefs = model.prefs;

    // Remove Account ▸ (non-active accounts).
    let mut remove_sub = SubmenuBuilder::new(app, "Remove Account");
    let removable: Vec<(Provider, &str)> = model
        .accounts
        .iter()
        .filter(|(_, _, is_active)| !is_active)
        .map(|(provider, email, _)| (*provider, email.as_str()))
        .collect();
    if removable.is_empty() {
        remove_sub = remove_sub.item(
            &MenuItemBuilder::new("(none)")
                .id("remove_none")
                .enabled(false)
                .build(app)?,
        );
    } else {
        for (provider, email) in removable {
            // Encode provider + email in the id: `remove::<provider_id>:<email>`.
            remove_sub = remove_sub.item(
                &MenuItemBuilder::new(format!("{email} ({})", provider.display_name()))
                    .id(format!("{}{}:{email}", ids::REMOVE_PREFIX, provider.id()))
                    .build(app)?,
            );
        }
    }
    let remove_sub = remove_sub.build()?;

    // Menu Bar / Tray Display ▸ (style + metric radio-ish groups via checks).
    let style_items = [
        ("Icon + Percent", IndicatorStyle::IconAndPercent),
        ("Icon Only", IndicatorStyle::IconOnly),
        ("Percent Only", IndicatorStyle::PercentOnly),
    ];
    let mut display_sub = SubmenuBuilder::new(app, "Menu Bar / Tray Display");
    for (label, style) in style_items {
        display_sub = display_sub.item(
            &CheckMenuItemBuilder::new(label)
                .id(format!("{}{}", ids::STYLE_PREFIX, style.as_key()))
                .checked(prefs.style == style)
                .build(app)?,
        );
    }
    display_sub = display_sub.separator();
    let metric_items = [
        ("Highest (binding)", crate::prefs::IndicatorMetric::Binding),
        ("5-hour", crate::prefs::IndicatorMetric::FiveHour),
        ("Weekly", crate::prefs::IndicatorMetric::Weekly),
    ];
    for (label, metric) in metric_items {
        display_sub = display_sub.item(
            &CheckMenuItemBuilder::new(label)
                .id(format!("{}{}", ids::METRIC_PREFIX, metric.as_key()))
                .checked(prefs.metric == metric)
                .build(app)?,
        );
    }
    let display_sub = display_sub.build()?;

    // Tray Account ▸ — which account the icon reflects (auto, or a pinned one).
    let mut tray_sub = SubmenuBuilder::new(app, "Tray Account");
    tray_sub = tray_sub.item(
        &CheckMenuItemBuilder::new("Highest usage (auto)")
            .id(format!("{}auto", ids::TRAY_ACCOUNT_PREFIX))
            .checked(model.tray_account.is_none())
            .build(app)?,
    );
    if !model.accounts.is_empty() {
        tray_sub = tray_sub.separator();
        for (provider, email, _active) in &model.accounts {
            let key = crate::source::secret_key(*provider, email);
            tray_sub = tray_sub.item(
                &CheckMenuItemBuilder::new(format!("{email} ({})", provider.display_name()))
                    .id(format!("{}{key}", ids::TRAY_ACCOUNT_PREFIX))
                    .checked(model.tray_account.as_deref() == Some(key.as_str()))
                    .build(app)?,
            );
        }
    }
    let tray_sub = tray_sub.build()?;

    // Log in to new account ▸ (one item per provider).
    let mut login_sub = SubmenuBuilder::new(app, "Log in to New Account");
    for provider in Provider::ALL {
        login_sub = login_sub.item(
            &MenuItemBuilder::new(provider.display_name())
                .id(format!("{}{}", ids::LOGIN_PREFIX, provider.id()))
                .build(app)?,
        );
    }
    let login_sub = login_sub.build()?;

    let updated = model
        .last_refresh
        .map(|t| crate::format::updated(t.with_timezone(&chrono::Local)))
        .unwrap_or_else(|| "never".into());

    // Primary way to open the UI on Linux (tray click only shows this menu).
    // The configured hotkey is shown beside it. On Windows/macOS that's a native
    // right-aligned accelerator; the Linux appindicator (DBusMenu) doesn't render
    // accelerators, so we fold the hotkey into the label text instead.
    #[cfg(target_os = "linux")]
    let open_item = {
        let label = if model.shortcut.is_empty() {
            "Open PitStopX".to_string()
        } else {
            format!("Open PitStopX  ({})", humanize_accelerator(&model.shortcut))
        };
        MenuItemBuilder::new(label).id(ids::SHOW).build(app)?
    };
    #[cfg(not(target_os = "linux"))]
    let open_item = {
        let mut b = MenuItemBuilder::new("Open PitStopX").id(ids::SHOW);
        if !model.shortcut.is_empty() {
            b = b.accelerator(&model.shortcut);
        }
        b.build(app)?
    };

    let menu = MenuBuilder::new(app)
        .item(&open_item)
        .separator()
        .item(&login_sub)
        .item(
            &MenuItemBuilder::new("Save Current Account")
                .id(ids::SAVE_CURRENT)
                .build(app)?,
        )
        .item(&remove_sub)
        .separator()
        .item(
            &MenuItemBuilder::new("Refresh Now")
                .id(ids::REFRESH_NOW)
                .build(app)?,
        )
        .item(&display_sub)
        .item(&tray_sub)
        .separator()
        .item(
            &CheckMenuItemBuilder::new("Launch at Login")
                .id(ids::LAUNCH_AT_LOGIN)
                .checked(launch_at_login)
                .build(app)?,
        )
        .separator()
        .item(
            &MenuItemBuilder::new(format!("Updated {updated} · refreshes every 2 min"))
                .id(ids::UPDATED_INFO)
                .enabled(false)
                .build(app)?,
        )
        .item(
            &MenuItemBuilder::new("Quit PitStopX")
                .id(ids::QUIT)
                .build(app)?,
        )
        .build()?;

    Ok(menu)
}

#[cfg(test)]
mod preview {
    //! Visual smoke test: dumps both tray renderers to PNGs for eyeballing.
    //! `#[ignore]`d so it's opt-in: `cargo test tray_preview -- --ignored`.
    use super::*;

    fn save(img: &Image, path: &str) {
        // Encode via tiny-skia (already a dep) to avoid pulling the `image` crate
        // — its default features drag in the heavy AVIF encoder tree. The render
        // output is premultiplied RGBA, which is exactly what Pixmap expects.
        let size = tiny_skia::IntSize::from_wh(img.width(), img.height()).expect("nonzero size");
        let pm = Pixmap::from_vec(img.rgba().to_vec(), size).expect("dims match buffer");
        std::fs::write(path, pm.encode_png().expect("encode png")).expect("write png");
    }

    #[test]
    #[ignore]
    fn tray_preview() {
        let mid = TrayVisual {
            utilization: Some(0.68),
            five_hour: Some(0.68),
            seven_day: Some(0.41),
            stale: false,
            prefs: IndicatorPrefs::default(),
        };
        save(&render_square(&mid).unwrap(), "/tmp/tray_square.png");
        save(&render_rectangular(&mid).unwrap(), "/tmp/tray_rect.png");

        let warn = TrayVisual {
            utilization: Some(0.93),
            five_hour: Some(0.93),
            seven_day: Some(0.78),
            stale: false,
            prefs: IndicatorPrefs::default(),
        };
        save(
            &render_rectangular(&warn).unwrap(),
            "/tmp/tray_rect_warn.png",
        );
    }
}
