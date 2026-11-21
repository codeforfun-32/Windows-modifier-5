//! The task list on the Focus page. Saved in %APPDATA%\DynamicIsland\tasks.txt
use std::path::PathBuf;
use std::sync::Mutex;

use slint::{ModelRc, VecModel, Weak};

use crate::{MainWindow, TaskItem};

static TASKS: Mutex<Vec<(bool, String)>> = Mutex::new(Vec::new());

fn file() -> PathBuf {
    shared::Config::dir().join("tasks.txt")
}

fn load() -> Vec<(bool, String)> {
    let text = std::fs::read_to_string(file()).unwrap_or_default();
    text.lines()
        .filter_map(|l| {
            let (flag, rest) = l.split_once('|')?;
            let rest = rest.trim();
            if rest.is_empty() {
                return None;
            }
            Some((flag == "1", rest.to_string()))
        })
        .collect()
}

fn save(list: &[(bool, String)]) {
    let _ = std::fs::create_dir_all(shared::Config::dir());
    let mut out = String::new();
    for (done, text) in list {
        out.push_str(if *done { "1|" } else { "0|" });
        out.push_str(&text.replace(['\n', '\r'], " "));
        out.push('\n');
    }
    let _ = std::fs::write(file(), out);
}

fn push_to_ui(ui: &MainWindow) {
    let list = TASKS.lock().map(|g| g.clone()).unwrap_or_default();
    let done = list.iter().filter(|t| t.0).count() as i32;
    let items: Vec<TaskItem> = list
        .into_iter()
        .map(|(done, text)| TaskItem { text: text.into(), done })
        .collect();
    ui.set_tasks(ModelRc::new(VecModel::from(items)));
    ui.set_tasks_done(done);
}

/// Load the saved tasks. Call once on the UI thread at start-up.
pub fn init(ui: &MainWindow) {
    if let Ok(mut g) = TASKS.lock() {
        *g = load();
    }
    push_to_ui(ui);
}

fn change(ui: &Weak<MainWindow>, f: impl FnOnce(&mut Vec<(bool, String)>)) {
    if let Ok(mut g) = TASKS.lock() {
        f(&mut g);
        save(&g);
    }
    if let Some(ui) = ui.upgrade() {
        push_to_ui(&ui);
    }
}

pub fn add(ui: &Weak<MainWindow>, text: &str) {
    let t = text.trim().to_string();
    if t.is_empty() {
        return;
    }
    change(ui, |l| l.push((false, t)));
}

pub fn toggle(ui: &Weak<MainWindow>, index: usize) {
    change(ui, |l| {
        if let Some(t) = l.get_mut(index) {
            t.0 = !t.0;
        }
    });
}

pub fn remove(ui: &Weak<MainWindow>, index: usize) {
    change(ui, |l| {
        if index < l.len() {
            l.remove(index);
        }
    });
}
