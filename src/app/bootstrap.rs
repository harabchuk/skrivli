//! GPUI Kit startup. Disk work finishes before the foreground loop.
//! The loop owns the window, the tray thread, the activation listener, and the
//! fake shell-event producer. `ShutdownController::join` runs after `run` returns.

use std::sync::atomic::AtomicIsize;
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
use crate::infrastructure::storage::settings::{Settings, ThemePreference};
use crate::infrastructure::windows::single_instance::{
    signal_primary_instance, InstanceGuardError, SingleInstance, ACTIVATE_EVENT_NAME,
    SINGLE_INSTANCE_MUTEX,
};
use crate::infrastructure::windows::tray::{self, TrayCommand};
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
    spawn_workers(tx, &shutdown);

    tracing::info!(
        close_to_tray = settings.close_to_tray(),
        start_hidden = settings.start_hidden(),
        "opening shell"
    );
    open_shell(settings, rx, writer, shutdown.clone());
    shutdown.join();
    drop(instance);
    drop(logs);
    0
}

fn spawn_workers(tx: mpsc::Sender<BridgeMessage>, shutdown: &ShutdownController) {
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
            Box::new(move |command: TrayCommand| {
                let _ = sender.send(BridgeMessage::Command(command_from_tray(command)));
            }),
        ) {
            tracing::error!(error = %error, "tray stopped");
        }
        tray_worker.acknowledge();
    }));
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

fn open_shell(
    settings: Settings,
    commands: mpsc::Receiver<BridgeMessage>,
    writer: SettingsWriter,
    shutdown: ShutdownController,
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
            let presentation = Presentation::new(
                settings.close_to_tray(),
                settings.tray_explanation_seen(),
                !settings.start_hidden(),
            );
            let window_shutdown = shutdown.clone();
            let window_writer = writer;
            let opened = gpui_kit::open_window(options, cx, move |window, cx| {
                window.set_window_title("Skrivli");
                cx.new(|cx| {
                    MainWindow::new(
                        presentation,
                        settings,
                        window_writer,
                        commands,
                        window_shutdown,
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
