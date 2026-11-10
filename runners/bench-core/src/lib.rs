//! Bevy-independent shared code for the Spine benchmark runners. See `BENCHMARK_SPEC.md`.

use clap::Parser;
use serde::Deserialize;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub const WINDOW_W: f32 = 1920.0;
pub const WINDOW_H: f32 = 1080.0;

// ---------------------------------------------------------------- CLI

/// Common CLI for all runners.
#[derive(Parser, Debug, Clone)]
pub struct Args {
    /// Scene TOML file.
    #[arg(long)]
    pub scene: PathBuf,
    /// Override the number of onscreen instances.
    #[arg(long)]
    pub onscreen: Option<u32>,
    /// Override the number of offscreen instances.
    #[arg(long)]
    pub offscreen: Option<u32>,
    /// Warmup seconds after loading.
    #[arg(long, default_value_t = 3.0)]
    pub warmup: f64,
    /// Measured seconds.
    #[arg(long, default_value_t = 15.0)]
    pub duration: f64,
    /// Skip the final screenshot.
    #[arg(long)]
    pub no_screenshot: bool,
    /// Interactive mode (vsync, pan/zoom, FPS in title, no measuring).
    #[arg(long)]
    pub view: bool,
    /// Hold every instance at its start pose (animation time scale 0, `bob` motion at t = 0), so screenshots are deterministic
    /// across runs. For visual comparisons only; currently honoured by the spine-gpu runner.
    #[arg(long)]
    pub freeze: bool,
}

impl Args {
    pub fn parse_args() -> Args {
        Args::parse()
    }
}

// ---------------------------------------------------------------- paths

fn strip_verbatim(p: PathBuf) -> PathBuf {
    match p.to_str().and_then(|s| s.strip_prefix(r"\\?\")) {
        Some(rest) if !rest.starts_with("UNC\\") => PathBuf::from(rest),
        _ => p,
    }
}

/// Repo root: canonicalized `CARGO_MANIFEST_DIR/../..` (no `\\?\` prefix on Windows).
pub fn repo_root() -> PathBuf {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    strip_verbatim(p.canonicalize().unwrap_or(p))
}
pub fn assets_dir() -> PathBuf {
    repo_root().join("assets")
}
pub fn results_dir() -> PathBuf {
    repo_root().join("results")
}

/// `results/screenshots/<backend>_<scene>.png`; creates the directory.
pub fn screenshot_path(backend: &str, scene: &Scene) -> PathBuf {
    let dir = results_dir().join("screenshots");
    let _ = fs::create_dir_all(&dir);
    dir.join(format!("{backend}_{}.png", scene.name))
}

// ---------------------------------------------------------------- scene

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnimMode {
    Same,
    SameRandomPhase,
    Random,
}

impl AnimMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            AnimMode::Same => "same",
            AnimMode::SameRandomPhase => "same_random_phase",
            AnimMode::Random => "random",
        }
    }
}

/// How overlapping instances are ordered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DrawOrder {
    /// z = 0 for all; overlaps are not ordered (spine-gpu: static types).
    #[default]
    Any,
    /// Instances whose feet are lower on screen (smaller y) are drawn in front of those they overlap.
    YSorted,
}

/// Whether instances move.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Motion {
    #[default]
    None,
    /// Vertical sine motion, see `InstanceSpec::pos_at`.
    Bob,
}

/// Period of the `bob` motion in seconds.
pub const BOB_PERIOD_SECS: f64 = 4.0;

/// Scene file as parsed from TOML.
#[derive(Debug, Deserialize)]
struct SceneFile {
    rig: Option<String>,
    rigs: Option<Vec<String>>,
    animations: Vec<String>,
    anim_mode: AnimMode,
    onscreen: u32,
    #[serde(default)]
    offscreen: u32,
    scale: f32,
    seed: u64,
    #[serde(default)]
    draw_order: DrawOrder,
    #[serde(default)]
    motion: Motion,
}

#[derive(Debug, Clone)]
pub struct Scene {
    /// Set from the file stem by `Scene::load`.
    pub name: String,
    /// One or more rigs (`rig = "x"` in the file is a single-element list).
    pub rigs: Vec<String>,
    pub animations: Vec<String>,
    pub anim_mode: AnimMode,
    pub onscreen: u32,
    pub offscreen: u32,
    pub scale: f32,
    pub seed: u64,
    pub draw_order: DrawOrder,
    pub motion: Motion,
}

impl Scene {
    /// Load a scene (relative paths: cwd first, then repo root) and apply CLI overrides.
    pub fn load(path: &Path, args: &Args) -> Result<Scene, String> {
        let mut full = path.to_path_buf();
        if full.is_relative() && !full.exists() {
            full = repo_root().join(path);
        }
        let text = fs::read_to_string(&full).map_err(|e| format!("cannot read {}: {e}", full.display()))?;
        let f: SceneFile = toml::from_str(&text).map_err(|e| format!("bad scene {}: {e}", full.display()))?;
        if f.animations.is_empty() {
            return Err(format!("scene {} has no animations", full.display()));
        }
        let rigs = match (f.rig, f.rigs) {
            (Some(r), None) => vec![r],
            (None, Some(r)) if !r.is_empty() => r,
            _ => return Err(format!("scene {} needs exactly one of `rig` and a non-empty `rigs`", full.display())),
        };
        let mut s = Scene {
            name: full.file_stem().map(|x| x.to_string_lossy().into_owned()).unwrap_or_default(),
            rigs,
            animations: f.animations,
            anim_mode: f.anim_mode,
            onscreen: f.onscreen,
            offscreen: f.offscreen,
            scale: f.scale,
            seed: f.seed,
            draw_order: f.draw_order,
            motion: f.motion,
        };
        if let Some(n) = args.onscreen {
            s.onscreen = n;
        }
        if let Some(n) = args.offscreen {
            s.offscreen = n;
        }
        Ok(s)
    }

    /// Rig names joined with `+` (the `rig` CSV column); just the name for single-rig scenes.
    pub fn rig_label(&self) -> String {
        self.rigs.join("+")
    }

    /// Asset-relative `(json, atlas)` per rig (index = `InstanceSpec::rig`),
    /// e.g. `rigs/ella/Ella.json`, `rigs/ella/Ella.atlas`.
    pub fn rig_paths(&self) -> Vec<(String, String)> {
        self.rigs
            .iter()
            .map(|rig| {
                let dir = rig.to_lowercase();
                let mut c = rig.chars();
                let stem: String = c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default();
                (format!("rigs/{dir}/{stem}.json"), format!("rigs/{dir}/{stem}.atlas"))
            })
            .collect()
    }

    /// Deterministic placement and animation state: all onscreen, then all offscreen.
    pub fn instances(&self) -> Vec<InstanceSpec> {
        let mut rng = SplitMix64::new(self.seed);
        let (on, on_cell_h) = grid(self.onscreen, -960.0 + 40.0, 960.0 - 40.0, -540.0 + 40.0, 540.0 - 40.0);
        let (off, off_cell_h) = grid(self.offscreen, -10000.0, 10000.0, 100000.0 - 10000.0, 100000.0 + 10000.0);
        let n_anims = self.animations.len() as u64;
        let n_rigs = self.rigs.len() as u64;
        let bob = self.motion == Motion::Bob;
        on.into_iter()
            .map(|p| (p, true, on_cell_h))
            .chain(off.into_iter().map(|p| (p, false, off_cell_h)))
            .enumerate()
            .map(|(index, (pos, onscreen, cell_h))| {
                // Always draw exactly two values, regardless of mode.
                let a = rng.next_u64();
                let p = rng.next_f64();
                let (anim, phase) = match self.anim_mode {
                    AnimMode::Same => (0, 0.0),
                    AnimMode::SameRandomPhase => (0, p),
                    AnimMode::Random => ((a % n_anims) as usize, p),
                };
                InstanceSpec {
                    index,
                    rig: ((a >> 32) % n_rigs) as usize,
                    pos: [pos[0], pos[1], 0.0],
                    scale: self.scale,
                    anim: self.animations[anim].clone(),
                    phase_frac: phase as f32,
                    onscreen,
                    bob_amp: if bob { cell_h } else { 0.0 },
                    bob_phase: if bob { ((a >> 16) & 0xffff) as f32 / 65536.0 } else { 0.0 },
                }
            })
            .collect()
    }
}

/// One skeleton instance. Start `anim` looping on track 0 with time `phase_frac * duration`.
#[derive(Debug, Clone, PartialEq)]
pub struct InstanceSpec {
    pub index: usize,
    /// Index into `Scene::rigs` / `Scene::rig_paths()`.
    pub rig: usize,
    /// Base position (the feet; z = 0). Moving scenes: use `pos_at`.
    pub pos: [f32; 3],
    pub scale: f32,
    pub anim: String,
    pub phase_frac: f32,
    pub onscreen: bool,
    /// Vertical `bob` amplitude (= cell height of the instance's grid), 0 without motion.
    pub bob_amp: f32,
    /// Phase of the `bob` motion in periods, in [0,1).
    pub bob_phase: f32,
}

impl InstanceSpec {
    /// Position (z = 0) of the instance `t` seconds after app start:
    /// `y(t) = base_y + amp * sin(2π * (t / 4s + bob_phase))`, x fixed. Without motion this is `pos`.
    pub fn pos_at(&self, t: f64) -> [f32; 3] {
        if self.bob_amp == 0.0 {
            return self.pos;
        }
        let ph = t / BOB_PERIOD_SECS + self.bob_phase as f64;
        let dy = self.bob_amp as f64 * (std::f64::consts::TAU * ph).sin();
        [self.pos[0], self.pos[1] + dy as f32, self.pos[2]]
    }

    /// Sort value at time `t`: the feet y (world, y up). Smaller = lower on screen = drawn in front of
    /// overlapping instances with larger values (for `DrawOrder::YSorted`).
    pub fn sort_y(&self, t: f64) -> f32 {
        self.pos_at(t)[1]
    }
}

/// Grid of `n` positions over the rect (y up). Row 0 is the top row. Position is the cell's
/// horizontal center and `cell_center_y - 0.4*cell_h` (skeleton origin is at the feet).
/// Also returns the cell height.
fn grid(n: u32, x0: f32, x1: f32, y0: f32, y1: f32) -> (Vec<[f32; 2]>, f32) {
    if n == 0 {
        return (vec![], 0.0);
    }
    let (w, h) = (x1 - x0, y1 - y0);
    let cols = ((n as f64 * (w / h) as f64).sqrt().ceil() as u32).max(1);
    let rows = n.div_ceil(cols);
    let (cw, ch) = (w / cols as f32, h / rows as f32);
    let pts = (0..n)
        .map(|k| {
            let (c, r) = ((k % cols) as f32, (k / cols) as f32);
            let cy = y1 - (r + 0.5) * ch;
            [x0 + (c + 0.5) * cw, cy - 0.4 * ch]
        })
        .collect();
    (pts, ch)
}

/// World z per pixel of feet y for `y_sorted` scenes on backends that order by `Transform` z (Bevy 0.19 backends).
pub const Y_SORT_Z_PER_PX: f32 = 0.5;
/// Feet y beyond which the z mapping saturates (keeps z inside the default 2D camera's [-1000, 1000]; only
/// the far-offscreen instances at y ~ 100000 are affected, and they are never visible).
pub const Y_SORT_MAX_Y: f32 = 1000.0;

/// Transform z for an instance with feet at `sort_y` (see `InstanceSpec::sort_y`): `-0.5 * y` clamped to
/// +-500, so lower on screen = larger z = in front. 1 px of y is 0.5 z, which is far more than the slot
/// z offsets the backends add within one skeleton (18 slots * 0.001), so a skeleton stays between its y-neighbours.
pub fn y_sorted_z(sort_y: f32) -> f32 {
    -sort_y.clamp(-Y_SORT_MAX_Y, Y_SORT_MAX_Y) * Y_SORT_Z_PER_PX
}

// ---------------------------------------------------------------- RNG

/// SplitMix64 (Vigna).
pub struct SplitMix64(u64);

impl SplitMix64 {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform in [0,1) from the top 53 bits.
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

// ---------------------------------------------------------------- lifecycle

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Loading,
    Warmup,
    Measure,
    Screenshot,
    Done,
}

/// Advances the runner lifecycle. Call `update` once per frame; record frames only while `Measure`.
/// In `Screenshot`, request the screenshot, wait for the file, then call `screenshot_done()`.
#[derive(Debug, Clone)]
pub struct BenchClock {
    pub phase: Phase,
    pub load_secs: f64,
    pub measure_secs: f64,
    start: Instant,
    warmup: f64,
    duration: f64,
    screenshot: bool,
    phase_start: Instant,
}

impl BenchClock {
    /// `start` = app start time (load_secs is measured from it).
    pub fn new(start: Instant, args: &Args) -> Self {
        Self {
            phase: Phase::Loading,
            load_secs: 0.0,
            measure_secs: 0.0,
            start,
            warmup: args.warmup,
            duration: args.duration,
            screenshot: !args.no_screenshot,
            phase_start: start,
        }
    }

    /// Seconds since app start (the `t` for `InstanceSpec::pos_at`).
    pub fn elapsed_secs(&self, now: Instant) -> f64 {
        now.duration_since(self.start).as_secs_f64()
    }

    pub fn update(&mut self, now: Instant, all_ready: bool) -> Phase {
        let elapsed = now.duration_since(self.phase_start).as_secs_f64();
        match self.phase {
            Phase::Loading if all_ready => {
                self.load_secs = now.duration_since(self.start).as_secs_f64();
                self.enter(Phase::Warmup, now);
            }
            Phase::Warmup if elapsed >= self.warmup => self.enter(Phase::Measure, now),
            Phase::Measure if elapsed >= self.duration => {
                self.measure_secs = elapsed;
                let next = if self.screenshot { Phase::Screenshot } else { Phase::Done };
                self.enter(next, now);
            }
            _ => {}
        }
        self.phase
    }

    /// Call once the screenshot file has been written.
    pub fn screenshot_done(&mut self) {
        if self.phase == Phase::Screenshot {
            self.phase = Phase::Done;
        }
    }

    fn enter(&mut self, p: Phase, now: Instant) {
        self.phase = p;
        self.phase_start = now;
    }
}

// ---------------------------------------------------------------- disturbances

/// Dedup'd, insertion-ordered set of short reason strings ("focus_lost", "occluded", "resized",
/// "scale_changed", "moved", "unfocused_at_start", "spike", "wrong_size"). A run with any reason is "disturbed".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Disturbances(Vec<String>);

impl Disturbances {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a reason (ignored if already present).
    pub fn note(&mut self, reason: &str) {
        if !self.0.iter().any(|r| r == reason) {
            self.0.push(reason.to_string());
        }
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn reasons(&self) -> &[String] {
        &self.0
    }

    /// Reasons joined with `;` (CSV-safe).
    pub fn joined(&self) -> String {
        self.0.join(";")
    }
}

/// A frame is a spike if it took more than this many times the median frame time.
pub const SPIKE_FACTOR: f64 = 20.0;

// ---------------------------------------------------------------- stats

#[derive(Default)]
pub struct FrameStats {
    rows: Vec<[Option<f64>; 4]>,
}

pub struct RunMeta<'a> {
    pub backend: &'a str,
    pub bevy_version: &'a str,
    pub scene: &'a Scene,
    pub load_secs: f64,
    pub measure_secs: f64,
    pub present_mode: String,
    /// Window/environment disturbances noted by the runner during warmup + measure.
    pub disturbances: &'a Disturbances,
    /// Refresh rate of the window's monitor in Hz, if known.
    pub refresh_hz: Option<f64>,
    /// Physical window size at measure start.
    pub window_px: (u32, u32),
}

const HEADER: &str = "timestamp,backend,bevy_version,scene,rig,anim_mode,onscreen,offscreen,load_secs,frames,measure_secs,fps_mean,frame_ms_mean,frame_ms_median,frame_ms_p95,frame_ms_p99,frame_ms_max,main_ms_mean,render_cpu_ms_mean,gpu_ms_mean,present_mode,vsync_suspect,disturbed,disturb_reasons,refresh_hz,window_px";

/// Ensure `<dir>/results.csv` exists with the current header. An existing file whose first line differs is
/// renamed to `results.csv.old-<ts_file>` (returned) and a fresh file is started.
fn prepare_csv(dir: &Path, ts_file: &str) -> std::io::Result<(PathBuf, Option<PathBuf>)> {
    fs::create_dir_all(dir.join("frames"))?;
    let csv = dir.join("results.csv");
    let mut rotated = None;
    if csv.exists() {
        let first = fs::read_to_string(&csv)?.lines().next().unwrap_or("").trim_end().to_string();
        if first == HEADER {
            return Ok((csv, None));
        }
        let old = dir.join(format!("results.csv.old-{ts_file}"));
        fs::rename(&csv, &old)?;
        rotated = Some(old);
    }
    let mut f = fs::File::create(&csv)?;
    writeln!(f, "{HEADER}")?;
    Ok((csv, rotated))
}

#[derive(Debug, Clone)]
pub struct Summary {
    pub frames: usize,
    pub fps_mean: f64,
    pub frame_ms_mean: f64,
    pub frame_ms_median: f64,
    pub frame_ms_p95: f64,
    pub frame_ms_p99: f64,
    pub frame_ms_max: f64,
    pub main_ms_mean: f64,
    pub render_cpu_ms_mean: Option<f64>,
    pub gpu_ms_mean: Option<f64>,
    pub vsync_suspect: bool,
}

fn mean(v: &[f64]) -> Option<f64> {
    (!v.is_empty()).then(|| v.iter().sum::<f64>() / v.len() as f64)
}

/// Nearest-rank percentile of an ascending-sorted slice, `p` in (0,1].
fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((p * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len()) - 1;
    sorted[idx]
}

fn median(sorted: &[f64]) -> f64 {
    let n = sorted.len();
    match n {
        0 => 0.0,
        _ if n % 2 == 1 => sorted[n / 2],
        _ => (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0,
    }
}

fn opt(v: Option<f64>) -> String {
    v.map(|x| format!("{x:.4}")).unwrap_or_default()
}

/// (ISO-8601 UTC for the CSV, filename-safe `YYYYMMDDTHHMMSSZ`).
fn timestamps() -> (String, String) {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
    let (days, rem) = (secs.div_euclid(86400), secs.rem_euclid(86400));
    // Howard Hinnant's civil_from_days.
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + (m <= 2) as i64;
    let (hh, mm, ss) = (rem / 3600, rem % 3600 / 60, rem % 60);
    (
        format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z"),
        format!("{y:04}{m:02}{d:02}T{hh:02}{mm:02}{ss:02}Z"),
    )
}

impl FrameStats {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, frame_ms: f64, main_ms: f64, render_cpu_ms: Option<f64>, gpu_ms: Option<f64>) {
        self.rows.push([Some(frame_ms), Some(main_ms), render_cpu_ms, gpu_ms]);
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    fn col(&self, i: usize) -> Vec<f64> {
        self.rows.iter().filter_map(|r| r[i]).collect()
    }

    /// True if any frame took more than `SPIKE_FACTOR` x the median frame time.
    pub fn has_spike(&self) -> bool {
        let mut f = self.col(0);
        f.sort_by(|a, b| a.total_cmp(b));
        let med = median(&f);
        med > 0.0 && f.last().is_some_and(|&m| m > SPIKE_FACTOR * med)
    }

    /// All disturbances: those noted by the runner plus "spike" and "wrong_size" derived here.
    fn all_disturbances(&self, meta: &RunMeta) -> Disturbances {
        let mut d = meta.disturbances.clone();
        if self.has_spike() {
            d.note("spike");
        }
        if meta.window_px != (WINDOW_W as u32, WINDOW_H as u32) {
            d.note("wrong_size");
        }
        d
    }

    /// Compute statistics only (no I/O).
    pub fn summarize(&self, measure_secs: f64) -> Summary {
        let mut f = self.col(0);
        f.sort_by(|a, b| a.total_cmp(b));
        let med = median(&f);
        Summary {
            frames: self.rows.len(),
            fps_mean: if measure_secs > 0.0 { self.rows.len() as f64 / measure_secs } else { 0.0 },
            frame_ms_mean: mean(&f).unwrap_or(0.0),
            frame_ms_median: med,
            frame_ms_p95: percentile(&f, 0.95),
            frame_ms_p99: percentile(&f, 0.99),
            frame_ms_max: f.last().copied().unwrap_or(0.0),
            main_ms_mean: mean(&self.col(1)).unwrap_or(0.0),
            render_cpu_ms_mean: mean(&self.col(2)),
            gpu_ms_mean: mean(&self.col(3)),
            vsync_suspect: [60.0, 75.0, 120.0, 144.0, 165.0, 240.0]
                .iter()
                .any(|r| (med - 1000.0 / r).abs() <= 0.03 * 1000.0 / r),
        }
    }

    /// Summarize, append to `results/results.csv`, write `results/frames/*.csv`, print a summary line.
    /// I/O errors are reported on stderr; the summary is returned regardless.
    pub fn finish(&self, meta: RunMeta) -> Summary {
        let s = self.summarize(meta.measure_secs);
        let (ts, ts_file) = timestamps();
        let sc = meta.scene;
        let dist = self.all_disturbances(&meta);
        if let Err(e) = self.write_files(&results_dir(), &s, &meta, &dist, &ts, &ts_file) {
            eprintln!("bench-core: failed to write results: {e}");
        }
        let flag = if dist.is_empty() { String::new() } else { format!("*** DISTURBED({}) *** ", dist.joined()) };
        println!(
            "{flag}[{}] {} {} on={} off={}: {} frames, {:.1} fps, frame_ms mean {:.3} med {:.3} p95 {:.3} p99 {:.3} max {:.3}, main {:.3}, render_cpu {}, gpu {}, present {}, vsync_suspect={}, refresh_hz={}, window={}",
            meta.backend, meta.bevy_version, sc.name, sc.onscreen, sc.offscreen, s.frames, s.fps_mean,
            s.frame_ms_mean, s.frame_ms_median, s.frame_ms_p95, s.frame_ms_p99, s.frame_ms_max,
            s.main_ms_mean, opt(s.render_cpu_ms_mean), opt(s.gpu_ms_mean), meta.present_mode, s.vsync_suspect,
            fmt_hz(meta.refresh_hz), fmt_px(meta.window_px)
        );
        s
    }

    fn write_files(
        &self,
        dir: &Path,
        s: &Summary,
        m: &RunMeta,
        dist: &Disturbances,
        ts: &str,
        ts_file: &str,
    ) -> std::io::Result<()> {
        let sc = m.scene;
        let (csv, rotated) = prepare_csv(dir, ts_file)?;
        if let Some(old) = rotated {
            println!("bench-core: results.csv header changed; moved old file to {}", old.display());
        }
        let mut f = fs::OpenOptions::new().append(true).open(&csv)?;
        writeln!(
            f,
            "{ts},{},{},{},{},{},{},{},{:.3},{},{:.3},{:.3},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{},{},{},{},{},{},{},{}",
            m.backend, m.bevy_version, sc.name, sc.rig_label(), sc.anim_mode.as_str(), sc.onscreen, sc.offscreen,
            m.load_secs, s.frames, m.measure_secs, s.fps_mean, s.frame_ms_mean, s.frame_ms_median,
            s.frame_ms_p95, s.frame_ms_p99, s.frame_ms_max, s.main_ms_mean, opt(s.render_cpu_ms_mean),
            opt(s.gpu_ms_mean), m.present_mode, s.vsync_suspect, !dist.is_empty(), dist.joined(),
            fmt_hz(m.refresh_hz), fmt_px(m.window_px)
        )?;
        let raw = dir.join("frames").join(format!(
            "{}_{}_{}_{}_{}.csv",
            m.backend, sc.name, sc.onscreen, sc.offscreen, ts_file
        ));
        let mut w = std::io::BufWriter::new(fs::File::create(raw)?);
        writeln!(w, "frame_ms,main_ms,render_cpu_ms,gpu_ms")?;
        for r in &self.rows {
            writeln!(w, "{},{},{},{}", opt(r[0]), opt(r[1]), opt(r[2]), opt(r[3]))?;
        }
        w.flush()
    }
}

fn fmt_hz(hz: Option<f64>) -> String {
    hz.map(|h| format!("{h:.2}")).unwrap_or_default()
}

fn fmt_px(px: (u32, u32)) -> String {
    format!("{}x{}", px.0, px.1)
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;

    fn scene(mode: AnimMode, on: u32, off: u32) -> Scene {
        Scene {
            name: "t".into(),
            rigs: vec!["ella".into()],
            animations: ["a", "b", "c", "d"].map(String::from).to_vec(),
            anim_mode: mode,
            onscreen: on,
            offscreen: off,
            scale: 0.5,
            seed: 42,
            draw_order: DrawOrder::Any,
            motion: Motion::None,
        }
    }

    const GOLDEN: [u64; 3] = [13679457532755275413, 2949826092126892291, 5139283748462763858];

    #[test]
    fn splitmix_golden() {
        let mut r = SplitMix64::new(42);
        assert_eq!(r.next_u64(), GOLDEN[0]);
        assert_eq!(r.next_u64(), GOLDEN[1]);
        assert_eq!(r.next_u64(), GOLDEN[2]);
        let f = SplitMix64::new(1).next_f64();
        assert!((0.0..1.0).contains(&f));
    }

    #[test]
    fn placement_deterministic_and_bounded() {
        let s = scene(AnimMode::Random, 1000, 500);
        let a = s.instances();
        assert_eq!(a, s.instances());
        assert_eq!(a.len(), 1500);
        assert_eq!(a.iter().filter(|i| i.onscreen).count(), 1000);
        assert!(a[..1000].iter().all(|i| i.pos[0].abs() < 960.0 && i.pos[1].abs() < 540.0));
        assert!(a[1000..].iter().all(|i| !i.onscreen && i.pos[1] > 50000.0));
        assert!(a.iter().all(|i| (0.0..1.0).contains(&i.phase_frac)));
        // Draws are mode-independent: phases match between random and same_random_phase.
        let b = scene(AnimMode::SameRandomPhase, 1000, 500).instances();
        assert!(a.iter().zip(&b).all(|(x, y)| x.phase_frac == y.phase_frac));
        assert!(b.iter().all(|i| i.anim == "a"));
        assert!(scene(AnimMode::Same, 10, 0).instances().iter().all(|i| i.phase_frac == 0.0));
    }

    #[test]
    fn grid_counts() {
        // 1000 in 1840x1000: cols = ceil(sqrt(1840)) = 43, rows = 24.
        let (g, _) = grid(1000, -920.0, 920.0, -500.0, 500.0);
        assert_eq!(g.len(), 1000);
        let xs: std::collections::HashSet<u32> = g.iter().map(|p| p[0].to_bits()).collect();
        assert_eq!(xs.len(), 43);
        assert!(grid(0, 0.0, 1.0, 0.0, 1.0).0.is_empty());
    }

    #[test]
    fn rig_paths_and_root() {
        let (j, a) = scene(AnimMode::Same, 1, 0).rig_paths().remove(0);
        assert_eq!((j.as_str(), a.as_str()), ("rigs/ella/Ella.json", "rigs/ella/Ella.atlas"));
        let mut s = scene(AnimMode::Same, 1, 0);
        s.rigs = vec!["ella".into(), "ella_b".into()];
        assert_eq!(s.rig_paths()[1].0, "rigs/ella_b/Ella_b.json");
        assert_eq!(s.rig_label(), "ella+ella_b");
        assert!(!repo_root().to_string_lossy().starts_with(r"\\?\"));
        assert!(repo_root().join("BENCHMARK_SPEC.md").exists());
    }

    #[test]
    fn loads_real_scene_with_override() {
        let args = Args::parse_from(["x", "--scene", "scenes/ella_random.toml", "--onscreen", "7"]);
        let s = Scene::load(&args.scene, &args).unwrap();
        assert_eq!((s.name.as_str(), s.onscreen, s.rig_label().as_str()), ("ella_random", 7, "ella"));
        assert_eq!((s.draw_order, s.motion), (DrawOrder::Any, Motion::None));
    }

    #[test]
    fn loads_new_scenes() {
        let load = |n: &str| {
            let args = Args::parse_from(["x", "--scene", &format!("scenes/{n}.toml")]);
            Scene::load(&args.scene, &args).unwrap()
        };
        let s = load("mixed3_random_ysort_bob");
        assert_eq!(s.rigs, ["ella", "ella_b", "ella_c"]);
        assert_eq!((s.draw_order, s.motion), (DrawOrder::YSorted, Motion::Bob));
        let s = load("ella_random_bob");
        assert_eq!((s.rigs.len(), s.draw_order, s.motion), (1, DrawOrder::Any, Motion::Bob));
        // Exactly one of rig / rigs
        let dir = std::env::temp_dir().join("bench_core_scene_test");
        fs::create_dir_all(&dir).unwrap();
        let base = "animations = [\"a\"]
anim_mode = \"same\"
onscreen = 1
scale = 1.0
seed = 1
";
        for (name, extra, ok) in [
            ("both", "rig = \"x\"
rigs = [\"y\"]
", false),
            ("none", "", false),
            ("empty", "rigs = []
", false),
            ("rigs", "rigs = [\"x\", \"y\"]
", true),
        ] {
            let path = dir.join(format!("{name}.toml"));
            fs::write(&path, format!("{extra}{base}")).unwrap();
            let args = Args::parse_from(["x", "--scene", path.to_str().unwrap()]);
            assert_eq!(Scene::load(&path, &args).is_ok(), ok, "{name}");
        }
    }

    #[test]
    fn single_rig_no_motion_unchanged() {
        // Placement/animation come from the same RNG stream as before the draw-order keys existed.
        let s = scene(AnimMode::Random, 50, 5);
        let mut rng = SplitMix64::new(42);
        let (pts, _) = grid(50, -920.0, 920.0, -500.0, 500.0);
        for (i, inst) in s.instances().iter().take(50).enumerate() {
            let a = rng.next_u64();
            let p = rng.next_f64();
            assert_eq!(inst.pos, [pts[i][0], pts[i][1], 0.0]);
            assert_eq!(inst.anim, s.animations[(a % 4) as usize]);
            assert_eq!(inst.phase_frac, p as f32);
            assert_eq!(inst.rig, 0);
            assert_eq!((inst.bob_amp, inst.bob_phase), (0.0, 0.0));
            assert_eq!(inst.pos_at(1.234), inst.pos);
            assert_eq!(inst.sort_y(7.0), inst.pos[1]);
        }
        // Adding rigs/motion changes neither placement nor animation.
        let mut m = scene(AnimMode::Random, 50, 5);
        m.rigs = vec!["ella".into(), "ella_b".into(), "ella_c".into()];
        m.motion = Motion::Bob;
        for (x, y) in s.instances().iter().zip(m.instances()) {
            assert_eq!((&x.anim, x.pos, x.phase_frac), (&y.anim, y.pos, y.phase_frac));
        }
    }

    #[test]
    fn rig_assignment_from_a() {
        let mut s = scene(AnimMode::Random, 2000, 0);
        s.rigs = vec!["ella".into(), "ella_b".into(), "ella_c".into()];
        let inst = s.instances();
        let mut rng = SplitMix64::new(42);
        let mut counts = [0usize; 3];
        for i in &inst {
            let a = rng.next_u64();
            rng.next_f64();
            assert_eq!(i.rig, ((a >> 32) % 3) as usize);
            counts[i.rig] += 1;
        }
        assert!(counts.iter().all(|&c| c > 500), "{counts:?}");
    }

    #[test]
    fn y_sorted_z_mapping() {
        assert!(y_sorted_z(-300.0) > y_sorted_z(-299.0));
        assert!(y_sorted_z(0.0) > y_sorted_z(1.0));
        assert_eq!(y_sorted_z(100000.0), -500.0);
        assert_eq!(y_sorted_z(0.0), 0.0);
        // A whole skeleton (18 slots * 0.001) fits between rows 1 px apart.
        assert!(y_sorted_z(10.0) - y_sorted_z(11.0) > 18.0 * 0.001);
    }

    #[test]
    fn bob_position() {
        let mut s = scene(AnimMode::Random, 1000, 10);
        s.motion = Motion::Bob;
        let inst = s.instances();
        let (_, cell_h) = grid(1000, -920.0, 920.0, -500.0, 500.0);
        for i in inst.iter().filter(|i| i.onscreen) {
            assert_eq!(i.bob_amp, cell_h);
            assert!((0.0..1.0).contains(&i.bob_phase));
            // x fixed, y within base +- amp, periodic in 4 s
            for t in [0.0, 0.7, 1.9, 3.3, 10.1] {
                let p = i.pos_at(t);
                assert_eq!((p[0], p[2]), (i.pos[0], 0.0));
                assert!((p[1] - i.pos[1]).abs() <= cell_h * 1.0001);
                assert!((i.pos_at(t + BOB_PERIOD_SECS)[1] - p[1]).abs() < 1e-2);
            }
            // exact formula
            let want = i.pos[1] as f64 + cell_h as f64 * (std::f64::consts::TAU * (0.25 + i.bob_phase as f64)).sin();
            assert!((i.pos_at(1.0)[1] as f64 - want).abs() < 1e-3);
            assert_eq!(i.sort_y(1.0), i.pos_at(1.0)[1]);
        }
        // Offscreen instances bob too (with their own grid's cell height).
        assert!(inst.iter().filter(|i| !i.onscreen).all(|i| i.bob_amp > 0.0));
        // The phases are spread out.
        let distinct: std::collections::HashSet<u32> = inst.iter().map(|i| i.bob_phase.to_bits()).collect();
        assert!(distinct.len() > 500);
    }

    #[test]
    fn stats_known_vector() {
        let mut fs = FrameStats::new();
        for i in 1..=100 {
            fs.push(i as f64, 1.0, if i > 50 { Some(2.0) } else { None }, None);
        }
        let s = fs.summarize(10.0);
        assert_eq!(s.frames, 100);
        assert_eq!(s.fps_mean, 10.0);
        assert_eq!(s.frame_ms_mean, 50.5);
        assert_eq!(s.frame_ms_median, 50.5);
        assert_eq!((s.frame_ms_p95, s.frame_ms_p99, s.frame_ms_max), (95.0, 99.0, 100.0));
        assert_eq!(s.render_cpu_ms_mean, Some(2.0));
        assert_eq!(s.gpu_ms_mean, None);
        assert!(!s.vsync_suspect);
        let mut v = FrameStats::new();
        (0..10).for_each(|_| v.push(6.94, 1.0, None, None)); // ~144 Hz
        assert!(v.summarize(1.0).vsync_suspect);
    }

    #[test]
    fn clock_phases() {
        let args = Args::parse_from(["x", "--scene", "s.toml", "--warmup", "1", "--duration", "2"]);
        let t0 = Instant::now();
        let at = |s: f64| t0 + std::time::Duration::from_secs_f64(s);
        let mut c = BenchClock::new(t0, &args);
        assert_eq!(c.update(at(1.0), false), Phase::Loading);
        assert_eq!(c.update(at(4.0), true), Phase::Warmup);
        assert_eq!(c.load_secs, 4.0);
        assert_eq!(c.update(at(4.5), true), Phase::Warmup);
        assert_eq!(c.update(at(5.0), true), Phase::Measure);
        assert_eq!(c.update(at(7.0), true), Phase::Screenshot);
        assert_eq!(c.measure_secs, 2.0);
        c.screenshot_done();
        assert_eq!(c.phase, Phase::Done);
    }

    fn meta<'a>(sc: &'a Scene, d: &'a Disturbances, px: (u32, u32)) -> RunMeta<'a> {
        RunMeta {
            backend: "t",
            bevy_version: "0",
            scene: sc,
            load_secs: 1.0,
            measure_secs: 1.0,
            present_mode: "X".into(),
            disturbances: d,
            refresh_hz: Some(144.0),
            window_px: px,
        }
    }

    #[test]
    fn disturbances_dedup() {
        let mut d = Disturbances::new();
        assert!(d.is_empty());
        d.note("focus_lost");
        d.note("resized");
        d.note("focus_lost");
        assert_eq!(d.reasons().len(), 2);
        assert_eq!(d.joined(), "focus_lost;resized");
    }

    #[test]
    fn spike_detection_and_wrong_size() {
        let sc = scene(AnimMode::Same, 1, 0);
        let none = Disturbances::new();
        let mut ok = FrameStats::new();
        (0..100).for_each(|_| ok.push(5.0, 1.0, None, None));
        assert!(!ok.has_spike());
        assert!(ok.all_disturbances(&meta(&sc, &none, (1920, 1080))).is_empty());
        let mut bad = FrameStats::new();
        (0..100).for_each(|_| bad.push(5.0, 1.0, None, None));
        bad.push(101.0, 1.0, None, None); // > 20 x 5
        assert!(bad.has_spike());
        let d = bad.all_disturbances(&meta(&sc, &none, (1280, 720)));
        assert_eq!(d.joined(), "spike;wrong_size");
        let mut edge = FrameStats::new();
        (0..100).for_each(|_| edge.push(5.0, 1.0, None, None));
        edge.push(100.0, 1.0, None, None); // exactly 20 x: not a spike
        assert!(!edge.has_spike());
    }

    #[test]
    fn csv_header_rotation() {
        let dir = std::env::temp_dir().join(format!("bench-core-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        // fresh
        let (csv, rot) = prepare_csv(&dir, "T1").unwrap();
        assert!(rot.is_none());
        assert_eq!(fs::read_to_string(&csv).unwrap().trim_end(), HEADER);
        // same header: kept, rows preserved
        fs::OpenOptions::new().append(true).open(&csv).unwrap().write_all(b"row\n").unwrap();
        let (_, rot) = prepare_csv(&dir, "T2").unwrap();
        assert!(rot.is_none());
        assert!(fs::read_to_string(&csv).unwrap().contains("row"));
        // old header: rotated
        fs::write(&csv, "timestamp,backend\nold\n").unwrap();
        let (csv, rot) = prepare_csv(&dir, "T3").unwrap();
        let old = rot.unwrap();
        assert!(old.ends_with("results.csv.old-T3"));
        assert_eq!(fs::read_to_string(old).unwrap(), "timestamp,backend\nold\n");
        assert_eq!(fs::read_to_string(&csv).unwrap().trim_end(), HEADER);
        // full write path
        let sc = scene(AnimMode::Same, 1, 0);
        let mut d = Disturbances::new();
        d.note("focus_lost");
        let mut st = FrameStats::new();
        st.push(5.0, 1.0, None, None);
        let m = meta(&sc, &d, (1920, 1080));
        let s = st.summarize(1.0);
        st.write_files(&dir, &s, &m, &d, "2026-01-01T00:00:00Z", "T4").unwrap();
        let text = fs::read_to_string(dir.join("results.csv")).unwrap();
        let row = text.lines().nth(1).unwrap();
        assert!(row.ends_with(",true,focus_lost,144.00,1920x1080"), "{row}");
        assert_eq!(row.split(',').count(), HEADER.split(',').count());
        let _ = fs::remove_dir_all(&dir);
    }
}
