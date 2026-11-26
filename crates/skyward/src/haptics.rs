use crate::waveform::{made, scaled, share, ticks, RATE, TICKS_RATE};
use anyhow::{anyhow, ensure, Context, Result};
use evdev::raw_stream::RawDevice;
use skyward_config::Strength;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// linux/input-event-codes.h
const EV_FF: u16 = 0x15;
const FF_PERIODIC: u16 = 0x51;
const FF_CUSTOM: u16 = 0x5d;

/// The ticks play at this share of the drive the driver gives samples at in
/// full, whatever the slider says: the 17 Ultra's own system plays them at
/// 3527 mV of the 8500 mV its device tree gives.
const TICKS_STRENGTH: f32 = 0.415;
/// How long the ticks stay loaded once their samples have run out, as the
/// phone's own system leaves them: the motor comes to rest on its own
/// before taking them away brakes it.
const TICKS_REST: Duration = Duration::from_millis(40);

/// The vibration motor, played through the kernel's force-feedback
/// events. The driver fills its queue with a made click's samples as the
/// click is loaded, refilling it for no second play, and keeps the click
/// playing after its samples have run out, even when told to stop, until
/// the click is taken away. So each click is loaded as it is played, at the
/// strength the slider gives, and taken away once it has played out.
pub struct Haptics {
    motor: Motor,
    strength: Strength,
    scale_samples: bool,
    /// The click playing, by what takes it away and its count, until it is
    /// taken away; a click taken away early by the next is not taken away
    /// again.
    playing: Arc<Mutex<Option<(Taking, u64)>>>,
    clicks: u64,
}

/// What the motor is reached through.
enum Motor {
    /// The kernel's force-feedback events, on an input device that takes
    /// periodic effects. The ticks are made where the motor's resonant
    /// frequency is known; without it the click's own waveform plays for
    /// the screen going off.
    Events {
        dev: File,
    click: Click,
    ticks: Option<Click>,
    },
    /// A driver with a folder of its own that plays a timed drive: its
    /// `duration` file takes how long the motor is driven for, in whole
    /// milliseconds, and its `activate` file starts that drive, or stops it
    /// when told 0. What it drives the motor at is a voltage of the driver's
    /// own choosing, so the only thing a click's strength can reach here is
    /// how long the drive lasts; the `brightness` file such a folder usually
    /// carries does nothing at all, its driver's own setter being empty.
    Timed { folder: PathBuf, drive: u32 },
    /// Legacy Android timed_output: write milliseconds directly to enable.
    TimedOutput { folder: PathBuf, drive: u32 },
}

/// What a click is, for a driver that plays waveforms or samples.
#[derive(Clone)]
enum Click {
    /// A waveform the driver keeps, by its number. The driver says how long
    /// it plays as it loads it.
    Kept(i16),
    /// Samples made at the motor's resonant frequency, which the driver
    /// plays as they are, at `rate` a second.
    Samples { data: Vec<i8>, rate: u32 },
}

/// What takes a click away.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Taking {
    /// An effect in a slot of the input device's.
    Slot(i16),
    /// A drive the driver stops when told to.
    Drive,
}

impl Haptics {
    /// The motor and the configured click. Loading a click fills a
    /// force-feedback driver's queue with what it plays, so the click is
    /// loaded only as it is played.
    pub fn open(cfg: &skyward_config::Haptics) -> Result<Haptics> {
        let motor = match (cfg.click, &cfg.resonance, &cfg.timed, &cfg.timed_output) {
            (Some(id), file, _, _) => {
        let (path, dev) = events()?;
        let ticks = match file {
                    Some(file) => {
                        let hz = resonance(file)?;
                        eprintln!("haptics {}: clicks with waveform {id}, ticks made for a motor swinging at {hz} Hz", path.display());
                        Some(Click::Samples {
                            data: ticks(hz),
                            rate: TICKS_RATE,
                        })
            }
            None => {
                eprintln!("haptics {}: clicks with waveform {id}", path.display());
                        None
                    }
                };
                Motor::Events {
                    dev,
                    click: Click::Kept(id),
                    ticks,
                }
            }
            (None, Some(file), _, _) => {
                let (path, dev) = events()?;
                let hz = resonance(file)?;
                eprintln!("haptics {}: clicks and ticks made for a motor swinging at {hz} Hz", path.display());
                Motor::Events {
                    dev,
                    click: Click::Samples { data: made(hz), rate: RATE },
                    ticks: Some(Click::Samples { data: ticks(hz), rate: TICKS_RATE,
                    }),
                }
            }
            (None, None, Some(folder), None) => {
                let folder = PathBuf::from(folder);
                ensure!(
                    folder.is_dir(),
                    "{} is no driver folder to write a click to",
                    folder.display()
                );
                eprintln!(
                    "haptics {}: clicks of a drive of {} ms at the most",
                    folder.display(),
                    cfg.drive
                );
                Motor::Timed {
                    folder,
                    drive: cfg.drive,
                }
            }
            (None, None, None, Some(folder)) => {
                let folder = PathBuf::from(folder);
                ensure!(
                    folder.join("enable").is_file(),
                    "{} has no timed_output enable file",
                    folder.display()
                );
                eprintln!(
                    "haptics {}: timed_output clicks of at most {} ms",
                    folder.display(), cfg.drive
                );
                Motor::TimedOutput { folder, drive: cfg.drive }
            }
            _ => unreachable!("the configuration names a click of one kind"),
        };
        Ok(Haptics {
            motor, strength: cfg.strength, scale_samples: cfg.scale_samples, playing: Arc::new(Mutex::new(None)), clicks: 0 })
    }

    /// One short click at the strength the slider's `position` gives, after
    /// taking away one still playing. Gives how long it plays.
    pub fn click(&mut self, position: f32) -> Result<Duration> {
        self.start(false, share(&self.strength, position))
    }

    /// The ticks of the screen going off, as strong as the phone's own
    /// system plays them whatever the slider says. Gives how long they play.
    pub fn ticks(&mut self) -> Result<Duration> {
        self.start(true, TICKS_STRENGTH)
    }

    /// Play the ticks, or else the click, at `share` of its full strength,
    /// after taking away what still plays.
    fn start(&mut self, ticks: bool, share: f32) -> Result<Duration> {
        let mut playing = self.playing.lock().expect("no thread panics while it holds the click playing");
        if let Some((taking, _)) = playing.take() {
            self.motor.stop(taking).context("take away the click still playing")?;
        }
        let (taking, length) = self.motor.start(ticks, share, self.scale_samples)?;
        self.clicks += 1;
        let this = (taking, self.clicks);
        *playing = Some(this);
        drop(playing);
        let motor = self.motor.share()?;
        // Timed from now: a thread may wait a while before it first runs.
        let until = Instant::now() + length;
        let playing = self.playing.clone();
        std::thread::spawn(move || {
            std::thread::sleep(until.saturating_duration_since(Instant::now()));
            let mut playing = playing.lock().expect("no thread panics while it holds the click playing");
            if *playing == Some(this) {
                *playing = None;
                if let Err(e) = motor.stop(this.0) {
                    eprintln!("take away the click: {e}");
                }
            }
        });
        Ok(length)
    }
}

impl Motor {
    /// Play the click, or the ticks of the screen going off, once at
    /// `share` of its full strength, saying what takes it away and how
    /// long it plays.
    fn start(&self, ticks: bool, share: f32, scale_samples: bool) -> Result<(Taking, Duration)> {
        match self {
            Motor::Events {
                dev,
                click,
                ticks: tick,
            } => {
                let effect = if ticks { tick.as_ref().unwrap_or(click) } else { click };
        let loaded = upload(dev, effect, share, scale_samples).context("load the click")?;
        play(dev, loaded.slot).context("play the click")?;
                // The driver keeps the ticks playing after their samples
                // run out; the motor comes to rest on its own first.
                let length = loaded.length + if ticks { TICKS_REST } else { Duration::ZERO };
                Ok((Taking::Slot(loaded.slot), length))
            }
            Motor::Timed { folder, drive } => {
                // A lighter click is a shorter drive. The voltage the motor
                // is driven at is the driver's own, and writing it for every
                // click is a reconfiguration of the driver rather than a
                // click, and one the motor's datasheet would have to bound.
                let ms = (*drive as f32 * share).round().max(1.0) as u32;
                write_at(folder, "duration", &ms.to_string())
                    .with_context(|| format!("ask the driver for a drive of {ms} ms"))?;
                write_at(folder, "activate", "1").context("ask the driver to start the drive")?;
                Ok((Taking::Drive, Duration::from_millis(u64::from(ms))))
            }
            Motor::TimedOutput { folder, drive } => {
                // Keep the driver's own voltage and waveform settings.
                // The strength slider changes only the drive duration.
                let ms = (*drive as f32 * share).round().max(1.0) as u32;
                write_at(folder, "enable", &ms.to_string())
                    .with_context(|| format!("start a timed_output drive of {ms} ms"))?;
                Ok((Taking::Drive, Duration::from_millis(u64::from(ms))))
            }
        }
    }

    /// Stop the click playing, for good.
    fn stop(&self, taking: Taking) -> io::Result<()> {
        match (self, taking) {
            (Motor::Events { dev, .. }, Taking::Slot(slot)) => erase(dev, slot),
            (Motor::Timed { folder, .. }, Taking::Drive) => write_at(folder, "activate", "0"),
            (Motor::TimedOutput { folder, .. }, Taking::Drive) => write_at(folder, "enable", "0"),
            // A click of one motor taken away from the other is no click at
            // all playing, and there is nothing to stop.
            _ => Ok(()),
        }
    }

    /// The same motor again, for the thread that takes the click away.
    fn share(&self) -> Result<Motor> {
        Ok(match self {
            Motor::Events { dev, click, ticks } => Motor::Events {
                dev: dev.try_clone().context("share the motor")?,
                click: click.clone(),
                ticks: ticks.clone(),
            },
            Motor::Timed { folder, drive } => Motor::Timed {
                folder: folder.clone(),
                drive: *drive,
            },
            Motor::TimedOutput { folder, drive } => Motor::TimedOutput {
                folder: folder.clone(),
                drive: *drive,
            },
        })
    }
}

/// Write one value into a file of the driver's own folder.
fn write_at(folder: &Path, name: &str, value: &str) -> io::Result<()> {
    let mut file = OpenOptions::new().write(true).open(folder.join(name))?;
    file.write_all(value.as_bytes())
}

/// Play the configured click once at the slider's `position`, or else the
/// ticks of the screen going off, saying how strongly and for how long, for
/// trying out a motor.
pub fn probe(cfg: &skyward_config::Haptics, position: f32, ticks: bool) -> Result<()> {
    let mut haptics = Haptics::open(cfg)?;
    let (what, strength, length) = if ticks {
        ("ticks", TICKS_STRENGTH, haptics.ticks()?)
    } else {
        ("click", share(&cfg.strength, position), haptics.click(position)?)
    };
    // Once it has played out, what played goes with the device as the
    // command ends.
    std::thread::sleep(length);
    println!("played the {what} at {strength:.3} of the full strength, {} ms long", length.as_millis());
    Ok(())
}

/// The first input device under /dev/input that takes periodic
/// force-feedback effects, opened for writing.
fn events() -> Result<(PathBuf, File)> {
    for (path, dev) in evdev::raw_stream::enumerate() {
        if takes_periodic(&dev) {
            let file = OpenOptions::new().read(true).write(true).open(&path).with_context(|| format!("open {}", path.display()))?;
            return Ok((path, file));
        }
    }
    Err(anyhow!("no input device takes force-feedback effects"))
}

fn takes_periodic(dev: &RawDevice) -> bool {
    dev.supported_ff().is_some_and(|ff| ff.contains(evdev::FFEffectCode::FF_PERIODIC))
}

/// The motor's resonant frequency in hertz, from the file the driver says
/// it in, in tenths of a hertz.
fn resonance(file: &str) -> Result<f32> {
    let text = fs::read_to_string(file).with_context(|| format!("read {file}"))?;
    let tenths: u32 = text.split_whitespace().next().and_then(|n| n.parse().ok()).with_context(|| format!("{file} holds {text:?}"))?;
    ensure!(tenths > 0, "{file} says the motor swings at 0 Hz");
    Ok(tenths as f32 / 10.0)
}

/// A click as the driver loaded it: the slot the kernel keeps it in, and
/// how long it plays.
struct Loaded {
    slot: i16,
    length: Duration,
}

/// Samples handed to the driver to play as they are, as its
/// `custom_fifo_data` lays them out.
#[repr(C)]
struct Samples {
    /// Unused by the driver.
    index: u32,
    length: u32,
    rate: u32,
    data: *const i8,
}

/// Load `click` at `share` of its full strength as a custom periodic
/// effect in a slot of its own. The driver drives the motor at the click's
/// highest voltage times the magnitude; where it is given the strength in
/// the samples it plays, `scale_samples`, they are made that share of
/// themselves and the magnitude is the full one.
fn upload(dev: &File, click: &Click, share: f32, scale_samples: bool) -> Result<Loaded> {
    let share = share.clamp(0.0, 1.0);
    // Kept until the driver has copied what the effect points to.
    let lighter = match click {
        Click::Samples { data, .. } if scale_samples => Some(scaled(data, share)),
        _ => None,
    };
    // The driver tells the two kinds apart by the length in bytes of what
    // the effect points to, and loads nothing for any other length, though
    // the upload still succeeds. For a waveform it keeps it reads the
    // waveform's number from the first of three values and writes how long
    // it plays into the other two, in whole seconds and then milliseconds.
    let mut kept: [i16; 3] = [0; 3];
    let mut samples = Samples { index: 0, length: 0, rate: RATE, data: std::ptr::null() };
    let (len, data) = match click {
        Click::Kept(id) => {
            kept[0] = *id;
            (std::mem::size_of_val(&kept), kept.as_mut_ptr())
        }
        Click::Samples { data, rate } => {
            samples.length = u32::try_from(data.len())?;
            samples.rate = *rate;
            samples.data = lighter.as_deref().unwrap_or(data).as_ptr();
            (std::mem::size_of::<Samples>(), (&mut samples as *mut Samples).cast::<i16>())
        }
    };
    let periodic = libc::ff_periodic_effect {
        waveform: FF_CUSTOM,
        period: 0,
        magnitude: if lighter.is_some() { i16::MAX } else { (share * f32::from(i16::MAX)).round() as i16 },
        offset: 0,
        phase: 0,
        envelope: libc::ff_envelope { attack_length: 0, attack_level: 0, fade_length: 0, fade_level: 0 },
        custom_len: len as u32,
        custom_data: data,
    };
    let mut effect = libc::ff_effect {
        type_: FF_PERIODIC,
        id: -1,
        direction: 0,
        trigger: libc::ff_trigger { button: 0, interval: 0 },
        replay: libc::ff_replay { length: 0, delay: 0 },
        u: [0; 4],
    };
    // The union holds the periodic effect; it is exactly as large.
    const _: () = assert!(std::mem::size_of::<libc::ff_periodic_effect>() == std::mem::size_of::<[u64; 4]>());
    effect.u = unsafe { std::mem::transmute::<libc::ff_periodic_effect, [u64; 4]>(periodic) };
    // EVIOCSFF: _IOW('E', 0x80, struct ff_effect)
    let size = std::mem::size_of::<libc::ff_effect>() as u32;
    let request = (1u32 << 30) | (size << 16) | (u32::from(b'E') << 8) | 0x80;
    if unsafe { libc::ioctl(dev.as_raw_fd(), request as _, &mut effect) } < 0 {
        return Err(io::Error::last_os_error().into());
    }
    let length = match click {
        Click::Kept(id) => {
            let [_, seconds, millis] = kept;
            let length = Duration::from_secs(u64::try_from(seconds)?) + Duration::from_millis(u64::try_from(millis)?);
            ensure!(!length.is_zero(), "the driver loaded waveform {id} without saying how long it plays");
            length
        }
        // The queue is full before the samples start playing.
        Click::Samples { data, rate } => Duration::from_secs_f64(data.len() as f64 / f64::from(*rate)),
    };
    Ok(Loaded { slot: effect.id, length })
}

/// Start the loaded effect in slot `id`.
fn play(dev: &File, id: i16) -> io::Result<()> {
    let mut out = dev;
    let event = libc::input_event { time: libc::timeval { tv_sec: 0, tv_usec: 0 }, type_: EV_FF, code: id as u16, value: 1 };
    let bytes = unsafe { std::slice::from_raw_parts((&event as *const libc::input_event).cast::<u8>(), std::mem::size_of::<libc::input_event>()) };
    out.write_all(bytes)
}

/// Take the effect in slot `id` away, which stops it for good.
fn erase(dev: &File, id: i16) -> io::Result<()> {
    // EVIOCRMFF: _IOW('E', 0x81, int)
    let request = (1u32 << 30) | ((std::mem::size_of::<libc::c_int>() as u32) << 16) | (u32::from(b'E') << 8) | 0x81;
    if unsafe { libc::ioctl(dev.as_raw_fd(), request as _, libc::c_int::from(id)) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
