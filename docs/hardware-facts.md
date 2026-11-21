# Hardware facts

What the shell assumes about each device. To check a unit of your own, run
the probe and compare:

```sh
sudo /usr/share/pt35-desktop/pt35-probe.sh > /tmp/probe.md 2>&1
```

`/usr/lib/pt35/pt35-device` says which device this is. The [uConsole](#clockworkpi-uconsole)
is below the PocketTerm35.

# PocketTerm35

Captured on a Pi 5 board, 2026-09-22.

## Verified

### Display: plain HDMI, no overlay needed
The 3.5" 640×480 IPS panel is driven over HDMI and comes up with the stock
`vc4-kms-v3d` driver. None of the three Waveshare overlays contains a display
node; they are touchscreen-only. Nothing in `config.txt` is needed for the
picture.

### GPU: vc4 shows, v3d renders
Captured on the Pi 5, 2026-11-14:

| node | driver | job |
|---|---|---|
| `card0`, `renderD128` | `v3d` | the GPU |
| `card1` | `vc4-drm` | the display (KMS), no render node |

wlroots renders through the display device unless told otherwise, and tells
clients to render there too. vc4 has no render node, so Wayland GL apps and
Xwayland fell back to llvmpipe (#9). `pt35-session` sets
`WLR_RENDER_DRM_DEVICE` to the render node whose driver is `v3d`. Buffers for
the panel still come from vc4. The `v3d` module also binds `brcm,2711-v3d`,
the Pi 4's GPU. The first line of `$XDG_RUNTIME_DIR/pt35/sway.log` names the
node in use.

### Touchscreen: Goodix GT911 on I²C-1, address 0x5d
Decompiling the shipped overlays (see `boot/README.md`):

| overlay | GT911 nodes declared |
|---|---|
| `waveshare-35dpi-3b.dtbo` | `0x14` |
| `waveshare-35dpi-4b.dtbo` | `0x14` |
| `waveshare-35dpi-5b.dtbo` | `0x14` **and `0x5d`**, plus `irq-gpios` |

PocketTerm35 units answer at `0x5d`, so **`waveshare-35dpi-5b.dtbo` is the right
overlay on both the Pi 4B and the Pi 5**. The split is by I²C address, not by
board. Both nodes declare `touchscreen-size-x = 0x280` (640) and
`touchscreen-size-y = 0x1e0` (480).

### Keyboard: USB HID behind an RP2040
The keyboard, D-pad and gaming buttons hang off an on-board RP2040 that presents
itself over the Pi's USB OTG port, which is why `dtoverlay=dwc2,dr_mode=host` is
required. The same MCU drives the backlight (a PWM pin, GP20) and the speaker
gain, so there is no `/sys/class/backlight`.

`1209:0001` ("My Company My Custom Pico") declares five interfaces: CDC ACM
(`/dev/ttyACM0`), one HID, and two audio. The ACM port is the CircuitPython
console: `pt35-kbd` flashes through it, and the patched firmware takes
brightness commands on it (below). Linux sees:

```
event1  My Company My Custom Pico Keyboard   sysrq kbd leds
event3  My Company My Custom Pico Mouse      5 buttons
```

**There is no gamepad.** Everything with a key on it arrives on `event1`: the
67-key QWERTY, the D-pad and the face buttons, one device, one keymap. Stock,
the A button and the `a` key send the same `KEY_A`, so no remapper (keyd
included) can separate them: keyd binds per device, and this is one device.

The firmware is **CircuitPython 10.0.0-beta.0** with `boot.py` calling
`storage.disable_usb_drive()`, which is why no CIRCUITPY drive appears. `code.py`
holds the matrix as a plain table, and `firmware/` changes the face-button row to
F13-F18. See `firmware/README.md`.

Adafruit's HID library has two names. A unit built on CircuitPython of
2026-08-21 has it as `/lib/hid`, and `import adafruit_hid` fails. One built on
2025-07-23 has `/lib/adafruit_hid` (issue #8). `boot_out.txt` gives the date.
Both firmware files try `adafruit_hid` first, then `hid`.

Matrix: rows `GP16 GP10 GP11 GP12 GP13 GP14 GP15`, columns `GP0`-`GP9`. Row 0 is
the D-pad and the face buttons. Row 6 carries Fn, Ctrl, Alt, Select, Space,
Start, right Alt and **Super** (`WINDOWS`), so the unit does have a Super key.
Stock, Select is `PRINT_SCREEN` and Start is `PAUSE`. The patched firmware
sends F21 and F22 for them and keeps Print and Pause on Fn.

### Network: Ethernet and Wi-Fi
The Pi's own `eth0` (RJ45 on the board) and `wlan0`, both under
NetworkManager. With no cable `eth0` is `down` and nmcli calls it
`unavailable`. `/proc/net/wireless` lists only the radio, so a wired link has
no signal reading. A cable with link carries the traffic when both are up.

### Boot configuration
```
dtparam=i2c_arm=on
dtoverlay=waveshare-35dpi-5b
dtoverlay=dwc2,dr_mode=host
```
On Trixie this file is `/boot/firmware/config.txt`, not `/boot/config.txt`.

### Software base
Raspberry Pi OS **Trixie** 64-bit. `sway` must be the Raspberry Pi rebuild
(`1.10.1-2+rpt1` or later): the Debian build fails against the Pi's patched
`libwlroots-0.18`. `seatd` is required. Pi OS Lite ships no audio stack, so
the package recommends PipeWire and `install.sh` installs it.

### Brightness over serial
The patched firmware reads `B<0-100>` and `B?` lines on `/dev/ttyACM0` and
answers `PT35 B=<n>`, with a 5% floor. `pt35d` drives it, and the answer is also
how the shell knows the patched firmware is on. Stock firmware never answers:
brightness is then Fn with `-` and `=` only.

### Battery
There is no `/sys/class/power_supply` and no gauge on I²C. The bar shows no
battery.

### Still open
- Where audio comes out: HDMI through the panel board, or a separate DAC.
  Volume works through PipeWire either way.

# ClockworkPi uConsole

Read from ak-rex's "ClockworkPi Debian 13 Trixie lite" image (akrex kernel
7.1.4, built 2026-07-30) and from ClockworkPi's keyboard firmware source
(`clockworkpi/uConsole`, `Code/uconsole_keyboard`). Not yet checked on a unit.

### Software base
The image is Raspberry Pi OS Trixie Lite (`pi-gen`, stage2) with ak-rex's
kernel and apt repo. `archive.raspberrypi.com` is in its sources, so sway is
the `+rpt` build, as on the PocketTerm35. `/etc/os-release` says
`VERSION_CODENAME=trixie`. The board model reads "Raspberry Pi Compute Module
4" or "5". The CM5 boots `kernel8.img` (4K pages).

### How the shell knows it
`config.txt` loads `clockworkpi-uconsole-cm4` (or `-cm4s`, `-cm5`). Each puts a
DSI panel with `compatible = "cw,cwu50"` in the device tree. `pt35-device`
looks for that string under `/proc/device-tree`. The DevTerm's panel is
`cw,cwd686`, which is not supported.

### Display: 720x1280 DSI panel, turned
The panel is portrait, `cw,cwu50`, with `rotation = <90>` in the overlay. Its
driver reads the rotation into the DRM "panel orientation" property, and sway
1.10 applies that when the config gives no `transform`. So the desktop is
1280x720 with no transform set. The console has `fbcon=rotate:1` for the same
reason. The output is `DSI-1` on a CM4 and `DSI-2` on a CM5, where the panel
hangs off RP1. The panel is 5", about 11.6 px/mm, so `pt35/uconsole/theme.toml`
makes every size 1.3 times the PocketTerm35's.

### Keyboard: STM32, two firmwares
The keyboard runs one of two firmwares, and the face buttons differ:

- **ClockworkPi's own.** USB `1eaf:0024`, "ClockworkPI uConsole":
  USBComposite's default ids. One HID interface with four application
  collections, which Linux splits into Keyboard, Mouse, Consumer Control and
  a joystick, plus a CDC serial port (`/dev/ttyACM0`) that the firmware
  never reads. The table below is this firmware.
- **j1n6's QMK firmware** ([qmk-uconsole](https://github.com/j1n6/qmk-uconsole)).
  USB `feed:0000`, "Clockwork uConsole Keyboard": QMK's default ids, so the
  name is what tells it from another QMK keyboard. Seen on a CM4 unit,
  2026-11-22: two keyboard devices (6KRO and NKRO), a Mouse, System Control,
  Consumer Control and a joystick with six buttons. From v1.9, A B X Y send
  HID Execute, Stop, System Request and Menu on the keyboard device, which
  Linux reads as `KEY_OPEN`, `KEY_STOP`, `KEY_UNKNOWN` and `KEY_PROPS`.
  Select sends `KEY_FRONT` and Start is Super. Before v1.9, and in its Fn+G
  gamepad layer, A B X Y Select Start are joystick buttons 1 to 6
  (`BTN_TRIGGER` to `BTN_PINKIE`), and Fn+G puts the D-pad on the joystick's
  axes. It does Select's tap-on-release itself. evtest on that CM4 unit's
  joystick: A `BTN_TRIGGER`, B `BTN_THUMB`, X `BTN_THUMB2`, Y `BTN_TOP`.

| control | sends |
|---|---|
| D-pad | arrow keys (Fn: Page Up, Page Down, Home, End) |
| A B X Y | joystick buttons 2 3 1 4: `BTN_THUMB`, `BTN_THUMB2`, `BTN_TRIGGER`, `BTN_TOP` |
| Select, Start | joystick buttons 9 and 10: `BTN_BASE3`, `BTN_BASE4` (Fn: Print, Pause) |
| trackball | mouse; a wheel while Select is held |
| L, R | left and right mouse buttons, on the Mouse device (`BTN_LEFT`, `BTN_RIGHT`) |
| Fn + `,` `.` | brightness down, up (consumer keys) |
| Fn + Alt | Super |

L and R are the trackball's mouse buttons, not gamepad buttons, so they are
clicks and nothing can bind them. Switch 2 on the back turns ClockworkPi's
buttons into letters: A B X Y type `j k u i`, Select is Space and Start is
Enter. The shell needs it off. `pt35d`'s `gamepad.rs` knows both firmwares.
It grabs a joystick and reads the QMK keyboard devices without a grab, and
presses pt35 firmware keys for the buttons on a uinput keyboard. With
ClockworkPi's firmware, Select goes out on release, and not at all when the
trackball scrolled meanwhile.

### Backlight
`ocp8178-backlight`, in `/sys/class/backlight`. `max_brightness` is 9. A
level is a table entry, so `pt35d` moves at least one level per step.
`70-pt35-backlight.rules` gives `video` write access.

### Battery
An AXP228 PMIC on I2C: `/sys/class/power_supply/axp20x-battery`, with
`capacity` and `status`. The bar shows it. The image's
`99-uconsole-charging.rules` sets the charge current.

### Audio
PWM audio on GPIO 12 and 13 (`audremap`). ak-rex's
`clockworkpi-audio-patch.service` switches the speaker off when headphones
are in. Volume works through PipeWire.

### Touch and network
No touchscreen: the Touch row in Quick settings is hidden. Wi-Fi is the
module's own `wlan0`. There is no Ethernet port.

### Still open
- Whether the cwu50 driver sets the panel orientation property on the CM5. If
  the screen comes up sideways, add `output DSI-2 transform 270` to
  `~/.config/sway/config`.
- Whether rendering on v3d with the panel on RP1 (CM5) costs a copy per frame.
