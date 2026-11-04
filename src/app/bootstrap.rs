//! GPUI Kit startup. Disk work finishes before the foreground loop.
//! The loop owns the window, the tray thread, the activation listener, and the
//! fake shell-event producer. `ShutdownController::join` runs after `run` returns.

use std::sync::atomic::{AtomicBool, AtomicIsize};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{
    point, px, size, App, AppContext as _, Bounds, Pixels, WindowBounds, WindowOptions,
};

use crate::common::error::AppError;
use crate::common::logging;
use crate::common::paths::AppPaths;
use crate::domain::window_frame::{restore_window_bounds, LogicalRect};
use crate::infrastructure::audio::devices::spawn_device_watch;
use crate::infrastructure::audio::flex_capture::{FlexMicCapture, FlexSystemCapture};
use crate::infrastructure::audio::levels::{GainControl, LevelSnapshot};
use crate::infrastructure::audio::system::{QpcCounter, SystemClock, WindowsDisk};
use crate::infrastructure::files::player::PlayerHost;
use crate::infrastructure::files::wav::recover_partials;
use crate::infrastructure::storage::settings::{Settings, ThemePreference};
use crate::infrastructure::windows::single_instance::{
    signal_primary_instance, InstanceGuardError, SingleInstance, ACTIVATE_EVENT_NAME,
    SINGLE_INSTANCE_MUTEX,
};
use crate::infrastructure::windows::tray::{self, TrayCommand};
use crate::services::lease::EndpointLease;
use crate::services::level_monitor::{spawn_level_monitor, MonitorTargets};
use crate::services::library::LibraryHost;
use crate::services::recording::{
    spawn_recording_host, RecordingEngine, RecordingHost, NOTICE_RECOVERED, NOTICE_RECOVER_FAILED,
};
use crate::views::main_window::MainWindow;

use super::actions::{command_from_tray, AppCommand, BridgeMessage};
use super::bridge::{spawn_shell_producer, SHELL_PULSE_INTERVAL};
use super::menus::{install, register};
use super::runtime::ShellGlobal;
use super::settings_writer::SettingsWriter;
use super::shutdown::ShutdownController;
use super::state::Presentation;

pub(crate) fn run() -> i32 {
    let paths = match AppPaths::from_env() {
        Ok(paths) => paths,
        Err(error) => {
            eprintln!("{}", error.user_message());
            return 1;
        }
    };

    // Declared before the mutex so Drop flushes logs after the mutex is released.
    let logs = match logging::init(&paths.logs_dir()) {
        Ok(logs) => logs,
        Err(error) => {
            eprintln!("{}", error.user_message());
            return 1;
        }
    };

    let instance = match SingleInstance::acquire(SINGLE_INSTANCE_MUTEX) {
        Ok(instance) => instance,
        Err(InstanceGuardError::AlreadyRunning) => {
            tracing::info!("another Skrivli instance owns the mutex; signaling it");
            if let Err(error) = signal_primary_instance(ACTIVATE_EVENT_NAME) {
                tracing::warn!(error = %error, "the running instance was not signaled");
            }
            return 0;
        }
        Err(InstanceGuardError::Failed(error)) => {
            tracing::error!(error = %error, "single-instance guard failed");
            eprintln!("{}", error.user_message());
            return 1;
        }
    };

    if let Err(error) = prepare_local_dirs(&paths) {
        tracing::error!(error = %error, "local folders were not created");
        eprintln!("{}", error.user_message());
        return 1;
    }

    let defaults = match Settings::defaults(paths.recordings_dir()) {
        Ok(defaults) => defaults,
        Err(error) => {
            tracing::error!(error = %error, "default settings were rejected");
            eprintln!("{}", error.user_message());
            return 1;
        }
    };
    let settings_path = paths.settings_file();
    let settings = if settings_path.is_file() {
        Settings::load(&settings_path, defaults)
    } else if let Err(error) = defaults.save(&settings_path) {
        tracing::error!(error = %error, "default settings were not saved");
        eprintln!("{}", error.user_message());
        return 1;
    } else {
        defaults
    };

    let shutdown = ShutdownController::new();
    let (tx, rx) = mpsc::channel();
    let writer = SettingsWriter::spawn(settings_path, &shutdown);
    let recording_flag = Arc::new(AtomicBool::new(false));
    let recovery_notice = recovery_notice(settings.recordings_directory());
    let levels = Arc::new(LevelSnapshot::new());
    let gain = Arc::new(GainControl::new(
        settings.mic_gain(),
        settings.agc_enabled(),
    ));
    let lease = Arc::new(EndpointLease::new());
    let targets = Arc::new(MonitorTargets::new());
    let workers = spawn_workers(
        tx,
        &shutdown,
        Arc::clone(&recording_flag),
        Arc::clone(&levels),
        Arc::clone(&gain),
        Arc::clone(&lease),
        Arc::clone(&targets),
        paths.rescue_dir(),
        settings.recordings_directory().to_path_buf(),
    );

    tracing::info!(
        close_to_tray = settings.close_to_tray(),
        start_hidden = settings.start_hidden(),
        explicit_system_endpoint = matches!(
            settings.system_audio(),
            crate::infrastructure::storage::settings::StoredEndpoint::Explicit(_)
        ),
        "opening shell"
    );
    open_shell(
        settings,
        rx,
        writer,
        shutdown.clone(),
        workers,
        recovery_notice,
        levels,
        gain,
        targets,
    );
    shutdown.join();
    drop(instance);
    drop(logs);
    0
}

struct ShellWorkers {
    recording: RecordingHost,
    library: LibraryHost,
    player: PlayerHost,
}

#[allow(clippy::too_many_arguments)]
fn spawn_workers(
    tx: mpsc::Sender<BridgeMessage>,
    shutdown: &ShutdownController,
    recording_flag: Arc<AtomicBool>,
    levels: Arc<LevelSnapshot>,
    gain: Arc<GainControl>,
    lease: Arc<EndpointLease>,
    targets: Arc<MonitorTargets>,
    rescue_dir: std::path::PathBuf,
    recordings_dir: std::path::PathBuf,
) -> ShellWorkers {
    let (host, session) = spawn_recording_host(
        RecordingEngine {
            mic: Box::new(FlexMicCapture),
            system: Box::new(FlexSystemCapture),
            clock: Box::new(SystemClock),
            space: Box::new(WindowsDisk),
            performance: Box::new(QpcCounter),
            levels: Arc::clone(&levels),
            gain: Arc::clone(&gain),
            lease: Arc::clone(&lease),
            rescue_dir,
            space_check_every: crate::services::recording::SPACE_CHECK_INTERVAL,
            write_fault: Arc::new(AtomicBool::new(false)),
            folder_fault: Arc::new(AtomicBool::new(false)),
            device_wake: Arc::new(AtomicBool::new(false)),
            stall_after: crate::services::recording::STALL_IDLE,
        },
        {
            let tx = tx.clone();
            Box::new(move |snapshot| {
                let _ = tx.send(BridgeMessage::Recording(snapshot));
            })
        },
        Arc::clone(&recording_flag),
    );
    let monitor_stop = shutdown.flag();
    shutdown.track(spawn_level_monitor(
        Box::new(FlexMicCapture),
        Box::new(FlexSystemCapture),
        lease,
        levels,
        gain,
        targets,
        monitor_stop,
    ));
    let stop_host = host.clone();
    shutdown.on_stop(move || stop_host.shutdown());
    shutdown.track(session);

    let device_tx = tx.clone();
    let device_stop = shutdown.flag();
    shutdown.track(spawn_device_watch(
        Box::new(move |catalog| {
            let _ = device_tx.send(BridgeMessage::Devices(catalog));
        }),
        device_stop,
    ));

    let producer = shutdown.clone();
    let producer_tx = tx.clone();
    let producer_stop = shutdown.flag();
    shutdown.track(thread::spawn(move || {
        let handle = spawn_shell_producer(producer_tx, producer_stop, SHELL_PULSE_INTERVAL);
        let _ = handle.join();
        producer.acknowledge();
    }));

    let activation = shutdown.clone();
    let activation_tx = tx.clone();
    let activation_stop = shutdown.flag();
    shutdown.track(thread::spawn(move || {
        listen_for_activation(&activation_stop, &activation_tx);
        activation.acknowledge();
    }));

    let library_tx = tx.clone();
    let library_stop = shutdown.flag();
    let (library, library_thread) = LibraryHost::spawn(
        recordings_dir,
        Box::new(move |snapshot| {
            let _ = library_tx.send(BridgeMessage::Library(snapshot));
        }),
        Arc::clone(&library_stop),
    );
    let stop_library = library.clone();
    shutdown.on_stop(move || stop_library.shutdown());
    shutdown.track(library_thread);

    let player_tx = tx.clone();
    let (player, player_thread) = PlayerHost::spawn(Box::new(move |snapshot| {
        let _ = player_tx.send(BridgeMessage::Player(snapshot));
    }));
    let stop_player = player.clone();
    shutdown.on_stop(move || stop_player.shutdown());
    shutdown.track(player_thread);

    let hwnd_slot = Arc::new(AtomicIsize::new(0));
    let nudge_slot = Arc::clone(&hwnd_slot);
    shutdown.on_stop(move || tray::nudge(&nudge_slot));
    let tray_worker = shutdown.clone();
    let tray_tx = tx;
    let tray_stop = shutdown.flag();
    shutdown.track(thread::spawn(move || {
        let sender = tray_tx;
        if let Err(error) = tray::run(
            tray_stop,
            hwnd_slot,
            recording_flag,
            Box::new(move |command: TrayCommand| {
                let _ = sender.send(BridgeMessage::Command(command_from_tray(command)));
            }),
        ) {
            tracing::error!(error = %error, "tray stopped");
        }
        tray_worker.acknowledge();
    }));
    ShellWorkers {
        recording: host,
        library,
        player,
    }
}

fn listen_for_activation(stop: &std::sync::atomic::AtomicBool, tx: &mpsc::Sender<BridgeMessage>) {
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    use crate::infrastructure::windows::single_instance::ShowSignal;

    let signal = match ShowSignal::create(ACTIVATE_EVENT_NAME) {
        Ok(signal) => Some(signal),
        Err(error) => {
            tracing::error!(error = %error, "activation event was not created");
            None
        }
    };
    while !stop.load(Ordering::Acquire) {
        if let Some(signal) = &signal {
            if signal.wait_timeout(Duration::from_millis(200)) {
                let _ = tx.send(BridgeMessage::Command(AppCommand::Show));
            }
        } else {
            thread::sleep(Duration::from_millis(200));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn open_shell(
    settings: Settings,
    commands: mpsc::Receiver<BridgeMessage>,
    writer: SettingsWriter,
    shutdown: ShutdownController,
    workers: ShellWorkers,
    recovery_notice: Option<&'static str>,
    levels: Arc<LevelSnapshot>,
    gain: Arc<GainControl>,
    targets: Arc<MonitorTargets>,
) {
    let theme = settings.theme();
    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            apply_theme(theme, cx);
            register(cx);
            install(cx);
            let options = shell_window_options(&settings, cx);
            let mut presentation = Presentation::new(
                settings.close_to_tray(),
                settings.tray_explanation_seen(),
                !settings.start_hidden(),
            );
            presentation.set_notice(recovery_notice);
            let window_shutdown = shutdown.clone();
            let window_writer = writer;
            let window_recording = workers.recording;
            let window_library = workers.library;
            let window_player = workers.player;
            let window_levels = levels;
            let window_gain = gain;
            let window_targets = targets;
            let opened = gpui_kit::open_window(options, cx, move |window, cx| {
                window.set_window_title("Skrivli");
                cx.new(|cx| {
                    MainWindow::new(
                        presentation,
                        settings,
                        window_writer,
                        commands,
                        window_shutdown,
                        window_recording,
                        window_library,
                        window_player,
                        window_levels,
                        window_gain,
                        window_targets,
                        window,
                        cx,
                    )
                })
            });
            match opened {
                Ok((window, session)) => {
                    let lifetime = shutdown.clone();
                    let closed = cx.on_window_closed(move |cx, _| {
                        if cx.windows().is_empty() {
                            lifetime.request();
                            cx.quit();
                        }
                    });
                    cx.set_global(ShellGlobal::new(session, window, closed));
                }
                Err(error) => {
                    tracing::error!(error = %error, "failed to open window");
                    eprintln!(
                        "{}",
                        AppError::message("open window", error.to_string()).user_message()
                    );
                    shutdown.request();
                    cx.quit();
                }
            }
        });
}

fn shell_window_options(settings: &Settings, cx: &App) -> WindowOptions {
    let restored = restored_bounds(settings, cx);
    WindowOptions {
        window_bounds: Some(
            restored.unwrap_or_else(|| WindowBounds::centered(size(px(720.), px(480.)), cx)),
        ),
        window_min_size: Some(size(px(560.), px(420.))),
        focus: true,
        show: !settings.start_hidden(),
        ..Default::default()
    }
}

fn restored_bounds(settings: &Settings, cx: &App) -> Option<WindowBounds> {
    let stored = settings.window_bounds()?;
    let saved = LogicalRect::new(stored.x(), stored.y(), stored.width(), stored.height())?;
    let (primary, others) = work_areas(cx);
    let primary = primary?;
    let restored = restore_window_bounds(saved, primary, &others);
    Some(WindowBounds::Windowed(Bounds::new(
        point(px(restored.x() as f32), px(restored.y() as f32)),
        size(px(restored.width() as f32), px(restored.height() as f32)),
    )))
}

fn work_areas(cx: &App) -> (Option<LogicalRect>, Vec<LogicalRect>) {
    let primary_id = cx.primary_display().map(|display| display.id());
    let mut primary = None;
    let mut others = Vec::new();
    for display in cx.displays() {
        let rect = logical_area(display.visible_bounds());
        if primary_id == Some(display.id()) {
            primary = rect;
        } else if let Some(rect) = rect {
            others.push(rect);
        }
    }
    (primary, others)
}

fn logical_area(bounds: Bounds<Pixels>) -> Option<LogicalRect> {
    LogicalRect::new(
        bounds.origin.x.to_i32(),
        bounds.origin.y.to_i32(),
        bounds.size.width.to_i32().max(1),
        bounds.size.height.to_i32().max(1),
    )
}

fn recovery_notice(directory: &std::path::Path) -> Option<&'static str> {
    let report = recover_partials(directory);
    if report.unrecoverable > 0 {
        Some(NOTICE_RECOVER_FAILED)
    } else if report.recovered > 0 {
        Some(NOTICE_RECOVERED)
    } else {
        None
    }
}

fn apply_theme(preference: ThemePreference, cx: &mut App) {
    let mode = match preference {
        ThemePreference::Light => ThemeMode::Light,
        ThemePreference::Dark => ThemeMode::Dark,
        ThemePreference::System => ThemeMode::from(cx.window_appearance()),
    };
    Theme::change(mode, None, cx);
}

fn prepare_local_dirs(paths: &AppPaths) -> Result<(), AppError> {
    for directory in [paths.models_dir(), paths.rescue_dir()] {
        std::fs::create_dir_all(&directory)
            .map_err(|source| AppError::io("prepare folders", source))?;
    }
    Ok(())
}
