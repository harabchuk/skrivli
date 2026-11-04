//! Background settings saves. The GPUI thread only enqueues a snapshot.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use crate::infrastructure::storage::settings::Settings;

use super::shutdown::ShutdownController;

enum SettingsMsg {
    Save(Settings),
    Stop,
}

#[derive(Clone)]
pub(crate) struct SettingsWriter {
    tx: Sender<SettingsMsg>,
}

impl SettingsWriter {
    pub(crate) fn spawn(path: PathBuf, shutdown: &ShutdownController) -> Self {
        let (tx, rx) = mpsc::channel();
        let stop_tx = tx.clone();
        shutdown.on_stop(move || {
            let _ = stop_tx.send(SettingsMsg::Stop);
        });
        let worker = shutdown.clone();
        let handle = thread::spawn(move || {
            write_loop(&path, rx);
            worker.acknowledge();
        });
        shutdown.track(handle);
        Self { tx }
    }

    pub(crate) fn save(&self, settings: Settings) {
        let _ = self.tx.send(SettingsMsg::Save(settings));
    }
}

fn write_loop(path: &std::path::Path, rx: Receiver<SettingsMsg>) {
    while let Ok(message) = rx.recv() {
        match message {
            SettingsMsg::Stop => break,
            SettingsMsg::Save(mut settings) => {
                while let Ok(next) = rx.try_recv() {
                    match next {
                        SettingsMsg::Save(newer) => settings = newer,
                        SettingsMsg::Stop => {
                            store(path, &settings);
                            return;
                        }
                    }
                }
                store(path, &settings);
            }
        }
    }
}

fn store(path: &std::path::Path, settings: &Settings) {
    if let Err(error) = settings.save(path) {
        tracing::error!(error = %error, "settings were not saved");
    }
}
