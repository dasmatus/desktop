//! `danube --background URL --size WIDTHxHEIGHT [--scale S]`: one page
//! drawn for derisk's desktop background, with no window. derisk starts it
//! as it starts ffmpeg for a video wallpaper, and the frames go the same
//! way, down stdout:
//!
//!     width: u32 LE, height: u32 LE, then width × height premultiplied
//!     RGBA pixels, row by row, for every frame the page renders.
//!
//! derisk writes `hide` or `show` lines on stdin when the background is
//! covered or uncovered; a hidden page stops rendering, as a tab behind
//! another does. Closing stdin ends it.
//!
//! The page gets nothing of the person's: a throwaway profile, no
//! extensions, no permission ever granted, no sound, no keyboard or
//! pointer. It still gets the system's content blockers. A load that fails
//! ends the run with the reason on stderr and a failed exit, so derisk
//! falls back to its own background; so does a crashed web process.

use std::convert::Infallible;
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::path::Path;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crossbeam_channel::{Receiver, TrySendError};
use eframe::egui;
use tracing::info;

use crate::shared::{Command, Commands, Event};

/// What to show, and at what size.
#[derive(Clone, Debug, PartialEq)]
pub struct Page {
    pub uri: String,
    /// Logical pixels.
    pub width: i32,
    pub height: i32,
    pub scale: f64,
}

/// `WIDTHxHEIGHT` in logical pixels.
pub fn parse_size(text: &str) -> Option<(i32, i32)> {
    let (w, h) = text.split_once('x')?;
    let (w, h) = (w.parse().ok()?, h.parse().ok()?);
    (w > 0 && h > 0 && w <= 16384 && h <= 16384).then_some((w, h))
}

/// Only web pages and local files: `javascript:`, `data:` and the like
/// make no background, and the setting is a file a program could write.
pub fn allowed(uri: &str) -> bool {
    ["https://", "http://", "file:///"]
        .iter()
        .any(|scheme| uri.starts_with(scheme))
}

/// The folder a `file://` page lives in, which the sandbox shows it
/// read-only so that its pictures and scripts load.
pub fn folder(uri: &str) -> Option<&Path> {
    let path = Path::new(uri.strip_prefix("file://")?);
    path.parent().filter(|p| p.is_absolute())
}

/// Moves stdout to a descriptor of the frames' own and points descriptor
/// 1 at stderr. WebKit's processes inherit stdout, and a web process
/// writes to it (a page's console, with tracing on): in the stream, that
/// would be read as a frame. Called before WebKit starts.
pub fn take_stdout() -> std::io::Result<File> {
    // SAFETY: plain descriptor calls; the new one is ours alone, and not
    // inherited by the processes WebKit starts.
    unsafe {
        let frames = libc::fcntl(libc::STDOUT_FILENO, libc::F_DUPFD_CLOEXEC, 3);
        if frames < 0 {
            return Err(std::io::Error::last_os_error());
        }
        if libc::dup2(libc::STDERR_FILENO, libc::STDOUT_FILENO) < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(File::from_raw_fd(frames))
    }
}

/// Takes WebKit's events until it quits: frames go to the writer, the one
/// page's failures end the run, everything that would ask the person is
/// refused. Returns why the page could not be shown, if it could not.
pub fn run(
    commands: &Commands,
    events: Receiver<Event>,
    page: &Page,
    out: File,
) -> Result<(), String> {
    commands.send(Command::Resize {
        width: page.width,
        height: page.height,
        scale: page.scale,
    });
    // Nothing types into it.
    commands.send(Command::Focus(false));

    // One frame in flight: when derisk stops reading, the writer blocks,
    // and newer frames replace the one waiting instead of queueing.
    let (frames, pending) = crossbeam_channel::bounded::<Arc<egui::ColorImage>>(1);
    // Dropped once WebKit has quit, which ends the stdin reader.
    let (done, stop) = crossbeam_channel::bounded::<Infallible>(0);
    thread::scope(|s| {
        s.spawn(|| write_frames(out, &pending, commands));
        s.spawn(|| read_visibility(commands, &stop));
        let mut first = None;
        let mut failure = None;
        for event in &events {
            match event {
                Event::Opened(tab) => {
                    // The page's own popups have nowhere to show.
                    match first {
                        None => first = Some(tab.id),
                        Some(_) => {
                            commands.send(Command::Close(tab.id));
                        }
                    }
                }
                Event::Frame(id, image) if Some(id) == first => {
                    if let Err(TrySendError::Full(image)) = frames.try_send(image) {
                        // The writer is still on the last one: drop the
                        // one waiting for this, newer, one.
                        let _ = pending.try_recv();
                        let _ = frames.try_send(image);
                    }
                }
                Event::LoadFailed { tab, uri, error } if Some(tab) == first => {
                    failure = Some(format!("{uri}: {error}"));
                    commands.send(Command::Quit);
                }
                Event::Crashed(id) if Some(id) == first => {
                    failure = Some(format!("{}: the page's web process ended", page.uri));
                    commands.send(Command::Quit);
                }
                Event::Permission(ask) => {
                    commands.send(Command::Permission {
                        id: ask.id,
                        allow: false,
                        remember: false,
                    });
                }
                Event::Quit => break,
                _ => {}
            }
        }
        // The writer and the stdin reader see their channels close.
        drop(frames);
        drop(done);
        failure.map_or(Ok(()), Err)
    })
}

/// Writes each frame to `out`, what was stdout; quits WebKit once it is
/// gone (derisk stopped the background).
fn write_frames(mut out: File, frames: &Receiver<Arc<egui::ColorImage>>, commands: &Commands) {
    for image in frames {
        let [w, h] = image.size;
        let header = [(w as u32).to_le_bytes(), (h as u32).to_le_bytes()].concat();
        let pixels: &[u8] = image.as_raw();
        if out
            .write_all(&header)
            .and_then(|()| out.write_all(pixels))
            .is_err()
        {
            info!("stdout closed; the background is no longer shown");
            commands.send(Command::Quit);
            return;
        }
    }
}

/// Reads `show` and `hide` from stdin; quits WebKit when stdin closes.
/// Polls, so that it also ends when WebKit has stopped by itself, which
/// `stop` disconnecting says.
fn read_visibility(commands: &Commands, stop: &Receiver<Infallible>) {
    let stdin = std::io::stdin();
    let fd = stdin.as_raw_fd();
    let mut lines = BufReader::new(stdin.lock());
    let mut line = String::new();
    loop {
        let mut poll = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        // A line already read into the buffer would wait for the next
        // write otherwise.
        let ready = if lines.buffer().is_empty() {
            // SAFETY: one pollfd that lives across the call.
            unsafe { libc::poll(&mut poll, 1, 200) }
        } else {
            1
        };
        if ready <= 0 {
            if crate::stopped(stop, Duration::ZERO) {
                return;
            }
            continue;
        }
        line.clear();
        match lines.read_line(&mut line) {
            Ok(0) | Err(_) => {
                commands.send(Command::Quit);
                return;
            }
            Ok(_) => {
                let shown = match line.trim() {
                    "show" => true,
                    "hide" => false,
                    _ => continue,
                };
                commands.send(Command::Shown(shown));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_two_positive_numbers() {
        assert_eq!(parse_size("1920x1080"), Some((1920, 1080)));
        assert_eq!(parse_size("0x1080"), None);
        assert_eq!(parse_size("1920"), None);
        assert_eq!(parse_size("-5x10"), None);
        assert_eq!(parse_size("99999x10"), None);
    }

    #[test]
    fn only_web_pages_and_files_make_a_background() {
        assert!(allowed("https://example.org/"));
        assert!(allowed("http://localhost:8080/"));
        assert!(allowed("file:///home/me/page/index.html"));
        assert!(!allowed("javascript:alert(1)"));
        assert!(!allowed("data:text/html,hi"));
        assert!(!allowed("file://host/share/index.html"));
    }

    #[test]
    fn a_file_page_shows_its_folder() {
        assert_eq!(
            folder("file:///home/me/page/index.html"),
            Some(Path::new("/home/me/page"))
        );
        assert_eq!(folder("https://example.org/a/b.html"), None);
    }
}
