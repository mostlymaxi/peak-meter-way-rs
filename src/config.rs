//! Command line options. Hand-rolled to keep the binary small and the startup instant.

use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    /// Thin continuous line `━━━━╸` with a `┃` peak-hold tick.
    Line,
    /// Chunky full-height blocks with 1/8th cell resolution.
    Blocks,
    /// LED-style dots.
    Dots,
}

#[derive(Clone, Debug)]
pub struct Config {
    /// Capture the default sink's monitor (what you hear) instead of the default source (mic).
    pub monitor: bool,
    /// Optional `target.object` (node name or serial) to capture from.
    pub target: Option<String>,
    pub style: Style,
    /// Number of glyph cells in the meter.
    pub width: usize,
    /// Frame period while the meter is moving.
    pub frame: Duration,
    /// Bottom of the scale in dBFS.
    pub floor_db: f32,
    /// Cells above this level are drawn in the warn colour.
    pub warn_db: f32,
    /// Cells above this level are drawn in the hot colour.
    pub hot_db: f32,
    /// Fall rate of the bar in dB per second (attack is instant).
    pub release_db_s: f32,
    /// How long the peak-hold tick stays put.
    pub hold: Duration,
    /// Fall rate of the peak-hold tick after the hold time, in dB per second.
    pub hold_release_db_s: f32,
    pub color_low: String,
    pub color_warn: String,
    pub color_hot: String,
    /// Alpha of the unlit track, `0`–`100`.
    pub track_alpha: u8,
    /// Print an empty text when silent so waybar hides the module.
    pub hide_idle: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            monitor: false,
            target: None,
            style: Style::Line,
            width: 20,
            frame: Duration::from_micros(1_000_000 / 30),
            floor_db: -60.0,
            warn_db: -18.0,
            hot_db: -6.0,
            release_db_s: 24.0,
            hold: Duration::from_millis(1200),
            hold_release_db_s: 30.0,
            color_low: "#a6e3a1".into(),
            color_warn: "#f9e2af".into(),
            color_hot: "#f38ba8".into(),
            track_alpha: 22,
            hide_idle: false,
        }
    }
}

pub const USAGE: &str = "\
peak-meter-way: a PipeWire peak meter for waybar custom modules

USAGE: peak-meter-way [OPTIONS] [TARGET]

Prints one JSON line per visual change (return-type = \"json\", escape = false).

OPTIONS:
  -m, --monitor            meter the default output (sink monitor) instead of the default input
  -t, --target NAME        capture from a specific node (name or serial); also positional
  -s, --style STYLE        line | blocks | dots                       [default: line]
  -w, --width N            number of cells                            [default: 20]
  -f, --fps N              max frames per second while moving         [default: 30]
      --floor DB           bottom of the scale in dBFS                [default: -60]
      --warn DB            start of the warn zone in dBFS             [default: -18]
      --hot DB             start of the hot zone in dBFS              [default: -6]
      --release DB/S       bar fall rate                              [default: 24]
      --hold MS            peak-hold time in ms, 0 disables the tick  [default: 1200]
      --hold-release DB/S  peak-hold fall rate                        [default: 30]
      --color-low HEX      colour of the normal zone                  [default: #a6e3a1]
      --color-warn HEX     colour of the warn zone                    [default: #f9e2af]
      --color-hot HEX      colour of the hot zone                     [default: #f38ba8]
      --track-alpha PCT    opacity of the unlit track, 0-100          [default: 22]
      --hide-idle          print empty text when silent (waybar hides the module)
  -h, --help               show this help
";

impl Config {
    pub fn from_args(mut args: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut c = Config::default();
        while let Some(arg) = args.next() {
            let (flag, inline) = match arg.split_once('=') {
                Some((f, v)) if f.starts_with("--") => (f.to_string(), Some(v.to_string())),
                _ => (arg.clone(), None),
            };
            let mut val = |name: &str| -> Result<String, String> {
                inline
                    .clone()
                    .or_else(|| args.next())
                    .ok_or_else(|| format!("{name} needs a value"))
            };
            match flag.as_str() {
                "-h" | "--help" => return Err(String::new()),
                "-m" | "--monitor" => c.monitor = true,
                "--hide-idle" => c.hide_idle = true,
                "-t" | "--target" => c.target = Some(val(&flag)?),
                "-s" | "--style" => {
                    c.style = match val(&flag)?.as_str() {
                        "line" => Style::Line,
                        "blocks" => Style::Blocks,
                        "dots" => Style::Dots,
                        s => return Err(format!("unknown style {s:?}")),
                    }
                }
                "-w" | "--width" => c.width = num(&flag, &val(&flag)?)?,
                "-f" | "--fps" => {
                    let fps: u32 = num(&flag, &val(&flag)?)?;
                    if fps == 0 || fps > 240 {
                        return Err("--fps must be between 1 and 240".into());
                    }
                    c.frame = Duration::from_micros(1_000_000 / u64::from(fps));
                }
                "--floor" => c.floor_db = num(&flag, &val(&flag)?)?,
                "--warn" => c.warn_db = num(&flag, &val(&flag)?)?,
                "--hot" => c.hot_db = num(&flag, &val(&flag)?)?,
                "--release" => c.release_db_s = num(&flag, &val(&flag)?)?,
                "--hold" => c.hold = Duration::from_millis(num(&flag, &val(&flag)?)?),
                "--hold-release" => c.hold_release_db_s = num(&flag, &val(&flag)?)?,
                "--color-low" => c.color_low = color(&val(&flag)?)?,
                "--color-warn" => c.color_warn = color(&val(&flag)?)?,
                "--color-hot" => c.color_hot = color(&val(&flag)?)?,
                "--track-alpha" => c.track_alpha = num::<u8>(&flag, &val(&flag)?)?.min(100),
                s if !s.starts_with('-') && c.target.is_none() => c.target = Some(arg),
                s => return Err(format!("unknown option {s:?}")),
            }
        }
        if c.width == 0 || c.width > 200 {
            return Err("--width must be between 1 and 200".into());
        }
        if c.floor_db.is_nan() || c.floor_db >= 0.0 {
            return Err("--floor must be negative".into());
        }
        if !(c.release_db_s > 0.0 && c.hold_release_db_s > 0.0) {
            return Err("release rates must be positive".into());
        }
        Ok(c)
    }
}

fn num<T: std::str::FromStr>(flag: &str, s: &str) -> Result<T, String> {
    s.parse()
        .map_err(|_| format!("{flag}: invalid number {s:?}"))
}

/// Accept `#rgb`, `#rrggbb` or `#rrggbbaa`. Only hex is allowed so the value can
/// be dropped into Pango markup and JSON without escaping.
fn color(s: &str) -> Result<String, String> {
    let hex = s.strip_prefix('#').unwrap_or(s);
    if matches!(hex.len(), 3 | 6 | 8) && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(format!("#{hex}"))
    } else {
        Err(format!("invalid colour {s:?}, expected #rrggbb"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Result<Config, String> {
        Config::from_args(s.split_whitespace().map(String::from))
    }

    #[test]
    fn parses_flags() {
        let c = parse("-m --style=blocks -w 12 --fps 60 --color-hot f00 alsa_output.foo").unwrap();
        assert!(c.monitor);
        assert_eq!(c.style, Style::Blocks);
        assert_eq!(c.width, 12);
        assert_eq!(c.frame, Duration::from_micros(16_666));
        assert_eq!(c.color_hot, "#f00");
        assert_eq!(c.target.as_deref(), Some("alsa_output.foo"));
    }

    #[test]
    fn rejects_bad_input() {
        assert!(parse("--color-low red").is_err());
        assert!(parse("--color-low #12'>").is_err());
        assert!(parse("--width 0").is_err());
        assert!(parse("--fps 0").is_err());
        assert!(parse("--floor 3").is_err());
        assert!(parse("--bogus").is_err());
        assert!(parse("--width").is_err());
    }
}
