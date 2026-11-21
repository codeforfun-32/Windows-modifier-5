use slint::Weak;

use crate::MainWindow;

/// Show a short message in the island's ticker and add it to the notification list.
/// Safe to call from any thread.
pub fn push(ui: &Weak<MainWindow>, text: String) {
    crate::notifications::record(&text);
    let snapshot = crate::notifications::merged();
    let _ = ui.upgrade_in_event_loop(move |ui| {
        ui.invoke_show_ticker(text.into());
        ui.set_notifs(crate::notifications::to_model(snapshot));
        ui.set_notif_unread(ui.get_notif_unread() + 1);
    });
}

/// Pop-up card with an app tile (WhatsApp, clipboard, ...). Must be called on the UI thread.
pub fn toast(ui: &MainWindow, app: &str, title: &str, body: &str) {
    let name = if app.trim().is_empty() { "Notification" } else { app.trim() };
    ui.set_ticker_letter(crate::notifications::letter_of(name).into());
    ui.set_ticker_hue(crate::notifications::hue_of(name));
    ui.invoke_show_toast(name.to_uppercase().into(), title.into(), body.into());
}
