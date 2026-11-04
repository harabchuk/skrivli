//! Presentation snapshots owned by the GPUI shell. Services are not running yet.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AppTab {
    Record,
    Recordings,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Presentation {
    tab: AppTab,
    shell_pulses: u64,
    visible: bool,
    close_to_tray: bool,
    tray_explanation_seen: bool,
    notice: Option<&'static str>,
    pending_show: bool,
}

impl Presentation {
    pub(crate) fn new(close_to_tray: bool, tray_explanation_seen: bool, visible: bool) -> Self {
        Self {
            tab: AppTab::Record,
            shell_pulses: 0,
            visible,
            close_to_tray,
            tray_explanation_seen,
            notice: None,
            pending_show: false,
        }
    }

    pub(crate) fn tab(&self) -> AppTab {
        self.tab
    }

    pub(crate) fn set_tab(&mut self, tab: AppTab) {
        self.tab = tab;
    }

    pub(crate) fn visible(&self) -> bool {
        self.visible
    }

    pub(crate) fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
    }

    pub(crate) fn close_to_tray(&self) -> bool {
        self.close_to_tray
    }

    pub(crate) fn set_close_to_tray(&mut self, close_to_tray: bool) {
        self.close_to_tray = close_to_tray;
    }

    pub(crate) fn tray_explanation_seen(&self) -> bool {
        self.tray_explanation_seen
    }

    pub(crate) fn mark_tray_explanation_seen(&mut self) {
        self.tray_explanation_seen = true;
    }

    pub(crate) fn shell_pulses(&self) -> u64 {
        self.shell_pulses
    }

    pub(crate) fn add_shell_pulses(&mut self, count: u64) {
        self.shell_pulses = self.shell_pulses.saturating_add(count);
    }

    pub(crate) fn set_notice(&mut self, notice: Option<&'static str>) {
        self.notice = notice;
    }

    pub(crate) fn pending_show(&self) -> bool {
        self.pending_show
    }

    pub(crate) fn set_pending_show(&mut self, pending: bool) {
        self.pending_show = pending;
    }

    pub(crate) fn status_line(&self) -> String {
        let shell = if self.shell_pulses() == 0 {
            "Idle".to_string()
        } else {
            format!("Shell ready ({})", self.shell_pulses)
        };
        match self.notice {
            Some(notice) => format!("{notice} {shell}"),
            None => shell,
        }
    }
}
