//! Application entry wiring. Disk work finishes before the GPUI foreground starts.

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{
    px, size, App, AppContext, Context, Entity, Global, Subscription, WindowBounds, WindowOptions,
};

use crate::common::error::AppError;
use crate::common::logging;
use crate::common::paths::AppPaths;
use crate::infrastructure::storage::settings::{Settings, ThemePreference};
use crate::infrastructure::windows::single_instance::{
    InstanceGuardError, SingleInstance, SINGLE_INSTANCE_MUTEX,
};
use crate::views::foundation::FoundationView;

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
            tracing::info!("another Skrivli instance owns the mutex; exiting");
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
    let theme = settings.theme();
    let close_to_tray = settings.close_to_tray();
    let start_hidden = settings.start_hidden();
    tracing::info!(
        recordings = %settings.recordings_directory().display(),
        close_to_tray,
        start_hidden,
        "opening foundation window"
    );

    open_foundation(theme);
    drop(instance);
    drop(logs);
    0
}

fn prepare_local_dirs(paths: &AppPaths) -> Result<(), AppError> {
    for directory in [paths.models_dir(), paths.rescue_dir()] {
        std::fs::create_dir_all(&directory)
            .map_err(|source| AppError::io("prepare folders", source))?;
    }
    Ok(())
}

fn open_foundation(theme: ThemePreference) {
    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            apply_theme(theme, cx);
            let owner = cx.new(QuitOnLastWindow::new);
            cx.set_global(QuitOnLastWindowGlobal { owner });
            let options = foundation_window_options(cx);
            if let Err(error) = gpui_kit::open_window(options, cx, |window, cx| {
                window.set_window_title("Skrivli");
                cx.new(|cx| FoundationView::new(window, cx))
            }) {
                tracing::error!(error = %error, "failed to open window");
                eprintln!(
                    "{}",
                    AppError::message("open window", error.to_string()).user_message()
                );
                cx.quit();
            }
        });
}

fn apply_theme(preference: ThemePreference, cx: &mut App) {
    let mode = match preference {
        ThemePreference::Light => ThemeMode::Light,
        ThemePreference::Dark => ThemeMode::Dark,
        ThemePreference::System => ThemeMode::from(cx.window_appearance()),
    };
    Theme::change(mode, None, cx);
}

fn foundation_window_options(cx: &App) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::centered(size(px(720.), px(480.)), cx)),
        window_min_size: Some(size(px(560.), px(420.))),
        focus: true,
        ..Default::default()
    }
}

/// Quits the process when the last window closes. Held for the app lifetime.
struct QuitOnLastWindow {
    _subscription: Subscription,
}

impl QuitOnLastWindow {
    fn new(cx: &mut Context<Self>) -> Self {
        let subscription = cx.on_window_closed(|cx, _window_id| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        });
        Self {
            _subscription: subscription,
        }
    }
}

/// Keeps the quit subscription alive until the GPUI app shuts down.
struct QuitOnLastWindowGlobal {
    #[allow(dead_code)]
    owner: Entity<QuitOnLastWindow>,
}

impl Global for QuitOnLastWindowGlobal {}
