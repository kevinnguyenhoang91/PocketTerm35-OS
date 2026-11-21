# Install on a uConsole with a CM4

pt35-desktop on a ClockworkPi uConsole with a Raspberry Pi CM4, on ak-rex's
Debian 13 image. Facts about the hardware are in
[hardware-facts.md](hardware-facts.md#clockworkpi-uconsole), and the
controls are in [KEYS.md](KEYS.md#uconsole).

You need:

- a uConsole with a CM4 (Wi-Fi model)
- a microSD card of 8 GB or more, and a computer to write it
- a Wi-Fi network

## 1. Write the image

Download the **lite** image from <https://images.ak-rex.com/#clockworkpi>,
under "ClockworkPi uConsole/DevTerm", "Debian 13 Trixie":

```sh
curl -LO https://images.ak-rex.com/download/ClockworkPi-Debian-13-Trixie-lite-7.1.4.img.xz
```

The desktop image works too, but pt35-desktop replaces its desktop. Lite is
smaller and boots faster.

Write it with [Raspberry Pi Imager](https://www.raspberrypi.com/software/):
"Choose OS", then "Use custom", and pick the `.img.xz`. In its settings, set a
user name and password, your Wi-Fi and your time zone, and turn on SSH. The
installer sets up the session for that user.

Or write it with `dd`, where `/dev/sdX` is the card:

```sh
xzcat ClockworkPi-Debian-13-Trixie-lite-7.1.4.img.xz | sudo dd of=/dev/sdX bs=4M status=progress conv=fsync
```

Then the first boot asks for a user name and password on the screen.

Leave `config.txt` on the card as it is. Its `[cm4]` section already loads
`clockworkpi-uconsole-cm4`, which is the screen, the keyboard, the backlight
and the battery.

## 2. First boot

Put the card in the uConsole and turn it on. The console is rotated, so the
text reads sideways until the desktop is installed.

Log in. If Imager did not set up Wi-Fi, join it now:

```sh
sudo nmtui
```

Check the board and the network:

```sh
tr -d '\0' < /proc/device-tree/model; echo   # Raspberry Pi Compute Module 4 ...
ping -c1 deb.debian.org
```

From here on, SSH from another computer is easier than the small keyboard:
`ssh <user>@<uconsole-ip>`.

## 3. Install pt35-desktop

The first release with uConsole support is not out yet. Until it is, the
installer builds the branch that has it, on the device, so it needs Rust:

```sh
sudo apt-get update && sudo apt-get install -y git curl
curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
. "$HOME/.cargo/env"

REPO=kevinnguyenhoang91/PocketTerm35-OS
BRANCH='feat/Migrate-to-uConsole-running-Debian-13-with-image-https/images.ak-rex.com/clockworkpi'
curl -fsSL "https://raw.githubusercontent.com/$REPO/refs/heads/$BRANCH/install.sh" \
  | sudo env PT35_REPO="$REPO" bash -s -- --branch "$BRANCH"
```

`--branch` clones that branch of `PT35_REPO` and builds it. From a clone of
your own, `git switch` to the branch and run `sudo ./install.sh --from-source`
instead.

The build takes 20 to 40 minutes on a CM4. Keep the uConsole on its charger.
Add `--no-ai` to skip the question about the local AI agent; a CM4 with 4 GB
or less is better off without it.

Once a release has uConsole support, the one command in the
[README](../README.md#install) does the same with no build:

```sh
curl -fsSL https://raw.githubusercontent.com/therezor/PocketTerm35-OS/main/install.sh | sudo bash
```

The installer:

- installs sway, the shell and their dependencies
- leaves `config.txt` alone: on a uConsole there is nothing to add
- sets up greetd to log you straight in to the desktop
- adds udev rules for the face buttons (uinput) and the backlight
- adds your user to `input`, `video`, `render` and `audio`

Do **not** run `pt35-kbd flash`. That is the PocketTerm35's keyboard. The
uConsole's keyboard works with its own firmware, and `pt35-kbd` refuses to
run here.

## 4. Reboot

```sh
sudo reboot
```

You land on the launcher, in landscape, with the bar at the top and the
battery in it.

Before you start, check **switch 2 on the back of the keyboard is off**.
When it is on, A B X Y type `j k u i` and the shell cannot use them as
buttons.

## 5. Check it

Open a terminal (Super+Enter, which is Fn+Alt+Enter, or Alt+Enter) and run:

```sh
/usr/lib/pt35/pt35-device                # uconsole
swaymsg -t get_outputs | grep -E '"name"|"transform"'   # DSI-1, 270
pt35ctl status --json | grep -oE '"(keypad|battery_percent|brightness_percent)":[^,]*'
```

Expect `"keypad":"uconsole"`, a battery level and a brightness level.

Then try the controls:

| try | expect |
|---|---|
| A | the launcher opens; again over an app, it closes |
| B | the window switcher |
| Y | the window in front closes |
| X | btop opens, or comes to the front |
| Start | the launcher opens and closes |
| Select, let go | the window switcher |
| hold Select, roll the trackball | it scrolls, no switcher |
| Fn + `,` and Fn + `.` | brightness down and up |
| Fn + Select | a screenshot in `~/Pictures` |

## If something is wrong

| problem | fix |
|---|---|
| the screen is sideways | add `output DSI-1 transform 270` to `~/.config/sway/config`, then Super+Shift+c |
| A B X Y do nothing | switch 2 on the back must be off. Then see the log below |
| A B X Y type `j k u i` | switch 2 on the back is on |
| brightness says "not writable" | log out and in again, so your user is in `video` |
| text too big or too small | set sizes in `~/.config/pt35/theme.toml`, see [theme.toml](../config/pt35/uconsole/theme.toml) |
| no desktop, a text login | `sudo systemctl status greetd`, and `cat /run/user/$(id -u)/pt35/sway.log` |

`pt35d` logs to sway's log, under the runtime folder:

```sh
grep -i 'uconsole joystick' /run/user/$(id -u)/pt35/sway.log
```

"the face buttons are keys" means they work. "no uConsole joystick" means
the keyboard was not found: run `sudo evtest` and look for "ClockworkPI
uConsole".

To send a report, attach the probe:

```sh
sudo /usr/share/pt35-desktop/pt35-probe.sh > ~/probe.md 2>&1
```

## Update and remove

Update: run the install command from step 3 again. It builds the branch as
it is now.

Remove, which gives back the image's own login:

```sh
curl -fsSL "https://raw.githubusercontent.com/$REPO/refs/heads/$BRANCH/install.sh" \
  | sudo bash -s -- --uninstall
sudo reboot
```

Your files in `~/.config` stay.
