//! Benchmark runner for the `bevy_spine_gpu` crate (Bevy 0.14). See `BENCHMARK_SPEC.md`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use bench_core::{Args, BenchClock, Disturbances, DrawOrder, FrameStats, InstanceSpec, Motion, Phase, RunMeta, Scene};
use bevy::app::AppExit;
use bevy::input::mouse::MouseWheel;
use bevy::render::view::screenshot::ScreenshotManager;
use bevy::render::{Render, RenderApp, RenderSet};
use bevy::window::{
    PresentMode, PrimaryWindow, WindowBackendScaleFactorChanged, WindowFocused, WindowMoved, WindowOccluded,
    WindowResized, WindowResolution, WindowScaleFactorChanged,
};
use bevy::winit::{UpdateMode, WinitSettings};
use bevy::{prelude::*, time::TimeSystem};
use bevy::asset::LoadState;
use bevy_spine_gpu::{
    AnimationControl, AnimationPlayback, LoadSpine, LoadSpineSettings, SkeletonInstance, SpineGPUPlugin, SpineSkeleton,
};

const BACKEND: &str = "spine-gpu";
const CARGO_LOCK: &str = include_str!("../Cargo.lock");

/// Resolved bevy version, parsed from Cargo.lock at compile time.
fn bevy_version() -> String {
    let mut lines = CARGO_LOCK.lines();
    while let Some(l) = lines.next() {
        if l == "name = \"bevy\"" {
            if let Some(v) = lines.next().and_then(|v| v.strip_prefix("version = \"")) {
                return v.trim_end_matches('"').to_string();
            }
        }
    }
    "0.14.x".into()
}

/// Render-world CPU time of the last completed render frame, in nanoseconds.
#[derive(Resource, Clone, Default)]
struct RenderCpuNanos(Arc<AtomicU64>);

#[derive(Resource)]
struct RenderStart(Instant);

fn render_start(mut commands: Commands) {
    commands.insert_resource(RenderStart(Instant::now()));
}

fn render_end(start: Option<Res<RenderStart>>, out: Res<RenderCpuNanos>) {
    if let Some(start) = start {
        out.0.store(start.0.elapsed().as_nanos() as u64, Ordering::Relaxed);
    }
}

#[derive(Component)]
struct BenchInstance(usize);

#[derive(Resource)]
struct Bench {
    args: Args,
    scene: Scene,
    specs: Vec<InstanceSpec>,
    clock: BenchClock,
    stats: FrameStats,
    present_mode: PresentMode,
    configured: usize,
    /// Start of the current frame (start of `First`).
    frame_start: Instant,
    last_frame_start: Option<Instant>,
    frame_ms: f64,
    // screenshot state
    shot_requested: bool,
    shot_seen_frames: u32,
    // view mode title
    title_acc: f64,
    title_frames: u32,
    dist: Disturbances,
    refresh_hz: Option<f64>,
    window_px: (u32, u32),
}

fn main() {
    let start = Instant::now();
    let args = Args::parse_args();
    let scene = match Scene::load(&args.scene, &args) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(2);
        }
    };
    // spine-gpu loads .json/.atlas/texture through the AssetServer, i.e. from AssetPlugin.file_path below.
    let assets = bench_core::assets_dir();

    let present_mode = if args.view { PresentMode::AutoVsync } else { PresentMode::AutoNoVsync };
    let specs = scene.instances();
    println!(
        "[{BACKEND}] scene={} rig={} on={} off={} draw_order={:?} motion={:?} present_mode={:?} bevy={}",
        scene.name,
        scene.rig_label(),
        scene.onscreen,
        scene.offscreen,
        scene.draw_order,
        scene.motion,
        present_mode,
        bevy_version()
    );

    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(WindowPlugin {
                primary_window: Some(Window {
                    title: "spine-gpu bench".into(),
                    resolution: WindowResolution::new(bench_core::WINDOW_W, bench_core::WINDOW_H)
                        .with_scale_factor_override(1.0),
                    resizable: false,
                    present_mode,
                    ..default()
                }),
                ..default()
            })
            .set(AssetPlugin { file_path: assets.to_string_lossy().into_owned(), ..default() }),
    )
    .add_plugins(SpineGPUPlugin::default())
    .insert_resource(WinitSettings {
        focused_mode: UpdateMode::Continuous,
        unfocused_mode: UpdateMode::Continuous,
    })
    .insert_resource(Bench {
        clock: BenchClock::new(start, &args),
        args: args.clone(),
        scene,
        specs,
        stats: FrameStats::new(),
        present_mode,
        configured: 0,
        frame_start: start,
        last_frame_start: None,
        frame_ms: 0.0,
        shot_requested: false,
        shot_seen_frames: 0,
        title_acc: 0.0,
        title_frames: 0,
        dist: Disturbances::new(),
        refresh_hz: None,
        window_px: (0, 0),
    })
    .add_systems(Startup, setup)
    .add_systems(First, frame_start.before(TimeSystem))
    .add_systems(Update, (update_motion, init_sort_keys, configure_instances, watch_window, check_load_failures))
    .add_systems(Last, frame_end);

    if args.view {
        app.add_systems(Update, view_controls);
    }

    // Render-world CPU time: first/last RenderSet of the Render schedule.
    let render_cpu = RenderCpuNanos::default();
    app.insert_resource(render_cpu.clone());
    if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
        render_app
            .insert_resource(render_cpu)
            .add_systems(Render, render_start.in_set(RenderSet::ExtractCommands))
            .add_systems(Render, render_end.in_set(RenderSet::Cleanup));
    }

    app.run();
}

/// Handles of the scene's skeleton types (same assets spine-gpu requests for `LoadSpine::SpineDirectory(base, None)`),
/// only used to notice failed loads.
#[derive(Resource)]
struct RigHandles(Vec<(String, Handle<SpineSkeleton>)>);

fn setup(mut commands: Commands, bench: Res<Bench>, asset_server: Res<AssetServer>) {
    // SpineDirectory takes the path without extension: it loads "<base>.json" (+ ".atlas" sibling) itself.
    let bases: Vec<String> =
        bench.scene.rig_paths().into_iter().map(|(json, _)| json.trim_end_matches(".json").to_string()).collect();
    commands.insert_resource(RigHandles(
        bases.iter().map(|b| (b.clone(), asset_server.load::<SpineSkeleton>(format!("{b}.json")))).collect(),
    ));
    // y_sorted: every rig is loaded into the sorted group (a property of the skeleton type); otherwise the static group.
    let sorted = bench.scene.draw_order == DrawOrder::YSorted;
    for spec in &bench.specs {
        commands.spawn((
            BenchInstance(spec.index),
            LoadSpine::spine_directory(bases[spec.rig].clone()),
            LoadSpineSettings {
                animation: spec.anim.clone(),
                transition_duration: 0.0,
                sorted,
                ..default()
            },
            SpatialBundle {
                transform: Transform::from_translation(Vec3::from(spec.pos_at(0.0)))
                    .with_scale(Vec3::splat(spec.scale)),
                ..default()
            },
        ));
    }
    commands.spawn(Camera2dBundle::default());
}

/// `bob` scenes: moves every instance and (for `y_sorted`) sets `SkeletonInstance::sort_key = -y` (higher = in front).
/// Runs in `Update`, so spine-gpu's PostUpdate systems see the new transforms and keys in the same frame.
fn update_motion(bench: Res<Bench>, mut q: Query<(&BenchInstance, &mut Transform, Option<&mut SkeletonInstance>)>) {
    if bench.scene.motion != Motion::Bob {
        return;
    }
    let sorted = bench.scene.draw_order == DrawOrder::YSorted;
    // --freeze holds bob instances at their t = 0 position too.
    let t = if bench.args.freeze { 0.0 } else { bench.clock.elapsed_secs(Instant::now()) };
    for (inst, mut transform, skeleton) in &mut q {
        let spec = &bench.specs[inst.0];
        transform.translation = Vec3::from(spec.pos_at(t));
        if let (true, Some(mut skeleton)) = (sorted, skeleton) {
            skeleton.sort_key = -spec.sort_y(t);
        }
    }
}

/// Static `y_sorted` scenes: set the sort key once, when the `SkeletonInstance` appears after loading.
fn init_sort_keys(bench: Res<Bench>, mut q: Query<(&BenchInstance, &mut SkeletonInstance), Added<SkeletonInstance>>) {
    if bench.scene.draw_order != DrawOrder::YSorted || bench.scene.motion != Motion::None {
        return;
    }
    for (inst, mut skeleton) in &mut q {
        skeleton.sort_key = -bench.specs[inst.0].sort_y(0.0);
    }
}

/// Once an instance is loaded (`SkeletonInstance` + `AnimationControl` + `AnimationPlayback` appear together):
/// start the animation at `phase_frac * duration` seconds (`AnimationPlayback::set_current_time_seconds`, raw
/// animation seconds). The label was applied at load via `LoadSpineSettings` (no transition); looping is the default.
fn configure_instances(
    mut q: Query<
        (&BenchInstance, &SkeletonInstance, &mut AnimationControl, &mut AnimationPlayback),
        Added<AnimationPlayback>,
    >,
    skeletons: Res<Assets<SpineSkeleton>>,
    mut bench: ResMut<Bench>,
) {
    let mut n = 0;
    for (inst, skeleton, mut control, mut playback) in &mut q {
        let spec = &bench.specs[inst.0];
        match skeleton.info(&skeletons).and_then(|info| info.animation_duration(&spec.anim)) {
            Some(d) => playback.set_current_time_seconds(spec.phase_frac * d),
            None => warn_once!("animation {:?} not found in rig", spec.anim),
        }
        control.set_repeat(true);
        if bench.args.freeze {
            control.set_time_scale(0.0);
        }
        n += 1;
    }
    bench.configured += n;
}

/// spine-gpu logs failed loads and drops the request (no panic), so the run would wait in `Loading` forever.
/// Abort instead (e.g. Mika: rotated atlas regions are not supported), so no row is written.
fn check_load_failures(rigs: Res<RigHandles>, asset_server: Res<AssetServer>) {
    for (base, handle) in &rigs.0 {
        if let LoadState::Failed(e) = asset_server.load_state(handle.id()) {
            eprintln!("[{BACKEND}] error: failed to load {base}: {e}");
            std::process::exit(1);
        }
    }
}

/// Notes window disturbances (KVM switch, focus steal, resize, ...) while warming up / measuring, and records
/// window size + monitor refresh rate at measure start.
#[allow(clippy::too_many_arguments)]
fn watch_window(
    mut bench: ResMut<Bench>,
    mut focused: EventReader<WindowFocused>,
    mut occluded: EventReader<WindowOccluded>,
    mut resized: EventReader<WindowResized>,
    mut scale: EventReader<WindowScaleFactorChanged>,
    mut backend_scale: EventReader<WindowBackendScaleFactorChanged>,
    mut moved: EventReader<WindowMoved>,
    windows: Query<&Window, With<PrimaryWindow>>,
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
        // Bevy 0.14 exposes no monitor refresh rate, so `refresh_hz` stays None.
        if let Ok(w) = windows.get_single() {
            if !w.focused {
                bench.dist.note("unfocused_at_start");
            }
            bench.window_px = (w.physical_width(), w.physical_height());
        }
    }
}

fn frame_start(mut bench: ResMut<Bench>) {
    let now = Instant::now();
    bench.frame_ms = match bench.last_frame_start {
        Some(prev) => now.duration_since(prev).as_secs_f64() * 1000.0,
        None => 0.0,
    };
    bench.last_frame_start = Some(now);
    bench.frame_start = now;
}

fn frame_end(
    mut bench: ResMut<Bench>,
    render_cpu: Res<RenderCpuNanos>,
    mut shots: ResMut<ScreenshotManager>,
    windows: Query<Entity, With<PrimaryWindow>>,
    mut windows_mut: Query<&mut Window, With<PrimaryWindow>>,
    mut exit: EventWriter<AppExit>,
) {
    let now = Instant::now();
    let main_ms = now.duration_since(bench.frame_start).as_secs_f64() * 1000.0;
    let b = &mut *bench;

    if b.args.view {
        b.title_acc += b.frame_ms;
        b.title_frames += 1;
        if b.title_acc >= 500.0 {
            let ms = b.title_acc / b.title_frames as f64;
            let title = format!("spine-gpu | {:.1} fps | {:.2} ms", 1000.0 / ms.max(1e-6), ms);
            if let Ok(mut w) = windows_mut.get_single_mut() {
                w.title = title;
            }
            b.title_acc = 0.0;
            b.title_frames = 0;
        }
        return;
    }

    let ready = b.configured >= b.specs.len();
    let phase = b.clock.update(now, ready);
    match phase {
        Phase::Measure if b.frame_ms > 0.0 => {
            // Render time is that of the most recently completed render frame (pipelined rendering
            // overlaps render of frame N with main of frame N+1, so it lags by one frame).
            let r = render_cpu.0.load(Ordering::Relaxed);
            let render_ms = (r > 0).then(|| r as f64 / 1e6);
            b.stats.push(b.frame_ms, main_ms, render_ms, None);
        }
        Phase::Screenshot => {
            let path = bench_core::screenshot_path(BACKEND, &b.scene);
            if !b.shot_requested {
                let _ = std::fs::remove_file(&path);
                if let Ok(win) = windows.get_single() {
                    match shots.save_screenshot_to_disk(win, &path) {
                        Ok(()) => b.shot_requested = true,
                        Err(e) => {
                            eprintln!("screenshot failed: {e}");
                            b.clock.screenshot_done();
                        }
                    }
                }
            } else if path.exists() {
                // give the writer thread a few frames to finish the file
                b.shot_seen_frames += 1;
                if b.shot_seen_frames >= 5 {
                    b.clock.screenshot_done();
                }
            }
        }
        Phase::Done => {
            b.stats.finish(RunMeta {
                backend: BACKEND,
                bevy_version: &bevy_version(),
                scene: &b.scene,
                load_secs: b.clock.load_secs,
                measure_secs: b.clock.measure_secs,
                present_mode: format!("{:?}", b.present_mode),
                disturbances: &b.dist,
                refresh_hz: b.refresh_hz,
                window_px: b.window_px,
            });
            exit.send(AppExit::Success);
        }
        _ => {}
    }
}

/// --view: WASD / arrows pan, mouse wheel zoom.
fn view_controls(
    keys: Res<ButtonInput<KeyCode>>,
    mut wheel: EventReader<MouseWheel>,
    time: Res<Time>,
    mut cam: Query<(&mut Transform, &mut OrthographicProjection), With<Camera2d>>,
) {
    let Ok((mut t, mut proj)) = cam.get_single_mut() else { return };
    let mut dir = Vec2::ZERO;
    if keys.pressed(KeyCode::KeyW) || keys.pressed(KeyCode::ArrowUp) {
        dir.y += 1.0;
    }
    if keys.pressed(KeyCode::KeyS) || keys.pressed(KeyCode::ArrowDown) {
        dir.y -= 1.0;
    }
    if keys.pressed(KeyCode::KeyA) || keys.pressed(KeyCode::ArrowLeft) {
        dir.x -= 1.0;
    }
    if keys.pressed(KeyCode::KeyD) || keys.pressed(KeyCode::ArrowRight) {
        dir.x += 1.0;
    }
    t.translation += (dir * 600.0 * proj.scale * time.delta_seconds()).extend(0.0);
    for ev in wheel.read() {
        proj.scale = (proj.scale * (1.0 - 0.1 * ev.y)).clamp(0.02, 500.0);
    }
}
