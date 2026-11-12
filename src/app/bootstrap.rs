//! GPUI Kit startup. Disk work finishes before the foreground loop.
//! The loop owns the window, the tray thread, the activation listener, and the
//! fake shell-event producer. `ShutdownController::join` runs after `run` returns.

use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;

use gpui_kit::{
    point, px, size, App, AppContext as _, Bounds, Pixels, WindowBounds, WindowOptions,
};

use crate::common::error::AppError;
use crate::common::logging;
use crate::common::paths::AppPaths;
use crate::domain::language::Language;
use crate::domain::window_frame::{restore_window_bounds, LogicalRect};
use crate::infrastructure::ai::fetch::UreqModelSource;
use crate::infrastructure::ai::parakeet::ParakeetInference;
use crate::infrastructure::audio::energy_pause::EnergyPauseDetectorFactory;
use crate::infrastructure::audio::flex_capture::{FlexMicCapture, FlexSystemCapture};
use crate::infrastructure::audio::levels::{GainControl, LevelSnapshot};
use crate::infrastructure::audio::sonora_steps::SonoraPipelineFactory;
use crate::infrastructure::files::player::RodioOutput;
use crate::infrastructure::platform::{self, PlatformHost};
use crate::infrastructure::storage::settings::Settings;
use crate::services::lease::EndpointLease;
use crate::services::level_monitor::{spawn_level_monitor, MonitorTargets};
use crate::services::library::LibraryHost;
use crate::services::models::ModelHost;
use crate::services::playback::PlayerHost;
use crate::services::ports::{ActivationWatch, HostCommand, InstanceAcquire, SharedTrayText};
use crate::services::recording::{spawn_recording_host, RecordingEngine, RecordingHost};
use crate::services::transcription::TranscriptionHost;
use crate::ui::theme::apply_theme;
use crate::views::actions::{AppCommand, BridgeMessage};
use crate::views::hooks::ShellHooks;
use crate::views::main_window::MainWindow;
use crate::views::state::Presentation;
use crate::views::tray_text::tray_text;

use super::bridge::{spawn_shell_producer, SHELL_PULSE_INTERVAL};
use super::menus::register;
use super::runtime::ShellGlobal;
use super::settings_writer::SettingsWriter;
use super::shutdown::ShutdownController;

pub(crate) fn run() -> i32 {
    let host = platform::host();
    let paths = match platform::user_paths() {
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

    let (primary_guard, activation) = match host.single_instance.acquire() {
        Ok(InstanceAcquire::Primary { guard, activation }) => (guard, activation),
        Ok(InstanceAcquire::Secondary) => {
            tracing::info!("another Skrivli instance owns the mutex; signaling it");
            return 0;
        }
        Err(error) => {
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

    init_localization(settings.language());
    let tray_text: SharedTrayText = Arc::new(Mutex::new(tray_text(settings.language())));

    let shutdown = ShutdownController::new();
    let (tx, rx) = mpsc::channel();
    let writer = SettingsWriter::spawn(settings_path, &shutdown);
    let recording_flag = Arc::new(AtomicBool::new(false));
    let levels = Arc::new(LevelSnapshot::new());
    let mic_gain = Arc::new(GainControl::new(settings.mic_gain()));
    let system_gain = Arc::new(GainControl::new(settings.system_gain()));
    let lease = Arc::new(EndpointLease::new());
    let targets = Arc::new(MonitorTargets::new());
    let workers = spawn_workers(
        &host,
        tx,
        &shutdown,
        Arc::clone(&recording_flag),
        Arc::clone(&levels),
        Arc::clone(&mic_gain),
        Arc::clone(&system_gain),
        Arc::clone(&lease),
        Arc::clone(&targets),
        Arc::clone(&tray_text),
        paths.models_dir(),
        settings.recordings_directory().to_path_buf(),
        settings.notify_on_transcript(),
        activation,
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
        levels,
        mic_gain,
        system_gain,
        targets,
        tray_text,
        Arc::clone(&host.shell),
    );
    shutdown.join();
    drop(primary_guard);
    drop(logs);
    0
}

struct ShellWorkers {
    recording: RecordingHost,
    library: LibraryHost,
    player: PlayerHost,
    transcription: TranscriptionHost,
    model_host: ModelHost,
    models_dir: std::path::PathBuf,
}

#[allow(clippy::too_many_arguments)]
fn spawn_workers(
    host: &PlatformHost,
    tx: mpsc::Sender<BridgeMessage>,
    shutdown: &ShutdownController,
    recording_flag: Arc<AtomicBool>,
    levels: Arc<LevelSnapshot>,
    mic_gain: Arc<GainControl>,
    system_gain: Arc<GainControl>,
    lease: Arc<EndpointLease>,
    targets: Arc<MonitorTargets>,
    tray_text: SharedTrayText,
    models_dir: std::path::PathBuf,
    recordings_dir: std::path::PathBuf,
    notify_on_transcript: bool,
    activation: Box<dyn ActivationWatch>,
) -> ShellWorkers {
    let performance = Arc::from(platform::performance_counter());
    let (host_recording, session) = spawn_recording_host(
        RecordingEngine {
            mic: Box::new(FlexMicCapture::new(Arc::clone(&performance))),
            system: Box::new(FlexSystemCapture::new(Arc::clone(&performance))),
            clock: platform::clock(),
            space: platform::disk(),
            performance: Box::new(CloneCounter(Arc::clone(&performance))),
            levels: Arc::clone(&levels),
            mic_gain: Arc::clone(&mic_gain),
            system_gain: Arc::clone(&system_gain),
            pipeline: Box::new(SonoraPipelineFactory),
            lease: Arc::clone(&lease),
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
        Box::new(FlexMicCapture::new(Arc::clone(&performance))),
        Box::new(FlexSystemCapture::new(performance)),
        lease,
        levels,
        mic_gain,
        system_gain,
        targets,
        monitor_stop,
    ));
    let stop_host = host_recording.clone();
    shutdown.on_stop(move || stop_host.shutdown());
    shutdown.track(session);

    let device_tx = tx.clone();
    let device_stop = shutdown.flag();
    shutdown.track(platform::spawn_device_watch(
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

    let activation_shutdown = shutdown.clone();
    let activation_tx = tx.clone();
    let activation_stop = shutdown.flag();
    shutdown.track(thread::spawn(move || {
        listen_for_activation(activation.as_ref(), &activation_stop, &activation_tx);
        activation_shutdown.acknowledge();
    }));

    let (notice_tx, notice_rx) = mpsc::channel();
    let notifier = platform::completion_notifier(notice_tx);
    let transcription_tx = tx.clone();
    let (transcription, transcription_thread) = TranscriptionHost::spawn_with_notifier(
        Box::new(ParakeetInference::new()),
        Box::new(EnergyPauseDetectorFactory),
        crate::services::transcription::TranscriptionConfig::default(),
        notifier,
        Arc::new(AtomicBool::new(notify_on_transcript)),
        Box::new(move |snapshot| {
            let _ = transcription_tx.send(BridgeMessage::Transcription(snapshot));
        }),
    );
    let stop_transcription = transcription.clone();
    shutdown.on_stop(move || stop_transcription.shutdown());
    shutdown.track(transcription_thread);

    let library_tx = tx.clone();
    let library_stop = shutdown.flag();
    let library_identity = transcription.clone();
    let (library, library_thread) = LibraryHost::spawn(
        recordings_dir,
        Arc::clone(&host.trash),
        Arc::clone(&host.watch),
        Arc::new(library_identity),
        Box::new(move |snapshot| {
            let _ = library_tx.send(BridgeMessage::Library(snapshot));
        }),
        Arc::clone(&library_stop),
    );
    let stop_library = library.clone();
    shutdown.on_stop(move || stop_library.shutdown());
    shutdown.track(library_thread);

    let player_tx = tx.clone();
    let (player, player_thread) = PlayerHost::spawn(
        Box::new(RodioOutput),
        Box::new(move |snapshot| {
            let _ = player_tx.send(BridgeMessage::Player(snapshot));
        }),
    );
    let stop_player = player.clone();
    shutdown.on_stop(move || stop_player.shutdown());
    shutdown.track(player_thread);

    let model_tx = tx.clone();
    let (model_host, model_thread) = ModelHost::spawn(
        models_dir.clone(),
        Box::new(UreqModelSource),
        Box::new(move |snapshot| {
            let _ = model_tx.send(BridgeMessage::Model(snapshot));
        }),
    );
    let stop_model = model_host.clone();
    shutdown.on_stop(move || stop_model.shutdown());
    shutdown.track(model_thread);

    let tray = Arc::clone(&host.tray);
    let tray_worker = shutdown.clone();
    let tray_tx = tx;
    let tray_stop = shutdown.flag();
    shutdown.track(thread::spawn(move || {
        let sender = tray_tx;
        if let Err(error) = tray.run(
            tray_stop,
            recording_flag,
            notice_rx,
            tray_text,
            Box::new(move |command: HostCommand| {
                let _ = sender.send(BridgeMessage::Command(AppCommand::from(command)));
            }),
        ) {
            tracing::error!(error = %error, "tray stopped");
        }
        tray_worker.acknowledge();
    }));
    ShellWorkers {
        recording: host_recording,
        library,
        player,
        transcription,
        model_host,
        models_dir,
    }
}

/// Loads the translations before any text is shown. Kit's own controls read the same
/// locale, and the `gpui_component.` entries in our catalog override their text.
fn init_localization(language: Language) {
    use gpui_kit::component as gpui_component;
    rust_i18n::extend!(gpui_component);
    gpui_kit::component::set_locale(language.code());
}

/// Share one performance-counter implementation with `RecordingEngine`'s `Box` slot.
struct CloneCounter(Arc<dyn crate::services::ports::PerformanceCounter>);

impl crate::services::ports::PerformanceCounter for CloneCounter {
    fn sample(&self) -> (u64, u64) {
        self.0.sample()
    }
}

fn listen_for_activation(
    activation: &dyn ActivationWatch,
    stop: &std::sync::atomic::AtomicBool,
    tx: &mpsc::Sender<BridgeMessage>,
) {
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    while !stop.load(Ordering::Acquire) {
        if activation.wait_timeout(Duration::from_millis(200)) {
            let _ = tx.send(BridgeMessage::Command(AppCommand::Show));
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
    levels: Arc<LevelSnapshot>,
    mic_gain: Arc<GainControl>,
    system_gain: Arc<GainControl>,
    targets: Arc<MonitorTargets>,
    tray_text: SharedTrayText,
    shell: Arc<dyn crate::services::ports::DesktopShell>,
) {
    let theme = settings.theme();
    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            apply_theme(theme, None, cx);
            register(cx);
            let options = shell_window_options(&settings, cx);
            let presentation = Presentation::new(
                settings.close_to_tray(),
                settings.tray_explanation_seen(),
                !settings.start_hidden(),
            );
            let window_recording = workers.recording;
            let window_library = workers.library;
            let window_player = workers.player;
            let window_transcription = workers.transcription;
            let window_model_host = workers.model_host;
            let window_models_dir = workers.models_dir;
            let window_levels = levels;
            let window_mic_gain = mic_gain;
            let window_system_gain = system_gain;
            let window_targets = targets;
            let window_shell = shell;
            let hooks = ShellHooks::new(
                {
                    let writer = writer;
                    Arc::new(move |settings| writer.save(settings))
                },
                {
                    let shutdown = shutdown.clone();
                    Arc::new(move || {
                        let _ = shutdown.request();
                    })
                },
                shutdown.flag(),
                tray_text,
            );
            let opened = gpui_kit::open_window(options, cx, move |window, cx| {
                window.set_window_title("Skrivli");
                cx.new(|cx| {
                    MainWindow::new(
                        presentation,
                        settings,
                        hooks,
                        commands,
                        window_shell,
                        window_recording,
                        window_library,
                        window_player,
                        window_transcription,
                        window_model_host,
                        window_models_dir,
                        window_levels,
                        window_mic_gain,
                        window_system_gain,
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

fn prepare_local_dirs(paths: &AppPaths) -> Result<(), AppError> {
    std::fs::create_dir_all(paths.models_dir())
        .map_err(|source| AppError::io("prepare folders", source))?;
    Ok(())
}
