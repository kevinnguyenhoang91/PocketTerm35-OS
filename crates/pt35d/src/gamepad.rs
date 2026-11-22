//! The uConsole's face buttons.
//!
//! sway binds only keys, and no firmware sends the face buttons as keys sway
//! has a use for. This thread reads them and presses a key the pt35 firmware
//! sends on a uinput keyboard of its own, so the PocketTerm35's bindings serve
//! both: A is F22 (Start, the launcher), B is F21 (Select, the window
//! switcher) and Y is F13 (L, close the window). X opens the Monitor app
//! itself, through the session: a sway binding of its own on F19 never fired
//! on a CM4 unit, and a launch from here needs no binding at all. The
//! uConsole's L and R are the trackball's mouse buttons, so no face button
//! has to stand for them.
//!
//! Two keyboard firmwares are about, and they differ in what the buttons send:
//!
//! - ClockworkPi's own (`1eaf:0024`): A B X Y, Select and Start are joystick
//!   buttons. The joystick is grabbed: a game would get both otherwise.
//! - j1n6's QMK firmware (`feed:0000`, "Clockwork uConsole Keyboard"): from
//!   v1.9, keyboard keys on the keyboard device (Execute, Stop, System
//!   Request, Menu and Select; Start is Super). That device carries the
//!   letters too, so it is read and not grabbed: an app also sees those keys,
//!   and none uses them. Before v1.9, and in its gamepad layer (Fn+G),
//!   joystick buttons 1 to 6.
//!
//! Select goes out when it is let go, and only if the trackball did not
//! scroll meanwhile: every firmware turns the trackball into a wheel while
//! Select is held. QMK from v1.9 does that itself, and sends the press and
//! release together, which reads the same here.

use std::fs::File;
use std::io::Read;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pt35_common::ipc::{Event, Request, Response};

use crate::hardware::bitmap_has;
use crate::server::Subscribers;
use crate::state::Session;
use crate::vkbd::FaceKeys;

const EV_KEY: u16 = 1;
const EV_REL: u16 = 2;
const REL_HWHEEL: u16 = 6;
const REL_WHEEL: u16 = 8;
const REL_WHEEL_HI_RES: u16 = 11;
const REL_HWHEEL_HI_RES: u16 = 12;

const KEY_A: u16 = 30;
const KEY_STOP: u16 = 128;
const KEY_PROPS: u16 = 130;
const KEY_FRONT: u16 = 132;
const KEY_OPEN: u16 = 134;
/// What Linux makes of a key it has no code for: QMK's System Request.
const KEY_UNKNOWN: u16 = 240;
/// HID button n of a joystick arrives as `BTN_JOYSTICK + n - 1`.
const BTN_JOYSTICK: u16 = 0x120;

/// The pt35 firmware's keys, which the shell binds.
const F13: u16 = 183;
const F21: u16 = 191;
const F22: u16 = 192;

/// What a button does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Out {
    /// Hold this key down for as long as the button is.
    Key(u16),
    /// Open the Monitor app, or bring it to the front, on the press.
    Monitor,
}

use Out::{Key, Monitor};

/// One way the buttons arrive: event code to what it does.
#[derive(Debug, PartialEq, Eq)]
struct Layout {
    keys: &'static [(u16, Out)],
    /// Sent on release, and only if the trackball did not scroll: Select,
    /// which the keyboard turns into a wheel modifier while it is held.
    select: Option<u16>,
}

/// ClockworkPi's firmware: joystick buttons 1 to 4 are X A B Y, 9 and 10 are
/// Select and Start.
const STOCK_JOYSTICK: Layout = Layout {
    keys: &[
        (BTN_JOYSTICK, Monitor),
        (BTN_JOYSTICK + 1, Key(F22)),
        (BTN_JOYSTICK + 2, Key(F21)),
        (BTN_JOYSTICK + 3, Key(F13)),
        (BTN_JOYSTICK + 9, Key(F22)),
    ],
    select: Some(BTN_JOYSTICK + 8),
};

/// j1n6's QMK firmware before v1.9, and its gamepad layer: joystick buttons
/// 1 to 6 are A B X Y Select Start. Before v1.9 Select goes down on the press
/// and stays down while the trackball scrolls, so it waits for the release
/// as ClockworkPi's does. From v1.9 the firmware holds it back itself.
const QMK_JOYSTICK: Layout = Layout {
    keys: &[
        (BTN_JOYSTICK, Key(F22)),
        (BTN_JOYSTICK + 1, Key(F21)),
        (BTN_JOYSTICK + 2, Monitor),
        (BTN_JOYSTICK + 3, Key(F13)),
        (BTN_JOYSTICK + 5, Key(F22)),
    ],
    select: Some(BTN_JOYSTICK + 4),
};

/// j1n6's QMK firmware from v1.9: HID Execute, Stop, System Request and Menu
/// for A B X Y, which Linux reads as Open, Stop, an unknown key and Props, and
/// Select as Front. Start is Super, and stays it.
const QMK_KEYS: Layout = Layout {
    keys: &[
        (KEY_OPEN, Key(F22)),
        (KEY_STOP, Key(F21)),
        (KEY_UNKNOWN, Monitor),
        (KEY_PROPS, Key(F13)),
        (KEY_FRONT, Key(F21)),
    ],
    select: None,
};

/// Which firmware the keyboard runs, by its USB ids.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Firmware {
    ClockworkPi,
    Qmk,
}

fn firmware(vendor: &str, product: &str, name: &str) -> Option<Firmware> {
    match (vendor, product) {
        // USBComposite's default ids, which ClockworkPi's firmware keeps.
        ("1eaf", "0024") => Some(Firmware::ClockworkPi),
        // QMK's default ids, which many keyboards keep: the name says which.
        ("feed", "0000") if name.contains("uConsole") => Some(Firmware::Qmk),
        _ => None,
    }
}

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

pub fn spawn(session: Arc<Mutex<Session>>, subscribers: Arc<Subscribers>) {
    let spawned = std::thread::Builder::new()
        .name("gamepad".into())
        .spawn(move || run(&session, &subscribers));
    if let Err(e) = spawned {
        log::warn!("no uConsole face buttons: {e}");
    }
}

/// The Monitor app, as if picked in the launcher. A failure is a toast, as
/// any request's is.
fn open_monitor(session: &Mutex<Session>, subscribers: &Subscribers) {
    let mut session = session.lock().expect("session");
    if let Response::Error { message } = session.handle(Request::Launch {
        app: "monitor".into(),
    }) {
        log::warn!("X: {message}");
    }
    subscribers.broadcast(&Event::Status(session.status.clone()));
    for note in &session.take_notifications() {
        subscribers.broadcast(note);
    }
}

fn run(session: &Mutex<Session>, subscribers: &Subscribers) {
    // Made before the buttons are found: sway takes a moment to adopt a new
    // keyboard, and the first press must not be the one that is lost.
    let mut keys = match FaceKeys::open() {
        Ok(keys) => keys,
        Err(e) => {
            log::warn!("/dev/uinput: {e}; the uConsole's face buttons do nothing");
            return;
        }
    };
    let monitor = || open_monitor(session, subscribers);
    let mut missing_said = false;
    loop {
        match find(Path::new("/sys/class/input")) {
            Some(found) => {
                missing_said = false;
                if let Err(e) = serve(&found, &mut keys, &monitor) {
                    log::info!("uConsole face buttons: {e}");
                }
            }
            None if !missing_said => {
                log::warn!(
                    "no uConsole face buttons (1eaf:0024, or feed:0000 named uConsole); \
                     looking again every 3s"
                );
                missing_said = true;
            }
            None => {}
        }
        std::thread::sleep(RESCAN);
    }
}

/// An event device the buttons arrive on.
#[derive(Debug, PartialEq, Eq)]
struct Source {
    path: PathBuf,
    /// Only a device with nothing but the buttons on it: a grab would take the
    /// letters away from sway.
    grab: bool,
    layout: &'static Layout,
}

#[derive(Debug, PartialEq, Eq)]
struct Found {
    sources: Vec<Source>,
    /// The trackball, to see it scroll while Select is held.
    mouse: Option<PathBuf>,
}

/// The uConsole keyboard's button and mouse devices, under `class`
/// (`/sys/class/input`).
fn find(class: &Path) -> Option<Found> {
    let mut sources = Vec::new();
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
        let Some(firmware) = firmware(
            read("id/vendor").trim(),
            read("id/product").trim(),
            read("name").trim(),
        ) else {
            continue;
        };
        let path = Path::new("/dev/input").join(event.file_name()?);
        let keys = read("capabilities/key");
        let has = |code: u16| bitmap_has(&keys, code as usize);
        if has(BTN_JOYSTICK) {
            sources.push(Source {
                path,
                grab: !has(KEY_A),
                layout: match firmware {
                    Firmware::ClockworkPi => &STOCK_JOYSTICK,
                    Firmware::Qmk => &QMK_JOYSTICK,
                },
            });
        } else if firmware == Firmware::Qmk && has(KEY_A) && has(KEY_OPEN) {
            sources.push(Source {
                path,
                grab: false,
                layout: &QMK_KEYS,
            });
        } else if mouse.is_none() && bitmap_has(&read("capabilities/rel"), REL_WHEEL as usize) {
            mouse = Some(path);
        }
    }
    (!sources.is_empty()).then_some(Found { sources, mouse })
}

/// Turn button events into key events until a device goes away.
fn serve(found: &Found, keys: &mut FaceKeys, monitor: &dyn Fn()) -> std::io::Result<()> {
    let mut files = Vec::with_capacity(found.sources.len());
    for source in &found.sources {
        let file = File::open(&source.path)?;
        // SAFETY: EVIOCGRAB takes an int by value.
        if source.grab && unsafe { ioctl(file.as_raw_fd(), EVIOCGRAB, 1) } < 0 {
            return Err(std::io::Error::last_os_error());
        }
        files.push(file);
    }
    // Without the trackball Select still works; it only cannot tell a scroll.
    let mut mouse = found.mouse.as_ref().and_then(|path| File::open(path).ok());
    let paths: Vec<String> = found
        .sources
        .iter()
        .map(|s| s.path.display().to_string())
        .collect();
    log::info!(
        "uConsole face buttons on {}: they are keys",
        paths.join(", ")
    );
    let mut buttons = Buttons::default();
    let mut buf = [0u8; EVENT * 16];
    loop {
        let mut fds: Vec<PollFd> = files
            .iter()
            .chain(mouse.iter())
            .map(|file| PollFd {
                fd: file.as_raw_fd(),
                events: POLLIN,
                revents: 0,
            })
            .collect();
        // SAFETY: `fds` is a live array of `fds.len()` pollfd structs.
        if unsafe { poll(fds.as_mut_ptr(), fds.len() as u64, -1) } < 0 {
            let e = std::io::Error::last_os_error();
            if e.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(e);
        }
        if fds.get(files.len()).is_some_and(|fd| fd.revents != 0) {
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
        for (index, file) in files.iter_mut().enumerate() {
            if fds[index].revents == 0 {
                continue;
            }
            let n = file.read(&mut buf)?;
            if n == 0 {
                return Err(std::io::ErrorKind::UnexpectedEof.into());
            }
            let layout = found.sources[index].layout;
            for (kind, code, value) in events(&buf[..n]) {
                for (out, down) in buttons.press(layout, kind, code, value) {
                    match out {
                        Key(key) => keys.send(key, down)?,
                        Monitor if down => monitor(),
                        Monitor => {}
                    }
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

    /// What one event does. A key's repeats (value 2) are dropped: the
    /// shell's bindings do not repeat.
    fn press(&mut self, layout: &Layout, kind: u16, code: u16, value: i32) -> Vec<(Out, bool)> {
        if kind != EV_KEY {
            return Vec::new();
        }
        if layout.select == Some(code) {
            return match value {
                1 => {
                    self.select = Some(self.scrolls);
                    Vec::new()
                }
                0 => match self.select.take() {
                    Some(at) if at == self.scrolls => vec![(Key(F21), true), (Key(F21), false)],
                    _ => Vec::new(),
                },
                _ => Vec::new(),
            };
        }
        if !matches!(value, 0 | 1) {
            return Vec::new();
        }
        layout
            .keys
            .iter()
            .find(|(button, _)| *button == code)
            .map(|&(_, out)| vec![(out, value == 1)])
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A, B, X and Y on each layout, in that order.
    fn abxy(layout: &Layout) -> Vec<Out> {
        let mut buttons = Buttons::default();
        let codes: &[u16] = match layout {
            l if *l == STOCK_JOYSTICK => &[
                BTN_JOYSTICK + 1,
                BTN_JOYSTICK + 2,
                BTN_JOYSTICK,
                BTN_JOYSTICK + 3,
            ],
            l if *l == QMK_JOYSTICK => &[
                BTN_JOYSTICK,
                BTN_JOYSTICK + 1,
                BTN_JOYSTICK + 2,
                BTN_JOYSTICK + 3,
            ],
            _ => &[KEY_OPEN, KEY_STOP, KEY_UNKNOWN, KEY_PROPS],
        };
        codes
            .iter()
            .map(|&code| {
                let down = buttons.press(layout, EV_KEY, code, 1);
                assert_eq!(buttons.press(layout, EV_KEY, code, 0).len(), 1);
                assert_eq!(down.len(), 1, "code {code} sends nothing");
                down[0].0
            })
            .collect()
    }

    #[test]
    fn a_b_x_y_are_the_launcher_switcher_monitor_and_close_on_every_firmware() {
        for layout in [&STOCK_JOYSTICK, &QMK_JOYSTICK, &QMK_KEYS] {
            assert_eq!(
                abxy(layout),
                [Key(F22), Key(F21), Monitor, Key(F13)],
                "{layout:?}"
            );
        }
    }

    #[test]
    fn a_key_held_down_does_not_repeat_its_binding() {
        let mut buttons = Buttons::default();
        assert!(buttons.press(&QMK_KEYS, EV_KEY, KEY_OPEN, 2).is_empty());
    }

    #[test]
    fn select_is_a_tap_on_release_unless_the_trackball_scrolled() {
        for (layout, select) in [
            (&STOCK_JOYSTICK, BTN_JOYSTICK + 8),
            (&QMK_JOYSTICK, BTN_JOYSTICK + 4),
        ] {
            let mut buttons = Buttons::default();
            assert!(
                buttons.press(layout, EV_KEY, select, 1).is_empty(),
                "{layout:?}: the switcher must not open on the press"
            );
            assert_eq!(
                buttons.press(layout, EV_KEY, select, 0),
                [(Key(F21), true), (Key(F21), false)]
            );

            assert!(buttons.press(layout, EV_KEY, select, 1).is_empty());
            buttons.mouse(EV_REL, REL_WHEEL);
            assert!(
                buttons.press(layout, EV_KEY, select, 0).is_empty(),
                "{layout:?}: Select held to scroll is not the switcher"
            );
        }
    }

    fn sysfs(name: &str, devices: &[(&str, &[(&str, &str)])]) -> PathBuf {
        let root = std::env::temp_dir().join(format!("pt35-gamepad-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (event, files) in devices {
            let dir = root.join(event).join("device");
            std::fs::create_dir_all(dir.join("id")).unwrap();
            std::fs::create_dir_all(dir.join("capabilities")).unwrap();
            for (file, text) in *files {
                std::fs::write(dir.join(file), text).unwrap();
            }
        }
        root
    }

    #[test]
    fn finds_the_stock_joystick_and_grabs_it_only_when_it_has_no_letters() {
        let ids = [("id/vendor", "1eaf\n"), ("id/product", "0024\n")];
        // KEY_A is bit 30 of the lowest word; BTN_JOYSTICK, 0x120, is bit 32
        // of the fifth.
        let root = sysfs(
            "stock",
            &[
                (
                    "event2",
                    &[
                        ids[0],
                        ids[1],
                        ("capabilities/key", "ffffffff 0 0 0 40000000\n"),
                    ],
                ),
                (
                    "event3",
                    &[
                        ids[0],
                        ids[1],
                        ("capabilities/key", "3ff00000000 0 0 0 0\n"),
                    ],
                ),
                ("event4", &[ids[0], ids[1], ("capabilities/rel", "143\n")]),
                (
                    "event5",
                    &[("id/vendor", "1209\n"), ("id/product", "0001\n")],
                ),
            ],
        );
        assert_eq!(
            find(&root),
            Some(Found {
                sources: vec![Source {
                    path: PathBuf::from("/dev/input/event3"),
                    grab: true,
                    layout: &STOCK_JOYSTICK,
                }],
                mouse: Some(PathBuf::from("/dev/input/event4")),
            })
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// The devices of a uConsole on j1n6's QMK firmware, as its sysfs has them.
    #[test]
    fn finds_both_keyboards_and_the_joystick_on_qmk_firmware() {
        let qmk = |name: &'static str, key: &'static str| -> Vec<(&'static str, &'static str)> {
            vec![
                ("id/vendor", "feed\n"),
                ("id/product", "0000\n"),
                ("name", name),
                ("capabilities/key", key),
            ]
        };
        let keyboard = qmk(
            "Clockwork uConsole Keyboard\n",
            "1000000000007 ff9f207ac14057ff febeffdfffefffff fffffffffffffffe\n",
        );
        let mut mouse = qmk("Clockwork uConsole Keyboard Mouse\n", "ff0000 0 0 0 0\n");
        mouse.push(("capabilities/rel", "1943\n"));
        let joystick = qmk("Clockwork uConsole Keyboard\n", "3f00000000 0 0 0 0\n");
        let consumer = qmk(
            "Clockwork uConsole Keyboard Consumer Control\n",
            "733fff 0 0 483ffff17aff32d bfd4444600000000 1 130ff38b17c000 677bfad9415fed 19ed68000004400 10000002\n",
        );
        let nkro = qmk(
            "Clockwork uConsole Keyboard\n",
            "1000000000007 ff98007a000007ff febeffdfffefffff fffffffffffffffe\n",
        );
        let other_qmk = [
            ("id/vendor", "feed\n"),
            ("id/product", "0000\n"),
            ("name", "Some Other Keyboard\n"),
            ("capabilities/key", "3f00000000 0 0 0 0\n"),
        ];
        let root = sysfs(
            "qmk",
            &[
                ("event0", &keyboard),
                ("event1", &mouse),
                ("event2", &joystick),
                ("event4", &consumer),
                ("event5", &nkro),
                ("event6", &other_qmk),
            ],
        );
        let source = |event: &str, grab, layout| Source {
            path: PathBuf::from(format!("/dev/input/{event}")),
            grab,
            layout,
        };
        assert_eq!(
            find(&root),
            Some(Found {
                sources: vec![
                    source("event0", false, &QMK_KEYS),
                    source("event2", true, &QMK_JOYSTICK),
                    source("event5", false, &QMK_KEYS),
                ],
                mouse: Some(PathBuf::from("/dev/input/event1")),
            })
        );
        std::fs::remove_dir_all(&root).unwrap();
    }
}
