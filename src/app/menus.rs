//! Reserved GPUI actions for future shortcuts. Menus are no longer installed;
//! Settings opens from the tab-bar gear and the tray.

use gpui_kit::App;

use super::actions::AppCommand;
use super::runtime::dispatch;

gpui_kit::actions!(
    skrivli,
    [
        ShowSkrivli,
        OpenSettingsMenu,
        OpenRecordingsFolderMenu,
        ShowAbout,
        ExitApp
    ]
);

pub(crate) fn register(cx: &mut App) {
    cx.on_action(|_: &ShowSkrivli, cx| dispatch(AppCommand::Show, cx));
    cx.on_action(|_: &OpenSettingsMenu, cx| dispatch(AppCommand::OpenSettings, cx));
    cx.on_action(|_: &OpenRecordingsFolderMenu, cx| dispatch(AppCommand::OpenRecordingsFolder, cx));
    cx.on_action(|_: &ShowAbout, cx| dispatch(AppCommand::About, cx));
    cx.on_action(|_: &ExitApp, cx| dispatch(AppCommand::Exit, cx));
}
