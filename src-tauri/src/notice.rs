//! OS notifications: which Transfer events are worth interrupting the user for while they are
//! not looking at the window, what they say, and how they are shown.
//!
//! They are ordinary notifications at normal urgency, with no sound of their own and no action
//! buttons, so the OS's do-not-disturb mode holds them back like any other app's.

use std::{collections::HashMap, sync::Mutex};

use bhayanakshare_core::{
    Contact, Device, DeviceId, Event, EventKind, Role, TransferEvent, TransferId, TransferState,
};
use tauri::{AppHandle, Manager, Runtime};

use crate::background;

/// What a Transfer event is worth telling the user about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    /// An incoming Offer is waiting for an answer.
    Offer,
    /// A Transfer a Contact's Auto-accept took without asking has finished.
    AutoAccepted,
    /// A Transfer ended in failure.
    Failed { reason: String },
}

/// Remembers which Transfers were accepted without asking, which only the first event of each
/// shows.
#[derive(Default)]
pub struct Tracker {
    /// Every Transfer that has not ended, and whether it began already Accepted.
    live: HashMap<TransferId, bool>,
}

impl Tracker {
    pub fn observe(&mut self, t: &TransferEvent) -> Option<Notice> {
        // An auto-accepted Offer is announced as Accepted straight away, never as Offered.
        let auto = *self
            .live
            .entry(t.transfer_id)
            .or_insert(t.role == Role::Receiver && t.state == TransferState::Accepted);
        if t.state.is_terminal() {
            self.live.remove(&t.transfer_id);
        }
        match &t.state {
            TransferState::Offered if t.role == Role::Receiver => Some(Notice::Offer),
            TransferState::Completed { .. } if auto => Some(Notice::AutoAccepted),
            TransferState::Failed { reason } => Some(Notice::Failed { reason: reason.clone() }),
            _ => None,
        }
    }
}

/// The [`Tracker`] as Tauri state. Absent in tests of the rest of the shell, which then get no
/// notifications.
#[derive(Default)]
pub struct Notifier(pub Mutex<Tracker>);

/// Shows a notification for `event` if it is one the user should hear about and they are not
/// already looking at it.
pub async fn notify<R: Runtime>(app: &AppHandle<R>, event: &Event) {
    let EventKind::Transfer(t) = &event.kind else { return };
    let Some(notifier) = app.try_state::<Notifier>() else { return };
    let notice = notifier.0.lock().unwrap_or_else(|e| e.into_inner()).observe(t);
    let Some(notice) = notice else { return };

    let window = app.get_webview_window("main");
    let state = WindowState {
        visible: window.as_ref().is_some_and(|w| {
            w.is_visible().unwrap_or(false) && !w.is_minimized().unwrap_or(false)
        }),
        focused: window.as_ref().is_some_and(|w| w.is_focused().unwrap_or(false)),
    };
    if !worth_showing(&notice, state) {
        return;
    }

    let contact = match app.try_state::<Device>() {
        Some(device) => device
            .contacts()
            .await
            .ok()
            .and_then(|all| all.into_iter().find(|c| c.id == t.peer)),
        None => None,
    };
    let (title, body) = text(&notice, t.role, &t.name, &who(t.peer, t.peer_name.as_deref(), contact.as_ref()));
    let on_click = (notice == Notice::Offer).then(|| {
        let app = app.clone();
        let id = t.transfer_id;
        Box::new(move || background::open_offer(&app, id)) as Box<dyn FnOnce() + Send>
    });
    show(app, &title, &body, on_click);
}

/// Where the main window is, which decides whether a notification is of any use.
#[derive(Debug, Clone, Copy)]
pub struct WindowState {
    pub visible: bool,
    pub focused: bool,
}

/// An Offer needs a notification only when the window is hidden (or minimised): otherwise its
/// sheet is already on screen. The others also reach a user who has the window open but is
/// working in another application.
pub fn worth_showing(notice: &Notice, window: WindowState) -> bool {
    match notice {
        Notice::Offer => !window.visible,
        Notice::AutoAccepted | Notice::Failed { .. } => !window.focused,
    }
}

/// How to refer to the other Device: a Contact by its Nickname or Device Name, anyone else by
/// the name it announced plus its Fingerprint (a name is only what the Device says), or by the
/// Fingerprint alone.
pub fn who(peer: DeviceId, announced: Option<&str>, contact: Option<&Contact>) -> String {
    let print = peer.fingerprint();
    match contact {
        Some(c) => c.display_name().or(announced).map_or(print, str::to_owned),
        None => match announced {
            Some(name) => format!("{name} · {print}"),
            None => print,
        },
    }
}

/// The title and body of a notification.
pub fn text(notice: &Notice, role: Role, name: &str, who: &str) -> (String, String) {
    match (notice, role) {
        (Notice::Offer, _) => ("Incoming file".into(), format!("{name} from {who}")),
        (Notice::AutoAccepted, _) => ("File received".into(), format!("{name} from {who}")),
        (Notice::Failed { reason }, Role::Sender) => {
            ("Could not send".into(), format!("{name} to {who}. {reason}"))
        }
        (Notice::Failed { reason }, Role::Receiver) => {
            ("Could not receive".into(), format!("{name} from {who}. {reason}"))
        }
    }
}

/// Shown the first time the window is closed.
pub const TRAY_TITLE: &str = "BhayanakShare is still running";
pub const TRAY_BODY: &str =
    "Closing the window keeps BhayanakShare in the background so you can receive files. Open it or quit it from its tray menu.";

/// Shows a notification. `on_click` runs if the user clicks it, on platforms where that can be
/// known (Linux); it is called from a background thread. Failure to show one (no notification
/// service running) is logged and otherwise ignored.
#[cfg(target_os = "linux")]
pub fn show<R: Runtime>(
    _app: &AppHandle<R>,
    title: &str,
    body: &str,
    on_click: Option<Box<dyn FnOnce() + Send>>,
) {
    let mut notification = notify_rust::Notification::new();
    notification.appname("BhayanakShare").summary(title).body(body).auto_icon();
    if on_click.is_some() {
        // The server's "default" action is the notification itself being clicked; it is not
        // offered as a button.
        notification.action("default", "Open");
    }
    std::thread::spawn(move || match notification.show() {
        // Blocks until the notification is clicked or goes away.
        Ok(handle) => handle.wait_for_action(|action| {
            if action == "default" {
                if let Some(on_click) = on_click {
                    on_click();
                }
            }
        }),
        Err(e) => tracing::debug!("could not show a notification: {e}"),
    });
}

/// Shows a notification. The Tauri plugin cannot report clicks on the desktop, so `on_click`
/// is dropped: clicking only brings the application forward, as the OS does.
#[cfg(not(target_os = "linux"))]
pub fn show<R: Runtime>(
    app: &AppHandle<R>,
    title: &str,
    body: &str,
    _on_click: Option<Box<dyn FnOnce() + Send>>,
) {
    use tauri_plugin_notification::NotificationExt;
    if let Err(e) = app.notification().builder().title(title).body(body).show() {
        tracing::debug!("could not show a notification: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(id: u8, role: Role, state: TransferState) -> TransferEvent {
        TransferEvent {
            transfer_id: TransferId::from_bytes([id; 16]),
            role,
            peer: peer(),
            peer_name: None,
            name: "photo.jpg".into(),
            size: 10,
            expires_at: 0,
            state,
        }
    }

    fn peer() -> DeviceId {
        // The Ed25519 base point: some valid public key.
        "LBTGMZTGMZTGMZTGMZTGMZTGMZTGMZTGMZTGMZTGMZTGMZTGMZTA".parse().unwrap()
    }

    #[test]
    fn an_incoming_offer_notifies_but_an_outgoing_one_does_not() {
        let mut tracker = Tracker::default();
        assert_eq!(
            tracker.observe(&event(1, Role::Receiver, TransferState::Offered)),
            Some(Notice::Offer)
        );
        assert_eq!(tracker.observe(&event(2, Role::Sender, TransferState::Offered)), None);
    }

    #[test]
    fn only_a_transfer_accepted_without_asking_notifies_on_completion() {
        let done = TransferState::Completed { saved_to: Some("/x".into()) };
        let mut tracker = Tracker::default();

        // Began Accepted: Auto-accept took it.
        tracker.observe(&event(1, Role::Receiver, TransferState::Accepted));
        tracker.observe(&event(1, Role::Receiver, TransferState::Transferring));
        assert_eq!(
            tracker.observe(&event(1, Role::Receiver, done.clone())),
            Some(Notice::AutoAccepted)
        );

        // Began Offered, accepted by the user: they already know.
        tracker.observe(&event(2, Role::Receiver, TransferState::Offered));
        tracker.observe(&event(2, Role::Receiver, TransferState::Accepted));
        assert_eq!(tracker.observe(&event(2, Role::Receiver, done.clone())), None);

        // A Sender's completion is not a receipt.
        tracker.observe(&event(3, Role::Sender, TransferState::Offered));
        assert_eq!(tracker.observe(&event(3, Role::Sender, done)), None);
    }

    #[test]
    fn a_failure_notifies_on_either_side_and_ended_transfers_are_forgotten() {
        let failed = TransferState::Failed { reason: "lost".into() };
        let mut tracker = Tracker::default();
        let notice = Some(Notice::Failed { reason: "lost".into() });
        tracker.observe(&event(1, Role::Sender, TransferState::Transferring));
        assert_eq!(tracker.observe(&event(1, Role::Sender, failed.clone())), notice);
        tracker.observe(&event(2, Role::Receiver, TransferState::Accepted));
        assert_eq!(tracker.observe(&event(2, Role::Receiver, failed)), notice);
        assert!(tracker.live.is_empty());
    }

    #[test]
    fn an_offer_notifies_when_the_window_is_hidden_the_others_when_it_is_not_focused() {
        let hidden = WindowState { visible: false, focused: false };
        let behind = WindowState { visible: true, focused: false };
        let front = WindowState { visible: true, focused: true };
        let failed = Notice::Failed { reason: String::new() };
        assert!(worth_showing(&Notice::Offer, hidden));
        assert!(!worth_showing(&Notice::Offer, behind));
        assert!(worth_showing(&Notice::AutoAccepted, behind));
        assert!(worth_showing(&failed, hidden));
        assert!(!worth_showing(&failed, front));
        assert!(!worth_showing(&Notice::AutoAccepted, front));
    }

    #[test]
    fn a_stranger_is_named_with_its_fingerprint_and_a_contact_by_its_own_name() {
        assert_eq!(who(peer(), Some("Laptop"), None), "Laptop · LBTG-MZTG");
        assert_eq!(who(peer(), None, None), "LBTG-MZTG");
        let contact = Contact {
            id: peer(),
            nickname: Some("Mum".into()),
            device_name: Some("Laptop".into()),
            auto_accept: false,
            last_known_address: Default::default(),
            added_at: 1,
        };
        assert_eq!(who(peer(), Some("Other"), Some(&contact)), "Mum");
    }

    #[test]
    fn the_text_says_what_and_who() {
        let (title, body) = text(&Notice::Offer, Role::Receiver, "a.txt", "Mum");
        assert_eq!((title.as_str(), body.as_str()), ("Incoming file", "a.txt from Mum"));
        let failed = Notice::Failed { reason: "The other Device went away.".into() };
        assert_eq!(text(&failed, Role::Sender, "a.txt", "Mum").0, "Could not send");
        assert_eq!(
            text(&failed, Role::Receiver, "a.txt", "Mum").1,
            "a.txt from Mum. The other Device went away."
        );
    }
}
