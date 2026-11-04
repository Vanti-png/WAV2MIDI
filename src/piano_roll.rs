//! Dalga formu + zaman cetveli + piyano rulosu bileşeni.

use eframe::egui::{self, pos2, vec2, Align2, Color32, CursorIcon, FontId, Rect, Sense, Stroke};
use wav2midi::notes::{is_black_key, note_name, Note};

const KEY_W: f32 = 54.0;
const WAVE_H: f32 = 64.0;
const RULER_H: f32 = 22.0;
const MAX_PX_PER_SEC: f32 = 3000.0;

pub struct RollView {
    pub px_per_sec: f32,
    /// Sol kenardaki zaman (sn)
    pub t0: f64,
    pub row_h: f32,
    /// Üst kenardaki perde (kesirli)
    pub top_pitch: f32,
    /// Çalma başlangıç imleci (sn)
    pub cursor: f64,
    /// Bir sonraki çizimde tüm dosyayı ekrana sığdır
    pub fit_pending: bool,
}

impl Default for RollView {
    fn default() -> Self {
        Self {
            px_per_sec: 100.0,
            t0: 0.0,
            row_h: 12.0,
            top_pitch: 90.0,
            cursor: 0.0,
            fit_pending: true,
        }
    }
}

/// Dalga formu çizimi için önceden hesaplanmış min/max kovaları.
pub struct Overview {
    buckets_per_sec: f64,
    minmax: Vec<(f32, f32)>,
    peak: f32,
}

impl Overview {
    pub fn new(samples: &[f32], sr: u32) -> Self {
        let buckets_per_sec = 400.0;
        let size = ((sr as f64 / buckets_per_sec).round() as usize).max(1);
        let minmax: Vec<(f32, f32)> = samples
            .chunks(size)
            .map(|c| {
                c.iter()
                    .fold((f32::MAX, f32::MIN), |(lo, hi), &v| (lo.min(v), hi.max(v)))
            })
            .collect();
        let peak = minmax
            .iter()
            .fold(0.0f32, |m, &(lo, hi)| m.max(lo.abs()).max(hi.abs()))
            .max(1e-6);
        Self {
            buckets_per_sec: sr as f64 / size as f64,
            minmax,
            peak,
        }
    }
}

pub struct RollResponse {
    /// Kullanıcı tıklayarak yeni konum seçti
    pub seek: Option<f64>,
}

fn note_color(velocity: u8, hovered: bool) -> Color32 {
    let t = (velocity as f32 / 127.0).clamp(0.0, 1.0);
    let a = [72.0, 132.0, 255.0];
    let b = [255.0, 118.0, 84.0];
    let mut c = [0u8; 3];
    for i in 0..3 {
        let v = a[i] + (b[i] - a[i]) * t;
        c[i] = if hovered { (v + 50.0).min(255.0) } else { v } as u8;
    }
    Color32::from_rgb(c[0], c[1], c[2])
}

pub fn format_time(t: f64, precise: bool) -> String {
    let t = t.max(0.0);
    let m = (t / 60.0).floor() as u64;
    let s = t - m as f64 * 60.0;
    if precise {
        format!("{m}:{s:04.1}")
    } else {
        format!("{m}:{:02}", s.floor() as u64)
    }
}

pub fn piano_roll(
    ui: &mut egui::Ui,
    view: &mut RollView,
    notes: &[Note],
    overview: &Overview,
    duration: f64,
    playhead: Option<f64>,
) -> RollResponse {
    let size = ui.available_size().max(vec2(200.0, 200.0));
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());

    let wave_rect = Rect::from_min_size(rect.min + vec2(KEY_W, 0.0), vec2(rect.width() - KEY_W, WAVE_H));
    let ruler_rect = Rect::from_min_size(wave_rect.left_bottom(), vec2(wave_rect.width(), RULER_H));
    let roll_rect = Rect::from_min_max(ruler_rect.left_bottom(), rect.max);
    let keys_rect = Rect::from_min_max(pos2(rect.left(), roll_rect.top()), pos2(roll_rect.left(), rect.bottom()));

    let duration = duration.max(0.1);
    let roll_w = roll_rect.width().max(1.0);
    let min_px = (roll_w / duration as f32 * 0.8).min(50.0);

    // --- Sığdırma
    if view.fit_pending {
        view.px_per_sec = (roll_w / duration as f32).clamp(min_px, MAX_PX_PER_SEC);
        view.t0 = 0.0;
        let rows = roll_rect.height() / view.row_h;
        if let (Some(lo), Some(hi)) = (
            notes.iter().map(|n| n.pitch).min(),
            notes.iter().map(|n| n.pitch).max(),
        ) {
            let span = (hi - lo) as f32 + 1.0;
            view.top_pitch = if span < rows {
                hi as f32 + 1.0 + (rows - span) / 2.0
            } else {
                hi as f32 + 2.0
            };
        } else {
            view.top_pitch = 72.0 + rows / 2.0;
        }
        view.fit_pending = false;
    }

    // --- Girdi
    if resp.hovered() {
        let (scroll, zoom, hover) =
            ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta(), i.pointer.hover_pos()));
        if zoom != 1.0 {
            if let Some(h) = hover {
                let anchor = view.t0 + ((h.x - roll_rect.left()) / view.px_per_sec) as f64;
                view.px_per_sec = (view.px_per_sec * zoom).clamp(min_px, MAX_PX_PER_SEC);
                view.t0 = anchor - ((h.x - roll_rect.left()) / view.px_per_sec) as f64;
            }
        } else if scroll != egui::Vec2::ZERO {
            view.t0 -= (scroll.x / view.px_per_sec) as f64;
            view.top_pitch += scroll.y / view.row_h;
        }
    }
    if resp.dragged() {
        let d = resp.drag_delta();
        view.t0 -= (d.x / view.px_per_sec) as f64;
        view.top_pitch += d.y / view.row_h;
        ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
    }
    let mut seek = None;
    if resp.clicked() {
        if let Some(p) = resp.interact_pointer_pos() {
            if p.x >= roll_rect.left() {
                let t = (view.t0 + ((p.x - roll_rect.left()) / view.px_per_sec) as f64).clamp(0.0, duration);
                view.cursor = t;
                seek = Some(t);
            }
        }
    }

    let visible_secs = (roll_w / view.px_per_sec) as f64;
    // Çalarken imleci takip et
    if let Some(ph) = playhead {
        if !resp.dragged() && (ph > view.t0 + visible_secs * 0.92 || ph < view.t0) {
            view.t0 = ph - visible_secs * 0.08;
        }
    }
    let max_t0 = (duration - visible_secs * 0.9).max(0.0);
    view.t0 = view.t0.clamp(0.0, max_t0);
    let rows = roll_rect.height() / view.row_h;
    view.top_pitch = view.top_pitch.clamp(rows.min(128.0), 128.0);

    let px = view.px_per_sec;
    let t0 = view.t0;
    let t1 = t0 + visible_secs;
    let x_of = |t: f64| roll_rect.left() + ((t - t0) as f32) * px;
    let y_of = |p: f32| roll_rect.top() + (view.top_pitch - p - 1.0) * view.row_h;

    let painter = ui.painter_at(rect);
    let visuals = ui.visuals();
    let bg = Color32::from_rgb(20, 22, 28);
    painter.rect_filled(rect, 0.0, bg);

    // --- Satırlar
    let roll_p = painter.with_clip_rect(roll_rect);
    let p_top = view.top_pitch.ceil() as i32;
    let p_bot = (view.top_pitch - rows).floor().max(0.0) as i32;
    for p in p_bot..=p_top.min(127) {
        let y = y_of(p as f32);
        let r = Rect::from_min_size(pos2(roll_rect.left(), y), vec2(roll_w, view.row_h));
        let fill = if is_black_key(p as u8) {
            Color32::from_rgb(24, 26, 33)
        } else {
            Color32::from_rgb(31, 34, 42)
        };
        roll_p.rect_filled(r, 0.0, fill);
        if p % 12 == 0 {
            roll_p.line_segment(
                [pos2(roll_rect.left(), r.bottom()), pos2(roll_rect.right(), r.bottom())],
                Stroke::new(1.0_f32, Color32::from_rgb(58, 63, 80)),
            );
        }
    }

    // --- Zaman ızgarası
    let steps = [0.05, 0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0];
    let step = steps
        .iter()
        .copied()
        .find(|s| (*s as f32) * px >= 80.0)
        .unwrap_or(600.0);
    let grid_col = Color32::from_rgba_unmultiplied(255, 255, 255, 16);
    let label_col = visuals.weak_text_color();
    let mut gt = (t0 / step).floor() * step;
    painter.rect_filled(ruler_rect, 0.0, Color32::from_rgb(28, 30, 38));
    while gt <= t1 + step {
        let x = x_of(gt);
        if x >= roll_rect.left() - 1.0 && x <= roll_rect.right() {
            roll_p.line_segment([pos2(x, roll_rect.top()), pos2(x, roll_rect.bottom())], Stroke::new(1.0_f32, grid_col));
            painter.with_clip_rect(ruler_rect).line_segment(
                [pos2(x, ruler_rect.bottom() - 6.0), pos2(x, ruler_rect.bottom())],
                Stroke::new(1.0_f32, label_col),
            );
            painter.with_clip_rect(ruler_rect).text(
                pos2(x + 3.0, ruler_rect.center().y - 2.0),
                Align2::LEFT_CENTER,
                format_time(gt, step < 1.0),
                FontId::monospace(11.0),
                label_col,
            );
        }
        gt += step;
    }

    // --- Dalga formu
    let wave_p = painter.with_clip_rect(wave_rect);
    wave_p.rect_filled(wave_rect, 0.0, Color32::from_rgb(17, 19, 24));
    let mid = wave_rect.center().y;
    let half = WAVE_H * 0.45;
    let wave_col = Color32::from_rgb(96, 140, 230);
    let n_b = overview.minmax.len();
    if n_b > 0 {
        let mut x = wave_rect.left();
        while x < wave_rect.right() {
            let ta = t0 + ((x - roll_rect.left()) / px) as f64;
            let tb = ta + (1.0 / px) as f64;
            let ia = (ta * overview.buckets_per_sec).floor().max(0.0) as usize;
            let ib = ((tb * overview.buckets_per_sec).ceil() as usize).max(ia + 1).min(n_b);
            if ia < n_b && ia < ib {
                let (lo, hi) = overview.minmax[ia..ib]
                    .iter()
                    .fold((f32::MAX, f32::MIN), |(l, h), &(a, b)| (l.min(a), h.max(b)));
                let y0 = mid - hi / overview.peak * half;
                let y1 = mid - lo / overview.peak * half;
                wave_p.line_segment([pos2(x, y0), pos2(x, y1.max(y0 + 1.0))], Stroke::new(1.0_f32, wave_col));
            }
            x += 1.0;
        }
    }

    // --- Notalar
    let hover_pos = resp.hover_pos().filter(|p| roll_rect.contains(*p));
    let mut hovered: Option<Note> = None;
    for n in notes {
        if n.end < t0 || n.start > t1 {
            continue;
        }
        let pf = n.pitch as f32;
        if pf > view.top_pitch || pf + 1.0 < view.top_pitch - rows {
            continue;
        }
        let x0 = x_of(n.start);
        let x1 = x_of(n.end).max(x0 + 2.0);
        let y = y_of(pf);
        let r = Rect::from_min_max(pos2(x0, y + 1.0), pos2(x1, y + view.row_h - 1.0));
        let is_hover = hover_pos.is_some_and(|p| r.contains(p));
        if is_hover {
            hovered = Some(*n);
        }
        roll_p.rect_filled(r, 2.0, note_color(n.velocity, is_hover));
        if r.width() > 5.0 {
            roll_p.rect_stroke(
                r,
                2.0,
                Stroke::new(1.0_f32, Color32::from_black_alpha(if is_hover { 0 } else { 90 })),
                egui::StrokeKind::Inside,
            );
        }
    }

    if notes.is_empty() {
        roll_p.text(
            roll_rect.center(),
            Align2::CENTER_CENTER,
            "Henüz nota yok — soldaki “Dönüştür” düğmesine basın",
            FontId::proportional(15.0),
            label_col,
        );
    }

    // --- Klavye
    let keys_p = painter.with_clip_rect(keys_rect);
    for p in p_bot..=p_top.min(127) {
        let y = y_of(p as f32);
        let r = Rect::from_min_size(pos2(keys_rect.left(), y), vec2(KEY_W, view.row_h));
        let black = is_black_key(p as u8);
        let fill = if black {
            Color32::from_rgb(30, 31, 36)
        } else {
            Color32::from_rgb(214, 216, 222)
        };
        let kr = if black { Rect::from_min_size(r.min, vec2(KEY_W * 0.62, r.height())) } else { r };
        if black {
            keys_p.rect_filled(r, 0.0, Color32::from_rgb(214, 216, 222));
        }
        keys_p.rect_filled(kr.shrink2(vec2(0.0, 0.5)), 1.0, fill);
        if p % 12 == 0 && view.row_h >= 8.0 {
            keys_p.text(
                pos2(r.right() - 4.0, r.center().y),
                Align2::RIGHT_CENTER,
                note_name(p as u8),
                FontId::proportional((view.row_h - 1.0).min(12.0)),
                Color32::from_rgb(40, 42, 50),
            );
        }
    }
    painter.line_segment(
        [pos2(keys_rect.right(), wave_rect.top()), pos2(keys_rect.right(), rect.bottom())],
        Stroke::new(1.0_f32, Color32::from_rgb(60, 64, 78)),
    );
    // Sol üst köşe
    let corner = Rect::from_min_max(rect.min, pos2(keys_rect.right(), roll_rect.top()));
    painter.rect_filled(corner, 0.0, Color32::from_rgb(28, 30, 38));
    painter.text(
        corner.center(),
        Align2::CENTER_CENTER,
        "🎹",
        FontId::proportional(22.0),
        label_col,
    );

    // --- İmleç ve çalma konumu
    let line_p = painter.with_clip_rect(Rect::from_min_max(wave_rect.min, rect.max));
    let cx = x_of(view.cursor);
    line_p.line_segment(
        [pos2(cx, wave_rect.top()), pos2(cx, rect.bottom())],
        Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(255, 255, 255, 90)),
    );
    if let Some(ph) = playhead {
        let x = x_of(ph);
        line_p.line_segment(
            [pos2(x, wave_rect.top()), pos2(x, rect.bottom())],
            Stroke::new(2.0_f32, Color32::from_rgb(255, 210, 90)),
        );
    }

    if resp.hovered() && !resp.dragged() {
        ui.ctx().set_cursor_icon(if hovered.is_some() { CursorIcon::PointingHand } else { CursorIcon::Grab });
    }
    if let Some(n) = hovered {
        resp.on_hover_ui_at_pointer(|ui| {
            ui.label(egui::RichText::new(format!("{}  (MIDI {})", note_name(n.pitch), n.pitch)).strong());
            ui.label(format!(
                "{} – {}  ({:.0} ms)",
                format_time(n.start, true),
                format_time(n.end, true),
                n.duration() * 1000.0
            ));
            ui.label(format!("Velocity: {}", n.velocity));
        });
    }

    RollResponse { seek }
}
