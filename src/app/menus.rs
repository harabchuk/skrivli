//! File and Help menus. Items dispatch through [`super::runtime::dispatch`].

use gpui_kit::component::GlobalState;
use gpui_kit::{App, Menu, MenuItem};

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

pub(crate) fn install(cx: &mut App) {
    cx.set_menus([
        Menu::new("File").items([
            MenuItem::action("Settings", OpenSettingsMenu),
            MenuItem::action("Open Recordings Folder", OpenRecordingsFolderMenu),
            MenuItem::separator(),
            MenuItem::action("Exit", ExitApp),
        ]),
        Menu::new("Help").items([MenuItem::action("About", ShowAbout)]),
    ]);
    if let Some(menus) = cx.get_menus() {
        GlobalState::global_mut(cx).set_app_menus(menus);
    }
}
