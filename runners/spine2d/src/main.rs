//! Benchmark runner for Latias94/spine2d (spine2d-bevy) on Bevy 0.19. See README.md and BENCHMARK_SPEC.md.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use bench_core::{
    Args, BenchClock, Disturbances, DrawOrder, FrameStats, InstanceSpec, Motion, Phase, RunMeta, Scene, WINDOW_H,
    WINDOW_W,
};
use bevy::app::AppExit;
use bevy::diagnostic::DiagnosticsStore;
use bevy::input::mouse::MouseWheel;
use bevy::prelude::*;
use bevy::render::diagnostic::RenderDiagnosticsPlugin;
use bevy::render::view::screenshot::{save_to_disk, Screenshot, ScreenshotCaptured};
use bevy::render::{Render, RenderApp, RenderSystems};
use bevy::window::{
    Monitor, OnMonitor, PresentMode, PrimaryWindow, WindowBackendScaleFactorChanged, WindowFocused, WindowMoved,
    WindowOccluded, WindowResized, WindowResolution, WindowScaleFactorChanged,
};
use bevy::winit::WinitSettings;
use spine2d_bevy::{
    Spine, Spine2dPlugin, SpineAnimationCommand, SpineAtlasAsset, SpineReady, SpineSkeletonAsset, SpineSystemSet,
    SpineTrackEntrySettings,
};

const BACKEND: &str = "spine2d";

/// Cargo.lock is embedded to record the exact resolved bevy version.
fn bevy_version() -> String {
    let lock = include_str!("../Cargo.lock");
    let mut lines = lock.lines();
    while let Some(l) = lines.next() {
        if l == "name = \"bevy\"" {
            if let Some(v) = lines.next().and_then(|v| v.strip_prefix("version = \"")) {
                return v.trim_end_matches('"').to_string();
            }
        }
    }
    "unknown".into()
}

// ------------------------------------------------------------ shared render-thread timing

/// Written by the render sub-app, read by the main world. `epoch` is a common Instant.
#[derive(Resource, Clone)]
struct RenderTiming {
    epoch: Instant,
    start_ns: Arc<AtomicU64>,
    /// f64 bits of the most recently completed render-world frame duration in ms.
    last_ms: Arc<AtomicU64>,
}

fn render_begin(t: Res<RenderTiming>) {
    t.start_ns.store(t.epoch.elapsed().as_nanos() as u64, Ordering::Release);
}

fn render_end(t: Res<RenderTiming>) {
    let start = t.start_ns.load(Ordering::Acquire);
    let now = t.epoch.elapsed().as_nanos() as u64;
    let ms = now.saturating_sub(start) as f64 / 1e6;
    t.last_ms.store(ms.to_bits(), Ordering::Release);
}

// ------------------------------------------------------------ main-world state

#[derive(Resource)]
#[allow(dead_code)]
struct Bench {
    args: Args,
    scene: Scene,
    specs: Vec<InstanceSpec>,
    clock: BenchClock,
    stats: FrameStats,
    total: usize,
    started: usize,
    /// Instant at start of `First` for the current frame.
    first_start: Instant,
    prev_first: Option<Instant>,
    frame_ms: f64,
    phase: Phase,
    screenshot_requested: bool,
    screenshot_flag: Arc<AtomicBool>,
    present: String,
    finished: bool,
    dist: Disturbances,
    refresh_hz: Option<f64>,
    window_px: (u32, u32),
}

#[derive(Component)]
struct Inst {
    /// Index into `Bench::specs`.
    idx: usize,
    anim: String,
    phase_frac: f32,
}

#[derive(Component)]
struct Started;

#[derive(Resource, Default)]
struct RigHandles {
    /// One (skeleton, atlas) per rig of the scene.
    rigs: Vec<(Handle<SpineSkeletonAsset>, Handle<SpineAtlasAsset>)>,
}

/// Asset root: `<repo>/assets-spine43`, a copy of `assets/` whose `skeleton.spine` version string is rewritten to
/// `4.3.0` (spine2d rejects 4.2 version strings; regenerate with `tools/make-spine43-assets.sh`).
/// `SPINE2D_ASSET_ROOT` overrides it.
fn asset_root() -> String {
    match std::env::var("SPINE2D_ASSET_ROOT") {
        Ok(p) => {
            eprintln!("[spine2d] WARNING: asset root overridden to {p}");
            p
        }
        Err(_) => bench_core::repo_root().join("assets-spine43").to_string_lossy().into_owned(),
    }
}

fn main() {
    let app_start = Instant::now();
    let args = Args::parse_args();
    let scene = match Scene::load(&args.scene, &args) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    let view = args.view;
    let present_mode = if view { PresentMode::AutoVsync } else { PresentMode::AutoNoVsync };
    let present = format!("{present_mode:?}");
    println!(
        "[spine2d] scene={} rig={} draw_order={:?} motion={:?} requesting present mode {present}",
        scene.name,
        scene.rig_label(),
        scene.draw_order,
        scene.motion
    );

    let timing = RenderTiming {
        epoch: Instant::now(),
        start_ns: Arc::new(AtomicU64::new(0)),
        last_ms: Arc::new(AtomicU64::new(f64::NAN.to_bits())),
    };

    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(AssetPlugin { file_path: asset_root(), ..default() })
            .set(WindowPlugin {
                primary_window: Some(Window {
                    title: "spine2d bench".into(),
                    resolution: WindowResolution::new(WINDOW_W as u32, WINDOW_H as u32).with_scale_factor_override(1.0),
                    resizable: false,
                    present_mode,
                    ..default()
                }),
                ..default()
            }),
    )
    .add_plugins(Spine2dPlugin)
    .insert_resource(WinitSettings::continuous())
    .insert_resource(timing.clone())
    .init_resource::<RigHandles>();

    if !view {
        app.add_plugins(RenderDiagnosticsPlugin);
    }

    if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
        render_app.insert_resource(timing.clone());
        render_app.add_systems(Render, render_begin.before(RenderSystems::ExtractCommands));
        render_app.add_systems(Render, render_end.after(RenderSystems::PostCleanup));
    }

    let specs = scene.instances();
    let total = specs.len();
    app.insert_resource(Bench {
        clock: BenchClock::new(app_start, &args),
        stats: FrameStats::new(),
        total,
        specs,
        started: 0,
        first_start: app_start,
        prev_first: None,
        frame_ms: 0.0,
        phase: Phase::Loading,
        screenshot_requested: false,
        screenshot_flag: Arc::new(AtomicBool::new(false)),
        present,
        finished: false,
        dist: Disturbances::new(),
        refresh_hz: None,
        window_px: (0, 0),
        args,
        scene,
    });

    app.add_systems(Startup, setup);
    app.add_systems(Update, (watch_window, update_motion));
    app.add_systems(
        Update,
        (start_animations.after(SpineSystemSet::Spawn).before(SpineSystemSet::Commands), check_load_failure),
    );
    if view {
        app.add_systems(Update, (view_controls, view_title));
    } else {
        app.add_systems(First, first_stamp.before(bevy::time::TimeSystems));
        app.add_systems(Last, last_stamp);
    }
    app.run();
}

fn setup(mut commands: Commands, asset_server: Res<AssetServer>, bench: Res<Bench>, mut rig: ResMut<RigHandles>) {
    commands.spawn(Camera2d);
    rig.rigs = bench
        .scene
        .rig_paths()
        .into_iter()
        .map(|(json, atlas)| (asset_server.load(json), asset_server.load(atlas)))
        .collect();
    for inst in &bench.specs {
        let (skeleton, atlas) = rig.rigs[inst.rig].clone();
        commands.spawn((
            // Default skin, no animation yet: animation + track time are applied via SpineAnimationCommand once ready.
            Spine::new(skeleton, atlas),
            instance_transform(&bench.scene, inst, 0.0),
            Inst { idx: inst.index, anim: inst.anim.clone(), phase_frac: inst.phase_frac },
        ));
    }
}

/// Position at time `t`; for `y_sorted` scenes z encodes the order (lower feet y = larger z = in front).
/// The mesh children are offset by 0.001 z per draw (scaled by the skeleton scale), which
/// `bench_core::y_sorted_z` keeps well below the z gap between skeletons 1 px apart in y.
fn instance_transform(scene: &Scene, s: &InstanceSpec, t: f64) -> Transform {
    let p = s.pos_at(t);
    let z = if scene.draw_order == DrawOrder::YSorted { bench_core::y_sorted_z(s.sort_y(t)) } else { p[2] };
    Transform::from_xyz(p[0], p[1], z).with_scale(Vec3::splat(s.scale))
}

/// `bob` scenes: moves every instance (and its z, for `y_sorted`) each frame.
fn update_motion(bench: Res<Bench>, mut q: Query<(&Inst, &mut Transform)>) {
    if bench.scene.motion != Motion::Bob {
        return;
    }
    let t = bench.clock.elapsed_secs(Instant::now());
    for (inst, mut transform) in &mut q {
        *transform = instance_transform(&bench.scene, &bench.specs[inst.idx], t);
    }
}

fn check_load_failure(asset_server: Res<AssetServer>, rig: Res<RigHandles>) {
    use bevy::asset::LoadState;
    for (skeleton, atlas) in &rig.rigs {
        if let LoadState::Failed(e) = asset_server.load_state(skeleton) {
            eprintln!("FATAL: skeleton failed to load: {e}");
            std::process::exit(3);
        }
        if let LoadState::Failed(e) = asset_server.load_state(atlas) {
            eprintln!("FATAL: atlas failed to load: {e}");
            std::process::exit(3);
        }
    }
}

fn start_animations(
    mut commands: Commands,
    mut bench: ResMut<Bench>,
    skeletons: Res<Assets<SpineSkeletonAsset>>,
    q: Query<(Entity, &Spine, &Inst), (With<SpineReady>, Without<Started>)>,
    mut writer: MessageWriter<SpineAnimationCommand>,
) {
    for (e, spine, inst) in &q {
        let Some(asset) = skeletons.get(spine.get_skeleton_handle()) else { continue };
        let Some(anim) = asset.get_data().find_animation(&inst.anim) else {
            eprintln!("FATAL: animation '{}' not found in rig", inst.anim);
            std::process::exit(4);
        };
        let t = inst.phase_frac * anim.get_duration();
        writer.write(
            SpineAnimationCommand::set_animation(e, 0, inst.anim.clone(), true)
                .with_entry_settings(SpineTrackEntrySettings::new().with_track_time(t)),
        );
        commands.entity(e).insert(Started);
        bench.started += 1;
    }
}

/// Notes window disturbances (KVM switch, focus steal, resize, ...) while warming up / measuring, and records
/// window size + monitor refresh rate at measure start.
#[allow(clippy::too_many_arguments)]
fn watch_window(
    mut bench: ResMut<Bench>,
    mut focused: MessageReader<WindowFocused>,
    mut occluded: MessageReader<WindowOccluded>,
    mut resized: MessageReader<WindowResized>,
    mut scale: MessageReader<WindowScaleFactorChanged>,
    mut backend_scale: MessageReader<WindowBackendScaleFactorChanged>,
    mut moved: MessageReader<WindowMoved>,
    windows: Query<(&Window, Option<&OnMonitor>), With<PrimaryWindow>>,
    monitors: Query<&Monitor>,
    mut started: Local<bool>,
) {
    let phase = bench.clock.phase;
    let active = matches!(phase, Phase::Warmup | Phase::Measure);
    let hits = [
        ("focus_lost", focused.read().filter(|e| !e.focused).count()),
        ("occluded", occluded.read().filter(|e| e.occluded).count()),
        ("resized", resized.read().count()),
        ("scale_changed", scale.read().count() + backend_scale.read().count()),
        ("moved", moved.read().count()),
    ];
    if !active {
        return;
    }
    for (reason, n) in hits {
        if n > 0 {
            bench.dist.note(reason);
        }
    }
    if phase == Phase::Measure && !*started {
        *started = true;
        if let Ok((w, on)) = windows.single() {
            if !w.focused {
                bench.dist.note("unfocused_at_start");
            }
            bench.window_px = (w.physical_width(), w.physical_height());
            bench.refresh_hz = on
                .and_then(|m| monitors.get(m.0).ok())
                .and_then(|m| m.refresh_rate_millihertz)
                .map(|mhz| mhz as f64 / 1000.0);
        }
    }
}

// ------------------------------------------------------------ bench-mode systems

fn first_stamp(mut b: ResMut<Bench>) {
    let now = Instant::now();
    b.first_start = now;
    b.frame_ms = b.prev_first.map(|p| now.duration_since(p).as_secs_f64() * 1000.0).unwrap_or(0.0);
    b.prev_first = Some(now);
    let ready = b.started >= b.total;
    let b = &mut *b;
    b.phase = b.clock.update(now, ready);
}

#[allow(clippy::too_many_arguments)]
fn last_stamp(
    mut commands: Commands,
    mut b: ResMut<Bench>,
    timing: Res<RenderTiming>,
    diags: Res<DiagnosticsStore>,
    mut exit: MessageWriter<AppExit>,
    mut logged: Local<bool>,
) {
    let main_ms = b.first_start.elapsed().as_secs_f64() * 1000.0;
    match b.phase {
        Phase::Measure => {
            let r = f64::from_bits(timing.last_ms.load(Ordering::Acquire));
            let render = (!r.is_nan()).then_some(r);
            let gpu = gpu_ms(&diags, &mut logged);
            let f = b.frame_ms;
            b.stats.push(f, main_ms, render, gpu);
        }
        Phase::Screenshot => {
            if !b.screenshot_requested {
                b.screenshot_requested = true;
                let path = bench_core::screenshot_path(BACKEND, &b.scene);
                let _ = std::fs::remove_file(&path);
                let flag = b.screenshot_flag.clone();
                commands
                    .spawn(Screenshot::primary_window())
                    .observe(save_to_disk(path))
                    // Runs after save_to_disk (observers run in registration order); the save is synchronous.
                    .observe(move |_: On<ScreenshotCaptured>| flag.store(true, Ordering::Release));
            } else if b.screenshot_flag.load(Ordering::Acquire) {
                b.clock.screenshot_done();
            }
        }
        Phase::Done if !b.finished => {
            b.finished = true;
            let b = &*b;
            b.stats.finish(RunMeta {
                backend: BACKEND,
                bevy_version: &bevy_version(),
                scene: &b.scene,
                load_secs: b.clock.load_secs,
                measure_secs: b.clock.measure_secs,
                present_mode: b.present.clone(),
                disturbances: &b.dist,
                refresh_hz: b.refresh_hz,
                window_px: b.window_px,
            });
            exit.write(AppExit::Success);
        }
        _ => {}
    }
}

/// Sum of GPU pass times from RenderDiagnosticsPlugin: all `render/<span>/elapsed_gpu` diagnostics whose path has
/// exactly 3 components (top level spans only, so nested spans are not double counted). None if the adapter has
/// no timestamp support (no such diagnostics appear).
fn gpu_ms(diags: &DiagnosticsStore, logged: &mut bool) -> Option<f64> {
    let mut sum = 0.0;
    let mut any = false;
    for d in diags.iter() {
        let p = d.path().as_str();
        if p.starts_with("render/") && p.ends_with("/elapsed_gpu") {
            if std::env::var_os("SPINE2D_BENCH_LOG_GPU").is_some() && !*logged {
                println!("gpu diagnostic: {p} = {:?}", d.value());
            }
            if p.matches('/').count() == 2 {
                if let Some(v) = d.value() {
                    sum += v;
                    any = true;
                }
            }
        }
    }
    *logged = true;
    any.then_some(sum)
}

// ------------------------------------------------------------ view mode

fn view_controls(
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    mut wheel: MessageReader<MouseWheel>,
    mut cam: Single<(&mut Transform, &mut Projection), With<Camera2d>>,
) {
    let (tf, proj) = &mut *cam;
    let Projection::Orthographic(o) = &mut **proj else { return };
    let mut d = Vec2::ZERO;
    if keys.pressed(KeyCode::KeyW) || keys.pressed(KeyCode::ArrowUp) {
        d.y += 1.0;
    }
    if keys.pressed(KeyCode::KeyS) || keys.pressed(KeyCode::ArrowDown) {
        d.y -= 1.0;
    }
    if keys.pressed(KeyCode::KeyA) || keys.pressed(KeyCode::ArrowLeft) {
        d.x -= 1.0;
    }
    if keys.pressed(KeyCode::KeyD) || keys.pressed(KeyCode::ArrowRight) {
        d.x += 1.0;
    }
    let v = d * 600.0 * o.scale * time.delta_secs();
    tf.translation += v.extend(0.0);
    for ev in wheel.read() {
        o.scale = (o.scale * 0.9f32.powf(ev.y)).clamp(0.05, 50.0);
    }
}

fn view_title(
    time: Res<Time<Real>>,
    mut window: Single<&mut Window, With<PrimaryWindow>>,
    mut acc: Local<(f32, u32)>,
) {
    acc.0 += time.delta_secs();
    acc.1 += 1;
    if acc.0 >= 0.5 {
        let ms = acc.0 * 1000.0 / acc.1 as f32;
        window.title = format!("spine2d view - {:.1} FPS, {:.2} ms", 1000.0 / ms, ms);
        *acc = (0.0, 0);
    }
}
