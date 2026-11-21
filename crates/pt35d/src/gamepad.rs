//! The uConsole's face buttons, as keys.
//!
//! Its keyboard sends A B X Y, Select and Start as joystick buttons, and sway
//! binds only keys. This thread reads the joystick and presses a key the pt35
//! firmware sends on a uinput keyboard of its own, so the PocketTerm35's
//! bindings serve both. The joystick is grabbed: the shell owns the buttons,
//! and a game would get both otherwise.
//!
//! The uConsole has no L or R, and its trackball is the mouse, so three face
//! buttons take the jobs that need a button of their own: A is Start (the
//! launcher), B is Select (the window switcher), Y is L (close the window).
//!
//! Select goes out when it is let go, and only if the trackball did not
//! scroll meanwhile: the keyboard turns the trackball into a wheel while
//! Select is held.

use std::fs::File;
use std::io::Read;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::hardware::bitmap_has;
use crate::vkbd::FaceKeys;

/// USBComposite's default ids, which the uConsole keyboard firmware keeps.
const VENDOR: &str = "1eaf";
const PRODUCT: &str = "0024";

const EV_KEY: u16 = 1;
const EV_REL: u16 = 2;
const REL_HWHEEL: u16 = 6;
const REL_WHEEL: u16 = 8;
const REL_WHEEL_HI_RES: u16 = 11;
const REL_HWHEEL_HI_RES: u16 = 12;
const KEY_A: usize = 30;
/// HID button n of a joystick arrives as `BTN_JOYSTICK + n - 1`.
const BTN_JOYSTICK: u16 = 0x120;
const SELECT: u16 = BTN_JOYSTICK + 8;
const F21: u16 = 191;

/// Joystick button to the pt35 firmware's key it stands for.
const KEYS: [(u16, u16); 5] = [
    (BTN_JOYSTICK, 185),     // button 1, X: F15, Tab
    (BTN_JOYSTICK + 1, 192), // button 2, A: F22, Start: the launcher
    (BTN_JOYSTICK + 2, 191), // button 3, B: F21, Select: the window switcher
    (BTN_JOYSTICK + 3, 183), // button 4, Y: F13, L: close the window
    (BTN_JOYSTICK + 9, 192), // button 10, Start: F22
];

/// How often a missing keyboard is looked for again. It is built in, so it is
/// missing only while it resets.
const RESCAN: Duration = Duration::from_secs(3);

/// `struct input_event` on a 64-bit kernel.
const EVENT: usize = 24;

// _IOW('E', 0x90, int) from linux/input.h.
const EVIOCGRAB: u64 = 0x4004_4590;
const POLLIN: i16 = 1;

#[repr(C)]
struct PollFd {
    fd: i32,
    events: i16,
    revents: i16,
}

extern "C" {
    fn ioctl(fd: i32, request: u64, ...) -> i32;
    fn poll(fds: *mut PollFd, nfds: u64, timeout: i32) -> i32;
}

pub fn spawn() {
    let spawned = std::thread::Builder::new()
        .name("gamepad".into())
        .spawn(run);
    if let Err(e) = spawned {
        log::warn!("no uConsole face buttons: {e}");
    }
}

fn run() {
    // Made before the joystick is found: sway takes a moment to adopt a new
    // keyboard, and the first press must not be the one that is lost.
    let mut keys = match FaceKeys::open() {
        Ok(keys) => keys,
        Err(e) => {
            log::warn!("/dev/uinput: {e}; the uConsole's face buttons do nothing");
            return;
        }
    };
    let mut missing_said = false;
    loop {
        match find(Path::new("/sys/class/input")) {
            Some(found) => {
                missing_said = false;
                if let Err(e) = serve(&found, &mut keys) {
                    log::info!("uConsole joystick {}: {e}", found.joystick.display());
                }
            }
            None if !missing_said => {
                log::warn!("no uConsole joystick ({VENDOR}:{PRODUCT}); looking again every 3s");
                missing_said = true;
            }
            None => {}
        }
        std::thread::sleep(RESCAN);
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Found {
    joystick: PathBuf,
    /// The trackball, to see it scroll while Select is held.
    mouse: Option<PathBuf>,
    /// Off when the joystick shares a device with the letters: a grab would
    /// take the keyboard away from sway.
    grab: bool,
}

/// The uConsole keyboard's joystick and mouse, under `class` (`/sys/class/input`).
fn find(class: &Path) -> Option<Found> {
    let mut joystick = None;
    let mut mouse = None;
    let mut entries: Vec<PathBuf> = std::fs::read_dir(class)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("event"))
        })
        .collect();
    entries.sort();
    for event in entries {
        let device = event.join("device");
        let read = |file: &str| std::fs::read_to_string(device.join(file)).unwrap_or_default();
        if read("id/vendor").trim() != VENDOR || read("id/product").trim() != PRODUCT {
            continue;
        }
        let node = Path::new("/dev/input").join(event.file_name()?);
        let keys = read("capabilities/key");
        if joystick.is_none() && bitmap_has(&keys, BTN_JOYSTICK as usize) {
            joystick = Some((node, !bitmap_has(&keys, KEY_A)));
        } else if mouse.is_none() && bitmap_has(&read("capabilities/rel"), REL_WHEEL as usize) {
            mouse = Some(node);
        }
    }
    let (joystick, grab) = joystick?;
    Some(Found {
        joystick,
        mouse,
        grab,
    })
}

/// Turn joystick events into key events until the device goes away.
fn serve(found: &Found, keys: &mut FaceKeys) -> std::io::Result<()> {
    let mut joystick = File::open(&found.joystick)?;
    // SAFETY: EVIOCGRAB takes an int by value.
    if found.grab && unsafe { ioctl(joystick.as_raw_fd(), EVIOCGRAB, 1) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // Without the trackball Select still works; it only cannot tell a scroll.
    let mut mouse = found.mouse.as_ref().and_then(|path| File::open(path).ok());
    log::info!(
        "uConsole joystick {}: the face buttons are keys",
        found.joystick.display()
    );
    let mut buttons = Buttons::default();
    let mut buf = [0u8; EVENT * 16];
    loop {
        let mut fds = vec![PollFd {
            fd: joystick.as_raw_fd(),
            events: POLLIN,
            revents: 0,
        }];
        if let Some(mouse) = &mouse {
            fds.push(PollFd {
                fd: mouse.as_raw_fd(),
                events: POLLIN,
                revents: 0,
            });
        }
        // SAFETY: `fds` is a live array of `fds.len()` pollfd structs.
        if unsafe { poll(fds.as_mut_ptr(), fds.len() as u64, -1) } < 0 {
            let e = std::io::Error::last_os_error();
            if e.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(e);
        }
        if fds.get(1).is_some_and(|fd| fd.revents != 0) {
            match mouse.as_mut().map(|m| m.read(&mut buf)) {
                Some(Ok(n)) if n > 0 => {
                    for (kind, code, _) in events(&buf[..n]) {
                        buttons.mouse(kind, code);
                    }
                }
                // The trackball went: carry on without it.
                _ => mouse = None,
            }
        }
        if fds[0].revents != 0 {
            let n = joystick.read(&mut buf)?;
            if n == 0 {
                return Err(std::io::ErrorKind::UnexpectedEof.into());
            }
            for (kind, code, value) in events(&buf[..n]) {
                for (key, down) in buttons.joystick(kind, code, value) {
                    keys.send(key, down)?;
                }
            }
        }
    }
}

fn events(buf: &[u8]) -> impl Iterator<Item = (u16, u16, i32)> + '_ {
    buf.chunks_exact(EVENT).map(|e| {
        (
            u16::from_ne_bytes([e[16], e[17]]),
            u16::from_ne_bytes([e[18], e[19]]),
            i32::from_ne_bytes([e[20], e[21], e[22], e[23]]),
        )
    })
}

/// What the buttons mean, apart from where the events come from.
#[derive(Default)]
struct Buttons {
    /// Wheel events seen from the trackball so far.
    scrolls: u64,
    /// `scrolls` when Select went down, while it is held.
    select: Option<u64>,
}

impl Buttons {
    fn mouse(&mut self, kind: u16, code: u16) {
        if kind == EV_REL
            && matches!(
                code,
                REL_WHEEL | REL_HWHEEL | REL_WHEEL_HI_RES | REL_HWHEEL_HI_RES
            )
        {
            self.scrolls += 1;
        }
    }

    /// The key presses one joystick event stands for.
    fn joystick(&mut self, kind: u16, code: u16, value: i32) -> Vec<(u16, bool)> {
        if kind != EV_KEY {
            return Vec::new();
        }
        match (code, value) {
            (SELECT, 1) => {
                self.select = Some(self.scrolls);
                Vec::new()
            }
            (SELECT, 0) => match self.select.take() {
                Some(at) if at == self.scrolls => vec![(F21, true), (F21, false)],
                _ => Vec::new(),
            },
            (_, 0 | 1) => KEYS
                .iter()
                .find(|(button, _)| *button == code)
                .map(|&(_, key)| vec![(key, value == 1)])
                .unwrap_or_default(),
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_b_and_y_are_the_launcher_the_switcher_and_close() {
        let mut buttons = Buttons::default();
        let a = BTN_JOYSTICK + 1;
        assert_eq!(buttons.joystick(EV_KEY, a, 1), [(192, true)], "A is Start");
        assert_eq!(buttons.joystick(EV_KEY, a, 0), [(192, false)]);
        let b = BTN_JOYSTICK + 2;
        assert_eq!(buttons.joystick(EV_KEY, b, 1), [(F21, true)], "B is Select");
        let y = BTN_JOYSTICK + 3;
        assert_eq!(buttons.joystick(EV_KEY, y, 1), [(183, true)], "Y is L");
        assert_eq!(buttons.joystick(EV_KEY, BTN_JOYSTICK + 9, 1), [(192, true)]);
    }

    #[test]
    fn select_is_a_tap_on_release_unless_the_trackball_scrolled() {
        let mut buttons = Buttons::default();
        assert!(buttons.joystick(EV_KEY, SELECT, 1).is_empty());
        assert_eq!(
            buttons.joystick(EV_KEY, SELECT, 0),
            [(F21, true), (F21, false)]
        );

        assert!(buttons.joystick(EV_KEY, SELECT, 1).is_empty());
        buttons.mouse(EV_REL, REL_WHEEL);
        assert!(
            buttons.joystick(EV_KEY, SELECT, 0).is_empty(),
            "Select held to scroll is not the switcher"
        );
    }

    #[test]
    fn finds_the_joystick_and_grabs_it_only_when_it_has_no_letters() {
        let root = std::env::temp_dir().join(format!("pt35-gamepad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let device = |event: &str, files: &[(&str, &str)]| {
            let dir = root.join(event).join("device");
            std::fs::create_dir_all(dir.join("id")).unwrap();
            std::fs::create_dir_all(dir.join("capabilities")).unwrap();
            for (file, text) in files {
                std::fs::write(dir.join(file), text).unwrap();
            }
        };
        let ids = [("id/vendor", "1eaf\n"), ("id/product", "0024\n")];
        // KEY_A is bit 30 of the lowest word; BTN_JOYSTICK, 0x120, is bit 32
        // of the fifth.
        device(
            "event2",
            &[
                ids[0],
                ids[1],
                ("capabilities/key", "ffffffff 0 0 0 40000000\n"),
            ],
        );
        device(
            "event3",
            &[
                ids[0],
                ids[1],
                ("capabilities/key", "3ff00000000 0 0 0 0\n"),
            ],
        );
        device("event4", &[ids[0], ids[1], ("capabilities/rel", "143\n")]);
        device(
            "event5",
            &[("id/vendor", "1209\n"), ("id/product", "0001\n")],
        );
        assert_eq!(
            find(&root),
            Some(Found {
                joystick: PathBuf::from("/dev/input/event3"),
                mouse: Some(PathBuf::from("/dev/input/event4")),
                grab: true,
            })
        );
        std::fs::remove_dir_all(&root).unwrap();
    }
}
