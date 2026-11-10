//! bevy_spine (jabuwu) benchmark runner. See README.md and ../../BENCHMARK_SPEC.md.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

use bench_core::{Args, BenchClock, Disturbances, DrawOrder, FrameStats, InstanceSpec, Motion, Phase, RunMeta, Scene};
use bevy::asset::AssetPlugin;
use bevy::diagnostic::DiagnosticsStore;
use bevy::input::mouse::MouseWheel;
use bevy::prelude::*;
use bevy::render::diagnostic::RenderDiagnosticsPlugin;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured, save_to_disk};
use bevy::render::{Render, RenderApp, RenderSystems};
use bevy::window::{
    Monitor, OnMonitor, PresentMode, PrimaryWindow, WindowBackendScaleFactorChanged, WindowFocused, WindowMoved,
    WindowOccluded, WindowResized, WindowResolution, WindowScaleFactorChanged,
};
use bevy::winit::{UpdateMode, WinitSettings};
use bevy_spine::{
    SkeletonController, SkeletonData, SkeletonDataHandle, Spine, SpinePlugin, SpineReadyEvent, SpineSet,
};

const BACKEND: &str = "bevy_spine";
const BEVY_VERSION: &str = "0.19.0";

#[derive(Resource)]
struct Bench {
    args: Args,
    scene: Scene,
    specs: Vec<InstanceSpec>,
    clock: BenchClock,
    stats: FrameStats,
    ready: usize,
    frame_start: Option<Instant>,
    prev_start: Option<Instant>,
    frame_ms: Option<f64>,
    screenshot_requested: bool,
    screenshot_written: Arc<AtomicBool>,
    present_mode: &'static str,
    // view mode
    title_timer: f32,
    title_frames: u32,
    title_ms: f64,
    dist: Disturbances,
    refresh_hz: Option<f64>,
    window_px: (u32, u32),
}

/// Latest render-world CPU time (nanoseconds), written by render-app systems, read by main world.
#[derive(Resource, Clone)]
struct RenderCpuNs(Arc<AtomicU64>);

/// Render-app-side start-of-frame timestamp.
#[derive(Resource, Default)]
struct RenderStart(Option<Instant>);

/// Index into `Bench::specs`.
#[derive(Component)]
struct InstanceIdx(usize);

fn main() {
    let start = Instant::now();
    let args = Args::parse_args();
    let scene = Scene::load(&args.scene, &args).unwrap_or_else(|e| {
        eprintln!("error: {e}");
        std::process::exit(2);
    });
    let specs = scene.instances();
    let (present, present_name) = if args.view {
        (PresentMode::AutoVsync, "AutoVsync")
    } else {
        (PresentMode::AutoNoVsync, "AutoNoVsync")
    };
    println!(
        "[{BACKEND}] scene={} rig={} draw_order={:?} motion={:?} requested present mode: {present_name}",
        scene.name,
        scene.rig_label(),
        scene.draw_order,
        scene.motion
    );

    let render_ns = RenderCpuNs(Arc::new(AtomicU64::new(0)));
    let bench = Bench {
        clock: BenchClock::new(start, &args),
        stats: FrameStats::new(),
        ready: 0,
        frame_start: None,
        prev_start: None,
        frame_ms: None,
        screenshot_requested: false,
        screenshot_written: Arc::new(AtomicBool::new(false)),
        present_mode: present_name,
        title_timer: 0.0,
        title_frames: 0,
        title_ms: 0.0,
        dist: Disturbances::new(),
        refresh_hz: None,
        window_px: (0, 0),
        args: args.clone(),
        scene,
        specs,
    };

    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(WindowPlugin {
                primary_window: Some(Window {
                    title: "bevy_spine benchmark".into(),
                    resolution: WindowResolution::new(1920, 1080).with_scale_factor_override(1.0),
                    resizable: false,
                    present_mode: present,
                    ..default()
                }),
                ..default()
            })
            .set(AssetPlugin {
                file_path: bench_core::assets_dir().to_string_lossy().into_owned(),
                ..default()
            }),
    )
    .add_plugins(SpinePlugin)
    .insert_resource(WinitSettings {
        focused_mode: UpdateMode::Continuous,
        unfocused_mode: UpdateMode::Continuous,
    })
    .insert_resource(bench)
    .insert_resource(render_ns.clone())
    .add_systems(Startup, setup)
    .add_systems(Update, on_ready.in_set(SpineSet::OnReady))
    .add_systems(Update, (watch_window, update_motion))
    .add_systems(First, frame_start.before(bevy::time::TimeSystems))
    .add_systems(Last, frame_end);

    if args.view {
        app.add_systems(Update, view_controls);
    } else {
        app.add_plugins(RenderDiagnosticsPlugin);
    }

    if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
        render_app
            .init_resource::<RenderStart>()
            .insert_resource(render_ns)
            .add_systems(Render, render_begin.before(RenderSystems::ExtractCommands))
            .add_systems(Render, render_end.after(RenderSystems::PostCleanup));
    }
    app.run();
}

fn setup(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut skeletons: ResMut<Assets<SkeletonData>>,
    bench: Res<Bench>,
) {
    commands.spawn(Camera2d);
    // One SkeletonData per rig (each rig is a distinct skeleton type / asset).
    let data: Vec<_> = bench
        .scene
        .rig_paths()
        .into_iter()
        .map(|(json, atlas)| skeletons.add(SkeletonData::new_from_json(asset_server.load(json), asset_server.load(atlas))))
        .collect();
    for s in &bench.specs {
        commands.spawn((
            SkeletonDataHandle(data[s.rig].clone()),
            instance_transform(&bench.scene, s, 0.0),
            InstanceIdx(s.index),
        ));
    }
}

/// Position at time `t`; for `y_sorted` scenes z encodes the order (lower feet y = larger z = in front).
/// The slot/mesh children are offset by tiny z steps (0.001 per slot) relative to the skeleton, which
/// `bench_core::y_sorted_z` keeps well below the z gap between skeletons 1 px apart in y.
fn instance_transform(scene: &Scene, s: &InstanceSpec, t: f64) -> Transform {
    let p = s.pos_at(t);
    let z = if scene.draw_order == DrawOrder::YSorted { bench_core::y_sorted_z(s.sort_y(t)) } else { p[2] };
    Transform::from_xyz(p[0], p[1], z).with_scale(Vec3::new(s.scale, s.scale, 1.0))
}

/// `bob` scenes: moves every instance (and its z, for `y_sorted`) each frame; transform propagation then
/// updates the whole bone/mesh hierarchy, which is part of what is measured.
fn update_motion(bench: Res<Bench>, mut q: Query<(&InstanceIdx, &mut Transform)>) {
    if bench.scene.motion != Motion::Bob {
        return;
    }
    let t = bench.clock.elapsed_secs(Instant::now());
    for (idx, mut transform) in &mut q {
        *transform = instance_transform(&bench.scene, &bench.specs[idx.0], t);
    }
}

/// Runs in `SpineSet::OnReady`, i.e. before the first animation update of a new skeleton.
fn on_ready(
    mut events: MessageReader<SpineReadyEvent>,
    mut spines: Query<(&mut Spine, &InstanceIdx)>,
    mut bench: ResMut<Bench>,
) {
    for ev in events.read() {
        let Ok((mut spine, idx)) = spines.get_mut(ev.entity) else { continue };
        let spec = &bench.specs[idx.0];
        let Spine(SkeletonController { animation_state, .. }) = spine.as_mut();
        let dur = animation_state
            .data()
            .skeleton_data()
            .find_animation(&spec.anim)
            .map(|a| a.duration())
            .unwrap_or(0.0);
        match animation_state.set_animation_by_name(0, &spec.anim, true) {
            Ok(mut entry) => entry.set_track_time(spec.phase_frac * dur),
            Err(e) => eprintln!("animation '{}' not found: {e:?}", spec.anim),
        }
        bench.ready += 1;
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

fn frame_start(mut bench: ResMut<Bench>) {
    let now = Instant::now();
    bench.frame_ms = bench.prev_start.map(|p| now.duration_since(p).as_secs_f64() * 1000.0);
    bench.prev_start = Some(now);
    bench.frame_start = Some(now);
}

fn frame_end(
    mut commands: Commands,
    mut bench: ResMut<Bench>,
    render_ns: Res<RenderCpuNs>,
    diagnostics: Option<Res<DiagnosticsStore>>,
    mut exit: MessageWriter<AppExit>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
) {
    let now = Instant::now();
    let main_ms = bench.frame_start.map(|s| now.duration_since(s).as_secs_f64() * 1000.0).unwrap_or(0.0);

    if bench.args.view {
        bench.title_frames += 1;
        if let Some(f) = bench.frame_ms {
            bench.title_ms += f;
            bench.title_timer += (f / 1000.0) as f32;
        }
        if bench.title_timer >= 0.5 {
            let ms = bench.title_ms / bench.title_frames.max(1) as f64;
            if let Ok(mut w) = windows.single_mut() {
                w.title = format!("bevy_spine view | {:.1} fps | {:.2} ms", 1000.0 / ms.max(1e-6), ms);
            }
            bench.title_timer = 0.0;
            bench.title_frames = 0;
            bench.title_ms = 0.0;
        }
        return;
    }

    let all_ready = bench.ready >= bench.specs.len();
    let phase = bench.clock.update(now, all_ready);
    match phase {
        Phase::Measure => {
            if let Some(frame_ms) = bench.frame_ms {
                let r = render_ns.0.load(Ordering::Relaxed);
                let render_ms = (r > 0).then(|| r as f64 / 1e6);
                let gpu = diagnostics.as_deref().and_then(gpu_ms);
                bench.stats.push(frame_ms, main_ms, render_ms, gpu);
            }
        }
        Phase::Screenshot => {
            if !bench.screenshot_requested {
                bench.screenshot_requested = true;
                let path = bench_core::screenshot_path(BACKEND, &bench.scene);
                let _ = std::fs::remove_file(&path);
                let flag = bench.screenshot_written.clone();
                commands
                    .spawn(Screenshot::primary_window())
                    .observe(save_to_disk(path))
                    .observe(move |_: On<ScreenshotCaptured>| flag.store(true, Ordering::SeqCst));
            } else if bench.screenshot_written.load(Ordering::SeqCst) {
                bench.clock.screenshot_done();
            }
        }
        Phase::Done => {
            let b = &*bench;
            b.stats.finish(RunMeta {
                backend: BACKEND,
                bevy_version: BEVY_VERSION,
                scene: &b.scene,
                load_secs: b.clock.load_secs,
                measure_secs: b.clock.measure_secs,
                present_mode: b.present_mode.to_string(),
                disturbances: &b.dist,
                refresh_hz: b.refresh_hz,
                window_px: b.window_px,
            });
            exit.write(AppExit::Success);
        }
        _ => {}
    }
}

/// Sum of top-level GPU pass timings: diagnostics `render/<pass>/elapsed_gpu` (exactly 3 path
/// components, so nested spans are not double counted). None if no GPU timestamps are present.
fn gpu_ms(store: &DiagnosticsStore) -> Option<f64> {
    let mut sum = 0.0;
    let mut any = false;
    for d in store.iter() {
        let p = d.path();
        if p.components().count() == 3 && p.as_str().ends_with("/elapsed_gpu") {
            if let Some(v) = d.value() {
                sum += v;
                any = true;
            }
        }
    }
    any.then_some(sum)
}

fn render_begin(mut s: ResMut<RenderStart>) {
    s.0 = Some(Instant::now());
}

fn render_end(s: Res<RenderStart>, ns: Res<RenderCpuNs>) {
    if let Some(t) = s.0 {
        ns.0.store(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
    }
}

fn view_controls(
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    mut wheel: MessageReader<MouseWheel>,
    mut cam: Query<(&mut Transform, &mut Projection), With<Camera2d>>,
) {
    let Ok((mut t, mut proj)) = cam.single_mut() else { return };
    let Projection::Orthographic(o) = proj.as_mut() else { return };
    let mut d = Vec2::ZERO;
    if keys.pressed(KeyCode::KeyW) || keys.pressed(KeyCode::ArrowUp) {
        d.y += 1.0;
    }
    if keys.pressed(KeyCode::KeyS) || keys.pressed(KeyCode::ArrowDown) {
        d.y -= 1.0;
    }
    if keys.pressed(KeyCode::KeyD) || keys.pressed(KeyCode::ArrowRight) {
        d.x += 1.0;
    }
    if keys.pressed(KeyCode::KeyA) || keys.pressed(KeyCode::ArrowLeft) {
        d.x -= 1.0;
    }
    t.translation += (d * 800.0 * o.scale * time.delta_secs()).extend(0.0);
    for w in wheel.read() {
        o.scale = (o.scale * 0.9f32.powf(w.y)).clamp(0.05, 50.0);
    }
}
