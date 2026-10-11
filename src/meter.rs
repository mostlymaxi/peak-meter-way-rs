//! Peak detection, meter ballistics and rendering. No PipeWire in here, so it is unit-testable.

use crate::config::{Config, Style};
use std::fmt::Write as _;
use std::time::{Duration, Instant};

/// Largest absolute sample value in a buffer of native-endian `f32` samples, returned
/// as the raw bits of a non-negative float.
///
/// Clearing the sign bit gives `|x|`, and non-negative IEEE-754 floats sort the same
/// as their bit patterns, so the whole thing is an integer max that LLVM turns into
/// packed `pmaxud`. Channels are interleaved and we want the peak over all of them,
/// so we never need to know the channel count.
#[inline]
pub fn peak_bits(bytes: &[u8]) -> u32 {
    // SAFETY: every bit pattern is a valid u32.
    let (head, body, tail) = unsafe { bytes.align_to::<u32>() };
    debug_assert!(head.is_empty() && tail.is_empty());

    const LANES: usize = 16;
    let mut acc = [0u32; LANES];
    let (chunks, rest) = body.as_chunks::<LANES>();
    for chunk in chunks {
        for (a, &s) in acc.iter_mut().zip(chunk) {
            *a = (*a).max(s & 0x7fff_ffff);
        }
    }
    let mut max = acc.into_iter().max().unwrap_or(0);
    for &s in rest {
        max = max.max(s & 0x7fff_ffff);
    }
    // NaN sorts above +inf; treat a corrupt buffer as silence rather than a full-scale hit.
    if max > f32::INFINITY.to_bits() {
        0
    } else {
        max
    }
}

#[inline]
pub fn lin_to_db(lin: f32) -> f32 {
    if lin > 0.0 {
        20.0 * lin.log10()
    } else {
        f32::NEG_INFINITY
    }
}

pub fn db_to_lin(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// Bar + peak-hold state with time-based (not callback-rate-based) ballistics.
pub struct Ballistics {
    floor: f32,
    release: f32,
    hold_time: Duration,
    hold_release: f32,
    pub level: f32,
    pub hold: f32,
    hold_since: Instant,
}

impl Ballistics {
    pub fn new(cfg: &Config, now: Instant) -> Self {
        Self {
            floor: cfg.floor_db,
            release: cfg.release_db_s,
            hold_time: cfg.hold,
            hold_release: cfg.hold_release_db_s,
            level: cfg.floor_db,
            hold: cfg.floor_db,
            hold_since: now,
        }
    }

    /// Feed the peak (in dBFS) seen since the last step and advance by `dt`.
    pub fn step(&mut self, peak_db: f32, dt: Duration, now: Instant) {
        let dt = dt.as_secs_f32();
        let peak = peak_db.max(self.floor);
        // Instant attack, linear-in-dB release.
        self.level = peak.max(self.level - self.release * dt).max(self.floor);

        if self.level >= self.hold {
            self.hold = self.level;
            self.hold_since = now;
        } else if now.duration_since(self.hold_since) >= self.hold_time {
            self.hold = (self.hold - self.hold_release * dt).max(self.level);
        }
    }

    /// Everything has fallen back to the floor; nothing left to animate.
    pub fn is_idle(&self) -> bool {
        self.level <= self.floor && self.hold <= self.floor
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Zone {
    Low,
    Warn,
    Hot,
}

struct Glyphs {
    full: char,
    /// Partially lit cell, index `k-1` for `k` lit sub-steps out of `partials.len() + 1`.
    partials: &'static [char],
    track: char,
    tick: char,
}

impl Style {
    fn glyphs(self) -> Glyphs {
        match self {
            Style::Line => Glyphs {
                full: '━',
                partials: &['╸'],
                track: '━',
                tick: '┃',
            },
            Style::Blocks => Glyphs {
                full: '█',
                partials: &['▏', '▎', '▍', '▌', '▋', '▊', '▉'],
                track: '█',
                tick: '▊',
            },
            Style::Dots => Glyphs {
                full: '●',
                partials: &[],
                track: '●',
                tick: '●',
            },
        }
    }
}

/// Turns a meter state into one waybar JSON line. Reuses its buffers, so a frame costs
/// no allocations once warmed up.
pub struct Renderer {
    glyphs: Glyphs,
    width: usize,
    floor: f32,
    zones: Vec<Zone>,
    lit_open: [String; 3],
    track_open: [String; 3],
    hold_enabled: bool,
    hide_idle: bool,
    warn_db: f32,
    hot_db: f32,
    cells: Vec<(Zone, bool, char)>,
    line: String,
}

impl Renderer {
    pub fn new(cfg: &Config) -> Self {
        let range = -cfg.floor_db;
        // A cell takes the colour of the zone its upper edge falls in.
        let zones = (0..cfg.width)
            .map(|i| {
                let top = cfg.floor_db + range * (i + 1) as f32 / cfg.width as f32;
                if top > cfg.hot_db {
                    Zone::Hot
                } else if top > cfg.warn_db {
                    Zone::Warn
                } else {
                    Zone::Low
                }
            })
            .collect();
        let colors = [&cfg.color_low, &cfg.color_warn, &cfg.color_hot];
        Self {
            glyphs: cfg.style.glyphs(),
            width: cfg.width,
            floor: cfg.floor_db,
            zones,
            lit_open: colors.map(|c| format!("<span foreground='{c}'>")),
            track_open: colors.map(|c| {
                format!(
                    "<span foreground='{c}' fgalpha='{}%'>",
                    cfg.track_alpha.max(1)
                )
            }),
            hold_enabled: !cfg.hold.is_zero(),
            hide_idle: cfg.hide_idle,
            warn_db: cfg.warn_db,
            hot_db: cfg.hot_db,
            cells: Vec::with_capacity(cfg.width),
            line: String::with_capacity(cfg.width * 64 + 128),
        }
    }

    /// Render and return the JSON line (with trailing newline).
    pub fn render(&mut self, m: &Ballistics) -> &str {
        let steps = self.glyphs.partials.len() + 1;
        let frac = |db: f32| ((db - self.floor) / -self.floor).clamp(0.0, 1.0);

        let lit_steps = (frac(m.level) * (self.width * steps) as f32) as usize;
        let full = lit_steps / steps;
        let part = lit_steps % steps;
        let first_dark = full + usize::from(part > 0);
        let tick = (self.hold_enabled && m.hold > self.floor)
            .then(|| ((frac(m.hold) * self.width as f32) as usize).min(self.width - 1))
            .filter(|&t| t >= first_dark);

        self.cells.clear();
        for (i, &zone) in self.zones.iter().enumerate() {
            let cell = if i < full {
                (zone, true, self.glyphs.full)
            } else if i == full && part > 0 {
                (zone, true, self.glyphs.partials[part - 1])
            } else if Some(i) == tick {
                (zone, true, self.glyphs.tick)
            } else {
                (zone, false, self.glyphs.track)
            };
            self.cells.push(cell);
        }

        let idle = m.is_idle();
        let class = if idle {
            "silent"
        } else if m.level > self.hot_db {
            "peak"
        } else if m.level > self.warn_db {
            "warn"
        } else {
            "safe"
        };

        self.line.clear();
        self.line.push_str("{\"text\":\"");
        if !(idle && self.hide_idle) {
            // Merge runs of identically styled cells into one span to keep the markup small.
            let mut run: Option<(Zone, bool)> = None;
            for &(zone, lit, ch) in &self.cells {
                if run != Some((zone, lit)) {
                    if run.is_some() {
                        self.line.push_str("</span>");
                    }
                    let open = if lit {
                        &self.lit_open
                    } else {
                        &self.track_open
                    };
                    self.line.push_str(&open[zone as usize]);
                    run = Some((zone, lit));
                }
                self.line.push(ch);
            }
            self.line.push_str("</span>");
        }
        let pct = (frac(m.level) * 100.0).round() as u32;
        let _ = write!(
            self.line,
            "\",\"class\":\"{class}\",\"percentage\":{pct},\"tooltip\":\""
        );
        if m.hold > self.floor {
            let _ = write!(self.line, "peak {:.1} dBFS", m.hold);
        } else {
            self.line.push_str("silent");
        }
        self.line.push_str("\"}\n");
        &self.line
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(s: &[f32]) -> Vec<u8> {
        s.iter().flat_map(|f| f.to_ne_bytes()).collect()
    }

    #[test]
    fn peak_is_abs_max_over_all_channels() {
        let mut v = vec![0.01f32; 1000];
        v[517] = -0.75; // negative, second channel, not in a full 16-lane chunk boundary
        v[3] = 0.5;
        let b = bytes(&v);
        assert_eq!(f32::from_bits(peak_bits(&b)), 0.75);
        // tail handling
        let b = bytes(&[0.0, 0.0, -0.9]);
        assert_eq!(f32::from_bits(peak_bits(&b)), 0.9);
        assert_eq!(peak_bits(&[]), 0);
        assert_eq!(peak_bits(&bytes(&[f32::NAN, 0.1])), 0);
    }

    #[test]
    fn ballistics_attack_release_hold() {
        let cfg = Config::default();
        let t0 = Instant::now();
        let mut b = Ballistics::new(&cfg, t0);
        assert!(b.is_idle());
        b.step(-6.0, Duration::from_millis(33), t0);
        assert_eq!(b.level, -6.0);
        assert_eq!(b.hold, -6.0);
        // half a second of silence: falls at 24 dB/s, hold stays put
        let t1 = t0 + Duration::from_millis(500);
        b.step(f32::NEG_INFINITY, Duration::from_millis(500), t1);
        assert!((b.level + 18.0).abs() < 1e-3);
        assert_eq!(b.hold, -6.0);
        // long after: everything back at the floor
        for i in 1..200 {
            b.step(
                f32::NEG_INFINITY,
                Duration::from_millis(33),
                t1 + Duration::from_millis(33 * i),
            );
        }
        assert!(b.is_idle());
    }

    fn text(line: &str) -> String {
        // strip markup to get the visible glyphs
        let t = &line[9..line.find("\",\"class").unwrap()];
        let mut out = String::new();
        let mut tag = false;
        for c in t.chars() {
            match c {
                '<' => tag = true,
                '>' => tag = false,
                c if !tag => out.push(c),
                _ => {}
            }
        }
        out
    }

    #[test]
    fn render_is_fixed_width_and_valid() {
        for style in [Style::Line, Style::Blocks, Style::Dots] {
            let cfg = Config {
                style,
                ..Config::default()
            };
            let mut r = Renderer::new(&cfg);
            let t0 = Instant::now();
            let mut b = Ballistics::new(&cfg, t0);
            for db in [-100.0, -59.0, -45.3, -30.0, -17.9, -6.1, -0.5, 0.0, 6.0] {
                b.level = db_clamp(db, &cfg);
                b.hold = (b.level + 10.0).min(0.0);
                let line = r.render(&b).to_string();
                assert!(line.ends_with("}\n"));
                assert_eq!(
                    text(&line).chars().count(),
                    cfg.width,
                    "{style:?} {db} {line}"
                );
                assert_eq!(
                    line.matches("<span").count(),
                    line.matches("</span>").count()
                );
            }
        }
    }

    fn db_clamp(db: f32, cfg: &Config) -> f32 {
        db.clamp(cfg.floor_db, 0.0)
    }

    #[test]
    fn render_positions() {
        let cfg = Config {
            width: 10,
            hold: Duration::ZERO,
            ..Config::default()
        };
        let mut r = Renderer::new(&cfg);
        let mut b = Ballistics::new(&cfg, Instant::now());
        assert_eq!(text(r.render(&b)), "━━━━━━━━━━");
        assert!(r.render(&b).contains("\"class\":\"silent\""));
        b.level = -30.0; // halfway on a -60..0 scale
        let line = r.render(&b).to_string();
        assert!(line.contains("\"percentage\":50"));
        // 5 lit cells then 5 track cells, all low/warn zone
        let lit = &r.lit_open[0];
        assert!(line.contains(&format!("{lit}━━━━━</span>")), "{line}");
        b.level = -27.0; // 5.5 cells
        assert_eq!(text(r.render(&b)), "━━━━━╸━━━━");
        b.level = 0.0;
        assert!(r.render(&b).contains("\"class\":\"peak\""));
    }

    #[test]
    fn hide_idle() {
        let cfg = Config {
            hide_idle: true,
            ..Config::default()
        };
        let mut r = Renderer::new(&cfg);
        let b = Ballistics::new(&cfg, Instant::now());
        assert!(r.render(&b).starts_with("{\"text\":\"\","));
    }

    #[test]
    fn db_roundtrip() {
        assert!((lin_to_db(db_to_lin(-42.0)) + 42.0).abs() < 1e-3);
        assert_eq!(lin_to_db(0.0), f32::NEG_INFINITY);
    }
}
