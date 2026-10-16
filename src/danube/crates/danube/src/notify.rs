//! Each download as a desktop notification (docs/danube.md, "Downloads"):
//! one that stays while the file arrives, with its progress and a Cancel
//! button, replaced in place as it moves, and replaced once more by one
//! saying how it ended. derisk serves the notifications
//! (`org.freedesktop.Notifications`, which the sandbox's bus proxy lets
//! Danube talk to); without a server, or without a bus, Danube says what
//! it would have in its own toasts instead.
//!
//! On WebKit's thread, over GDBus. One call per notification is in flight
//! at a time: the id the server answers with is what the next one
//! replaces, and a newer card for the same download waits for it (and
//! replaces any card already waiting), so a fast download costs one call
//! a second, not one per chunk.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use tracing::{debug, warn};

use crate::downloads::DownloadId;
use crate::engine::{emit, later};
use crate::shared::{Command, DownloadCommand, Event};
use crate::webkit::{Arg, Bus, Var};

const SERVER: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";

/// How often a download's progress is shown again.
const EVERY: Duration = Duration::from_secs(1);

/// What one notification says.
#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    pub summary: String,
    pub body: String,
    /// 0 to 100, drawn as a progress bar.
    pub progress: Option<u8>,
    /// Stays until replaced or dismissed, with a Cancel button: the
    /// download is still running. The last card of a download is neither.
    pub running: bool,
}

/// One download's notification.
#[derive(Default)]
struct Note {
    /// The server's id for it, 0 until the first call answers.
    id: u32,
    in_flight: bool,
    /// The newest card not shown yet.
    waiting: Option<Card>,
    sent: Option<Instant>,
    /// The person closed it while the file was arriving: progress is not
    /// shown again, only how it ended.
    dismissed: bool,
    /// The last card went: forget the download once it is answered.
    ended: bool,
}

pub struct Notifier {
    /// `None` until the first download, and for good once the bus or the
    /// server turned out to be missing.
    bus: Option<Bus>,
    /// Whether anyone serves notifications; `None` until a call says.
    served: Option<bool>,
    notes: HashMap<DownloadId, Note>,
    me: Weak<RefCell<Notifier>>,
}

impl Notifier {
    pub fn new() -> Rc<RefCell<Self>> {
        Rc::new_cyclic(|me| {
            RefCell::new(Self {
                bus: None,
                served: None,
                notes: HashMap::new(),
                me: me.clone(),
            })
        })
    }

    /// Shows `card` for `download`, now or as soon as the call before it
    /// has answered.
    pub fn show(&mut self, download: DownloadId, card: Card) {
        if self.served == Some(false) {
            return toast(&card);
        }
        if self.bus.is_none() && !self.connect() {
            return toast(&card);
        }
        let note = self.notes.entry(download).or_default();
        if card.running && note.dismissed {
            return;
        }
        let soon = note.sent.is_some_and(|t| t.elapsed() < EVERY);
        if note.in_flight || (card.running && soon) {
            // Shown when the call in flight answers, or with the next
            // update; a running card that waits too long is only stale.
            note.waiting = Some(card);
            return;
        }
        self.send(download, card);
    }

    fn connect(&mut self) -> bool {
        match Bus::session() {
            Ok(bus) => {
                let me = self.me.clone();
                bus.on_signal(SERVER, SERVER, PATH, move |signal, args| {
                    if let Some(me) = me.upgrade() {
                        // A signal arrives from the main loop, never from
                        // inside a call made here.
                        if let Ok(mut me) = me.try_borrow_mut() {
                            me.signal(signal, &args);
                        }
                    }
                });
                self.bus = Some(bus);
                true
            }
            Err(error) => {
                warn!(error, "no session bus; downloads are shown as toasts");
                self.served = Some(false);
                false
            }
        }
    }

    fn send(&mut self, download: DownloadId, card: Card) {
        let Some(bus) = &self.bus else { return };
        let note = self.notes.entry(download).or_default();
        note.in_flight = true;
        note.sent = Some(Instant::now());
        note.ended = !card.running;
        let me = self.me.clone();
        let shown = card.clone();
        bus.call(SERVER, PATH, SERVER, "Notify", &notify_args(note.id, &card), move |reply| {
            let Some(me) = me.upgrade() else { return };
            let Ok(mut me) = me.try_borrow_mut() else { return };
            me.answered(download, reply.map(|args| match args.first() {
                Some(Arg::U32(id)) => *id,
                _ => 0,
            }), shown);
        });
    }

    fn answered(&mut self, download: DownloadId, reply: Result<u32, String>, card: Card) {
        let id = match reply {
            Ok(id) => id,
            Err(error) => {
                // No server owns the name: nothing will, this session.
                warn!(error, "nothing shows notifications; downloads are shown as toasts");
                self.served = Some(false);
                self.notes.clear();
                return toast(&card);
            }
        };
        self.served = Some(true);
        let Some(note) = self.notes.get_mut(&download) else { return };
        note.id = id;
        note.in_flight = false;
        let due = note.sent.is_none_or(|t| t.elapsed() >= EVERY);
        match note.waiting.take() {
            // The last card is never held back; a running one waits if it
            // would follow this one too soon.
            Some(next) if !next.running || due => self.send(download, next),
            Some(next) => note.waiting = Some(next),
            None if note.ended => {
                self.notes.remove(&download);
            }
            None => {}
        }
    }

    /// The person pressed Cancel, or closed a notification.
    fn signal(&mut self, signal: &str, args: &[Arg]) {
        let Some(Arg::U32(id)) = args.first() else { return };
        let Some((&download, note)) = self.notes.iter_mut().find(|(_, n)| n.id == *id) else {
            return;
        };
        match (signal, args.get(1)) {
            ("ActionInvoked", Some(Arg::Str(action))) if action == "cancel" => {
                debug!(download, "cancelled from its notification");
                later(Command::Download(DownloadCommand::Cancel(download)));
            }
            // Closed by the person (reason 2) while it was running: the
            // next card for it is the one that says how it ended, and it
            // is a new notification.
            ("NotificationClosed", Some(Arg::U32(2))) if !note.ended => {
                note.dismissed = true;
                note.id = 0;
                note.waiting = None;
            }
            ("NotificationClosed", _) if note.ended && !note.in_flight => {
                self.notes.remove(&download);
            }
            _ => {}
        }
    }
}

/// `Notify`'s arguments for `card`, replacing notification `replaces`.
fn notify_args(replaces: u32, card: &Card) -> Var {
    let mut hints = vec![
        ("desktop-entry".to_owned(), Var::Str("org.losos.Danube".into())),
        ("category".to_owned(), Var::Str(if card.running { "transfer" } else { "transfer.complete" }.into())),
    ];
    if let Some(value) = card.progress {
        hints.push(("value".into(), Var::I32(value.into())));
    }
    let (actions, timeout) = if card.running {
        // Kept, not expired: the server keeps a notification with no
        // timeout until it is replaced or dismissed. `resident` keeps it
        // after Cancel is pressed, until the card saying it was cancelled
        // replaces it.
        hints.push(("resident".into(), Var::Bool(true)));
        (vec!["cancel".to_owned(), "Cancel".to_owned()], 0)
    } else {
        // The server's own timeout for the last card.
        (Vec::new(), -1)
    };
    Var::Tuple(vec![
        Var::Str("Danube".into()),
        Var::U32(replaces),
        Var::Str("org.losos.Danube".into()),
        Var::Str(card.summary.clone()),
        Var::Str(card.body.clone()),
        Var::Strings(actions),
        Var::Dict(hints),
        Var::I32(timeout),
    ])
}

/// Without a notification server, only how a download ended is said, in
/// the window's toasts, as before there were notifications.
fn toast(card: &Card) {
    if !card.running {
        emit(Event::Notice(if card.body.is_empty() {
            card.summary.clone()
        } else {
            format!("{}: {}", card.summary, card.body)
        }));
    }
}

/// `n` bytes for a person: "812 kB", "12.4 MB", "1.07 GB", in the decimal
/// units browsers and file managers use.
pub fn size(n: u64) -> String {
    const UNITS: [&str; 5] = ["bytes", "kB", "MB", "GB", "TB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    match unit {
        0 => format!("{n} bytes"),
        _ if value < 10.0 => format!("{value:.2} {}", UNITS[unit]),
        _ if value < 100.0 => format!("{value:.1} {}", UNITS[unit]),
        _ => format!("{value:.0} {}", UNITS[unit]),
    }
}

/// The running card's body: how much of how much, how fast, over how many
/// connections (aria2's; WebKit's own transfer says none).
pub fn progress_body(received: u64, total: Option<u64>, speed: Option<u64>, connections: Option<u32>) -> String {
    let mut parts = vec![match total {
        Some(total) => format!("{} of {}", size(received), size(total)),
        None => size(received),
    }];
    if let Some(speed) = speed.filter(|&s| s > 0) {
        parts.push(format!("{}/s", size(speed)));
    }
    match connections {
        Some(1) => parts.push("1 connection".into()),
        Some(n) if n > 1 => parts.push(format!("{n} connections")),
        _ => {}
    }
    parts.join(" · ")
}

/// 0 to 100, when the size is known.
pub fn percent(received: u64, total: Option<u64>) -> Option<u8> {
    let total = total.filter(|&t| t > 0)?;
    Some((received.min(total) * 100 / total) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_like_a_file_manager() {
        assert_eq!(size(0), "0 bytes");
        assert_eq!(size(999), "999 bytes");
        assert_eq!(size(1_234), "1.23 kB");
        assert_eq!(size(12_400_000), "12.4 MB");
        assert_eq!(size(812_000), "812 kB");
        assert_eq!(size(1_070_000_000), "1.07 GB");
    }

    #[test]
    fn bodies_say_what_is_known() {
        assert_eq!(
            progress_body(12_400_000, Some(80_000_000), Some(4_200_000), Some(16)),
            "12.4 MB of 80.0 MB · 4.20 MB/s · 16 connections"
        );
        assert_eq!(progress_body(500, None, None, None), "500 bytes");
        assert_eq!(percent(50, Some(200)), Some(25));
        assert_eq!(percent(300, Some(200)), Some(100));
        assert_eq!(percent(1, None), None);
    }

    #[test]
    fn running_cards_stay_and_offer_cancel() {
        let card = Card {
            summary: "a.iso".into(),
            body: String::new(),
            progress: Some(40),
            running: true,
        };
        let Var::Tuple(args) = notify_args(7, &card) else { panic!() };
        assert_eq!(args[1], Var::U32(7));
        assert_eq!(args[5], Var::Strings(vec!["cancel".into(), "Cancel".into()]));
        assert_eq!(args[7], Var::I32(0));
        let Var::Dict(hints) = &args[6] else { panic!() };
        assert!(hints.contains(&("value".into(), Var::I32(40))));
        assert!(hints.contains(&("resident".into(), Var::Bool(true))));

        let done = Card { running: false, progress: None, ..card };
        let Var::Tuple(args) = notify_args(7, &done) else { panic!() };
        assert_eq!(args[5], Var::Strings(Vec::new()));
        assert_eq!(args[7], Var::I32(-1));
    }
}
