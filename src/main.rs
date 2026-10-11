//! peak-meter-way: a PipeWire peak meter for waybar.
//!
//! Design, with CPU use in mind:
//! * The PipeWire `process` callback runs on PipeWire's own realtime data thread, which
//!   is woken once per graph cycle anyway. It only does a vectorised abs-max over the
//!   buffer and folds it into an atomic: no formatting, no I/O, no syscalls, and no hop
//!   to the main thread per cycle.
//! * A main-loop timer renders frames at a capped rate (30 fps by default), and a line
//!   is written only when the output differs from the previous one.
//! * Once the meter has fallen back to the floor the timer is disarmed, so silence costs
//!   zero main-thread wakeups. The first buffer above the floor pokes the main loop
//!   through an eventfd, once, to re-arm it.
//! * In `--monitor` mode the stream is passive, so the meter never keeps the sink awake.

mod config;
mod meter;

use config::{Config, USAGE};
use meter::{lin_to_db, peak_bits, Ballistics, Renderer};
use pipewire as pw;
use pw::loop_::{Signal, TimerSource};
use pw::{properties::properties, spa};
use std::cell::{Cell, OnceCell, RefCell};
use std::io::Write;
use std::process::ExitCode;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Lock-free state shared between the realtime process callback and the main loop.
struct Shared {
    /// Max |sample| since the last frame, as f32 bits (non-negative floats order like u32).
    peak: AtomicU32,
    /// Peak bits above this light up the meter and should wake the renderer.
    wake_threshold: u32,
    /// The frame timer is armed, or a wake-up for it is already in flight.
    awake: AtomicBool,
}

impl Shared {
    /// Realtime thread: record a buffer's peak; returns true if the main loop must be woken.
    #[inline]
    fn record(&self, bits: u32) -> bool {
        self.peak.fetch_max(bits, Ordering::SeqCst);
        bits > self.wake_threshold && !self.awake.swap(true, Ordering::SeqCst)
    }

    /// Main thread: the meter went idle. Returns true if it may really sleep, false if
    /// a loud buffer raced in meanwhile and the timer has to keep going.
    fn try_sleep(&self) -> bool {
        self.awake.store(false, Ordering::SeqCst);
        !(self.peak.load(Ordering::SeqCst) > self.wake_threshold
            && !self.awake.swap(true, Ordering::SeqCst))
    }
}

struct Output {
    meter: Ballistics,
    renderer: Renderer,
    last_frame: Instant,
    last_line: String,
    stdout: std::io::Stdout,
}

impl Output {
    /// Write the line if it changed. Returns false when stdout is gone (waybar exited).
    fn emit(&mut self) -> bool {
        let line = self.renderer.render(&self.meter);
        if line == self.last_line {
            return true;
        }
        self.last_line.clear();
        self.last_line.push_str(line);
        let mut out = self.stdout.lock();
        out.write_all(self.last_line.as_bytes())
            .and_then(|()| out.flush())
            .is_ok()
    }
}

fn main() -> ExitCode {
    let cfg = match Config::from_args(std::env::args().skip(1)) {
        Ok(c) => c,
        Err(e) if e.is_empty() => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("peak-meter-way: {e}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match run(cfg) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("peak-meter-way: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(cfg: Config) -> Result<ExitCode, pw::Error> {
    pw::init();

    // The loop lives for the whole process. Leaking it gives `'static` sources that the
    // stream callbacks can hold without lifetime gymnastics.
    let mainloop: &'static pw::main_loop::MainLoopRc =
        Box::leak(Box::new(pw::main_loop::MainLoopRc::new(None)?));
    let context = pw::context::ContextRc::new(mainloop, None)?;
    let core = context.connect_rc(None)?;

    let failed = Rc::new(Cell::new(false));

    let _sigint = mainloop
        .loop_()
        .add_signal_local(Signal::INT, || mainloop.quit());
    let _sigterm = mainloop
        .loop_()
        .add_signal_local(Signal::TERM, || mainloop.quit());

    // If the PipeWire daemon goes away, exit non-zero so waybar's restart-interval kicks in.
    let _core_listener = core
        .add_listener_local()
        .error({
            let failed = failed.clone();
            move |id, _seq, res, msg| {
                if id == pw::core::PW_ID_CORE {
                    eprintln!("peak-meter-way: pipewire error {res}: {msg}");
                    failed.set(true);
                    mainloop.quit();
                }
            }
        })
        .register();

    let now = Instant::now();
    let shared = Arc::new(Shared {
        peak: AtomicU32::new(0),
        // Anything that would not light even the first sub-step is noise; ignore it.
        wake_threshold: meter::db_to_lin(cfg.floor_db).to_bits(),
        awake: AtomicBool::new(false),
    });
    let output = Rc::new(RefCell::new(Output {
        meter: Ballistics::new(&cfg, now),
        renderer: Renderer::new(&cfg),
        last_frame: now,
        last_line: String::new(),
        stdout: std::io::stdout(),
    }));

    // Draw the empty meter straight away so the module has its final width from the start.
    if !output.borrow_mut().emit() {
        return Ok(ExitCode::SUCCESS);
    }

    // The timer's own callback disarms it, so it needs a handle to itself.
    let timer_cell: Rc<OnceCell<&'static TimerSource<'static>>> = Rc::new(OnceCell::new());
    let timer: &'static _ = Box::leak(Box::new(mainloop.loop_().add_timer({
        let shared = shared.clone();
        let output = output.clone();
        let timer_cell = timer_cell.clone();
        move |_expirations| {
            let mut out = output.borrow_mut();
            let now = Instant::now();
            let dt = now.duration_since(out.last_frame);
            out.last_frame = now;

            let peak = f32::from_bits(shared.peak.swap(0, Ordering::SeqCst));
            out.meter.step(lin_to_db(peak), dt, now);
            if !out.emit() {
                mainloop.quit();
                return;
            }
            if out.meter.is_idle() && shared.try_sleep() {
                if let Some(t) = timer_cell.get() {
                    let _ = t.update_timer(None, None);
                }
            }
        }
    })));
    let _ = timer_cell.set(timer);

    // Woken from the realtime thread when sound shows up while the meter is asleep.
    let frame = cfg.frame;
    let wake: &'static _ = Box::leak(Box::new(mainloop.loop_().add_event(move || {
        output.borrow_mut().last_frame = Instant::now();
        // Fire right away, then at the frame rate.
        let _ = timer.update_timer(Some(Duration::from_nanos(1)), Some(frame));
    })));

    let mut props = properties! {
        *pw::keys::MEDIA_TYPE => "Audio",
        *pw::keys::MEDIA_CATEGORY => "Capture",
        *pw::keys::MEDIA_ROLE => "Music",
        *pw::keys::NODE_NAME => "peak-meter-way",
        *pw::keys::NODE_DESCRIPTION => "Waybar peak meter",
        *pw::keys::APP_NAME => "peak-meter-way",
        // Never ask the graph for a smaller quantum than it already runs at.
        *pw::keys::NODE_LATENCY => "2048/48000",
        // Keep the device's native channel layout; we only need the overall peak.
        "stream.dont-remix" => "true",
    };
    if cfg.monitor {
        props.insert(*pw::keys::STREAM_CAPTURE_SINK, "true");
        // Don't keep the sink running just for the meter.
        props.insert(*pw::keys::NODE_PASSIVE, "true");
    }
    if let Some(target) = &cfg.target {
        props.insert("target.object", target.as_str());
    }

    let stream = pw::stream::StreamBox::new(&core, "peak-meter-way", props)?;
    let _stream_listener = stream
        .add_local_listener_with_user_data(())
        .state_changed({
            let failed = failed.clone();
            move |_, _, _old, new| {
                if let pw::stream::StreamState::Error(e) = new {
                    eprintln!("peak-meter-way: stream error: {e}");
                    failed.set(true);
                    mainloop.quit();
                }
            }
        })
        // Runs on PipeWire's realtime data thread (RT_PROCESS): touch only atomics.
        .process(move |stream, _| {
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            let Some(data) = buffer.datas_mut().first_mut() else {
                return;
            };
            let (offset, size) = (data.chunk().offset() as usize, data.chunk().size() as usize);
            let Some(bytes) = data.data() else {
                return;
            };
            let start = offset.min(bytes.len());
            let end = start.saturating_add(size).min(bytes.len());
            // Whole samples only.
            let end = start + (end - start) / 4 * 4;
            if shared.record(peak_bits(&bytes[start..end])) {
                // eventfd write; signalling a loop source is thread-safe.
                let _ = wake.signal();
            }
        })
        .register()?;

    // Accept F32 at the graph's native rate and channel count: no resampling, no remixing.
    let mut info = spa::param::audio::AudioInfoRaw::new();
    info.set_format(spa::param::audio::AudioFormat::F32LE);
    let pod = spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &spa::pod::Value::Object(spa::pod::Object {
            type_: spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
            id: spa::param::ParamType::EnumFormat.as_raw(),
            properties: info.into(),
        }),
    )
    .expect("serializing a fixed format pod cannot fail")
    .0
    .into_inner();
    let mut params = [spa::pod::Pod::from_bytes(&pod).expect("valid pod")];

    stream.connect(
        spa::utils::Direction::Input,
        None,
        pw::stream::StreamFlags::AUTOCONNECT
            | pw::stream::StreamFlags::MAP_BUFFERS
            | pw::stream::StreamFlags::RT_PROCESS,
        &mut params,
    )?;

    mainloop.run();

    Ok(if failed.get() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}
