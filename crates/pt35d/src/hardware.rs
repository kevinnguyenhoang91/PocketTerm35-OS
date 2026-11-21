//! Everything the daemon reads out of sysfs, and the graceful absence of it.
//!
//! On the PocketTerm35 the RP2040 owns the backlight and there is no battery
//! gauge at all, so neither path exists. That is a supported configuration:
//! every getter returns `Option` and the bar simply hides the widget. The
//! uConsole has both: an `ocp8178` backlight and the AXP228's battery.

use std::fs;
use std::path::{Path, PathBuf};

/// The battery, backlight and touchscreen are discovered once at startup:
/// re-scanning sysfs on every tick is wasted IO. Network interfaces come and go
/// (a USB adapter, a tether), so they are listed when asked, which is one readdir.
#[derive(Debug, Default, Clone)]
pub struct Hardware {
    pub battery: Option<PathBuf>,
    pub backlight: Option<PathBuf>,
    /// A touchscreen is wired in: the PocketTerm35's GT911. The uConsole has none.
    pub touchscreen: bool,
    net_root: PathBuf,
}

impl Hardware {
    pub fn probe() -> Self {
        Self::probe_in(Path::new("/sys"))
    }

    /// Same as [`Hardware::probe`] against an arbitrary sysfs root (tests).
    pub fn probe_in(sysfs: &Path) -> Self {
        let battery = list(&sysfs.join("class/power_supply"))
            .into_iter()
            .find(|p| read_trim(&p.join("type")).as_deref() == Some("Battery"));

        // Pick the backlight with the largest max_brightness — on a Pi there is
        // usually at most one, but a DSI panel can add a second, dummy device.
        let backlight = list(&sysfs.join("class/backlight"))
            .into_iter()
            .max_by_key(|p| read_u32(&p.join("max_brightness")).unwrap_or(0));

        Self {
            battery,
            backlight,
            touchscreen: list(&sysfs.join("class/input"))
                .iter()
                .any(|dev| is_touchscreen(dev)),
            net_root: sysfs.join("class/net"),
        }
    }

    fn interfaces(&self) -> Vec<PathBuf> {
        list(&self.net_root)
            .into_iter()
            .filter(|p| p.file_name().and_then(|n| n.to_str()) != Some("lo"))
            .collect()
    }

    pub fn battery_percent(&self) -> Option<u8> {
        let base = self.battery.as_ref()?;
        if let Some(capacity) = read_u32(&base.join("capacity")) {
            return Some(capacity.min(100) as u8);
        }
        // No `capacity`: derive it from charge_now/charge_full if present.
        let now = read_u32(&base.join("charge_now"))?;
        let full = read_u32(&base.join("charge_full"))?;
        if full == 0 {
            return None;
        }
        Some(((now as u64 * 100 / full as u64).min(100)) as u8)
    }

    pub fn charging(&self) -> Option<bool> {
        let status = read_trim(&self.battery.as_ref()?.join("status"))?;
        Some(matches!(status.as_str(), "Charging" | "Full"))
    }

    pub fn brightness_percent(&self) -> Option<u8> {
        let base = self.backlight.as_ref()?;
        let max = read_u32(&base.join("max_brightness"))?;
        let now = read_u32(&base.join("brightness"))?;
        if max == 0 {
            return None;
        }
        Some(((now as u64 * 100 / max as u64).min(100)) as u8)
    }

    /// Apply a percentage and return the level the panel took. `None` when
    /// there is no backlight to talk to (RP2040-owned) or it is not writable.
    ///
    /// A backlight with few levels rounds: the uConsole's has nine. `towards`
    /// is a step's direction, and a step always moves at least one level, or
    /// +10% from 22% rounds back to 22% and the key does nothing.
    pub fn set_brightness_percent(&self, percent: u8, towards: i32) -> Option<u8> {
        let base = self.backlight.as_ref()?;
        let max = read_u32(&base.join("max_brightness")).filter(|&max| max > 0)? as u64;
        let now = read_u32(&base.join("brightness")).unwrap_or(0) as u64;
        let mut value = (max * percent.min(100) as u64 + 50) / 100;
        if towards != 0 && value == now {
            value = if towards > 0 {
                now + 1
            } else {
                now.saturating_sub(1)
            };
        }
        let value = value.clamp(1, max);
        fs::write(base.join("brightness"), value.to_string()).ok()?;
        Some((value * 100 / max) as u8)
    }

    /// The interface carrying traffic: a cable beats Wi-Fi, as NetworkManager's
    /// route metrics do, then anything else that is up (a USB tether, a VPN).
    pub fn network(&self) -> Option<String> {
        let up: Vec<PathBuf> = self.interfaces().into_iter().filter(|i| is_up(i)).collect();
        up.iter()
            .find(|i| is_ethernet(i))
            .or_else(|| up.iter().find(|i| is_wireless(i)))
            .or_else(|| up.first())
            .and_then(|i| name(i))
    }

    /// A wired interface with a cable in. Only a real device counts: a bridge
    /// or a container's veth is up too, and is not a cable.
    pub fn ethernet(&self) -> Option<String> {
        self.interfaces()
            .into_iter()
            .find(|i| is_ethernet(i) && is_up(i))
            .and_then(|i| name(&i))
    }

    /// Wi-Fi link quality as a percentage, whether or not the cable is the
    /// route. `None` with no Wi-Fi link.
    pub fn network_signal(&self) -> Option<u8> {
        let iface = self
            .interfaces()
            .into_iter()
            .find(|i| is_wireless(i) && is_up(i))?;
        let text = fs::read_to_string("/proc/net/wireless").ok()?;
        wireless_quality(&text, &name(&iface)?)
    }
}

fn is_up(iface: &Path) -> bool {
    read_trim(&iface.join("operstate")).as_deref() == Some("up")
}

fn is_wireless(iface: &Path) -> bool {
    iface.join("wireless").is_dir() || iface.join("phy80211").exists()
}

fn is_ethernet(iface: &Path) -> bool {
    iface.join("device").exists() && !is_wireless(iface)
}

fn name(iface: &Path) -> Option<String> {
    Some(iface.file_name()?.to_string_lossy().into_owned())
}

/// An input device that is a touchscreen: it reports where it is touched
/// (multitouch X) on the display itself (`INPUT_PROP_DIRECT`), not a touchpad.
fn is_touchscreen(dev: &Path) -> bool {
    const INPUT_PROP_DIRECT: usize = 1;
    const ABS_MT_POSITION_X: usize = 0x35;
    let has = |file: &str, bit: usize| {
        read_trim(&dev.join(file)).is_some_and(|bits| bitmap_has(&bits, bit))
    };
    has("properties", INPUT_PROP_DIRECT) && has("capabilities/abs", ABS_MT_POSITION_X)
}

/// One bit of an input capability bitmap as sysfs prints it: hex words of 64
/// bits, the highest first, space-separated.
pub fn bitmap_has(bits: &str, bit: usize) -> bool {
    bits.split_whitespace()
        .rev()
        .nth(bit / 64)
        .and_then(|word| u64::from_str_radix(word, 16).ok())
        .is_some_and(|word| word >> (bit % 64) & 1 == 1)
}

/// `/proc/net/wireless` reports link quality out of 70, one line per radio.
fn wireless_quality(text: &str, iface: &str) -> Option<u8> {
    let line = text.lines().skip(2).find(|l| {
        l.trim_start()
            .strip_prefix(iface)
            .is_some_and(|rest| rest.starts_with(':'))
    })?;
    let quality = line.split_whitespace().nth(2)?.trim_end_matches('.');
    let quality: f32 = quality.parse().ok()?;
    Some(((quality / 70.0) * 100.0).clamp(0.0, 100.0) as u8)
}

/// CPU frequency profiles, as shares of the chip's top speed: 1.5 and 1.8 GHz
/// on a Pi 5 or CM5, 0.94 and 1.1 GHz on a CM4. The Pi 5 in the PocketTerm35
/// browns out under a full load on a small USB-C supply, so `balanced` is
/// the shipped default.
#[derive(Debug, Clone, Copy)]
pub struct CpuLimits {
    pub governor: &'static str,
    pub max_khz: u32,
}

pub fn cpu_limits(profile: pt35_common::ipc::CpuProfile, hardware_max: u32) -> CpuLimits {
    use pt35_common::ipc::CpuProfile::*;
    let share = |eighths: u64| (hardware_max as u64 * eighths / 8) as u32;
    match profile {
        Powersave => CpuLimits {
            governor: "powersave",
            max_khz: share(5),
        },
        Balanced => CpuLimits {
            governor: "ondemand",
            max_khz: share(6),
        },
        Performance => CpuLimits {
            governor: "performance",
            max_khz: hardware_max,
        },
    }
}

/// Write a CPU profile to every policy in cpufreq. Best-effort: on a kernel
/// without cpufreq write access this is a no-op and the caller reports it.
pub fn apply_cpu_profile(profile: pt35_common::ipc::CpuProfile) -> bool {
    let policies = cpu_policies();
    if policies.is_empty() {
        return false;
    }
    let mut ok = false;
    for policy in policies {
        let Some(hardware_max) = read_u32(&policy.join("cpuinfo_max_freq")) else {
            continue;
        };
        let limits = cpu_limits(profile, hardware_max);
        ok |= fs::write(policy.join("scaling_governor"), limits.governor).is_ok();
        let _ = fs::write(policy.join("scaling_max_freq"), limits.max_khz.to_string());
    }
    ok
}

/// Which profile the CPU is in now, read back from cpufreq. `None` when the
/// settings match no profile, as on a fresh boot: ondemand at full speed.
pub fn current_cpu_profile() -> Option<pt35_common::ipc::CpuProfile> {
    let policy = cpu_policies().into_iter().next()?;
    cpu_profile_of(
        &read_trim(&policy.join("scaling_governor"))?,
        read_u32(&policy.join("scaling_max_freq"))?,
        read_u32(&policy.join("cpuinfo_max_freq"))?,
    )
}

pub fn cpu_profile_of(
    governor: &str,
    max_khz: u32,
    hardware_max: u32,
) -> Option<pt35_common::ipc::CpuProfile> {
    use pt35_common::ipc::CpuProfile::*;
    [Powersave, Balanced, Performance]
        .into_iter()
        .find(|&profile| {
            let limits = cpu_limits(profile, hardware_max);
            limits.governor == governor && limits.max_khz == max_khz
        })
}

/// The `policy*` folders only: with ondemand active, cpufreq also holds an
/// `ondemand/` tunables folder, and it sorts first.
fn cpu_policies() -> Vec<PathBuf> {
    list(Path::new("/sys/devices/system/cpu/cpufreq"))
        .into_iter()
        .filter(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("policy"))
        })
        .collect()
}

fn list(dir: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .collect();
    found.sort();
    found
}

fn read_trim(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

fn read_u32(path: &Path) -> Option<u32> {
    read_trim(path)?.parse().ok()
}

#[cfg(test)]
mod cpu_tests {
    use super::*;
    use pt35_common::ipc::CpuProfile;

    #[test]
    fn a_profile_is_read_back_from_what_it_wrote() {
        assert_eq!(
            cpu_profile_of("ondemand", 1_800_000, 2_400_000),
            Some(CpuProfile::Balanced)
        );
        assert_eq!(
            cpu_profile_of("performance", 2_400_000, 2_400_000),
            Some(CpuProfile::Performance)
        );
        assert_eq!(
            cpu_profile_of("ondemand", 2_400_000, 2_400_000),
            None,
            "the boot default is no profile"
        );
    }

    #[test]
    fn a_cm4_powersaves_below_its_top_speed() {
        let limits = cpu_limits(CpuProfile::Powersave, 1_500_000);
        assert_eq!(limits.max_khz, 937_500);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a throwaway sysfs tree; `name` keeps parallel tests apart.
    fn sysfs_with(name: &str, entries: &[(&str, &str)]) -> PathBuf {
        let root = std::env::temp_dir().join(format!("pt35-sysfs-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for (path, contents) in entries {
            let full = root.join(path);
            fs::create_dir_all(full.parent().unwrap()).unwrap();
            fs::write(full, contents).unwrap();
        }
        root
    }

    #[test]
    fn reports_nothing_when_the_hardware_is_invisible() {
        let root = sysfs_with("empty", &[("class/net/lo/operstate", "unknown\n")]);
        let hw = Hardware::probe_in(&root);
        assert!(hw.battery.is_none());
        assert!(hw.backlight.is_none());
        assert_eq!(hw.battery_percent(), None);
        assert_eq!(hw.brightness_percent(), None);
        assert_eq!(hw.network(), None);
        assert_eq!(
            hw.set_brightness_percent(50, 0),
            None,
            "must admit it cannot set brightness"
        );
        assert!(!hw.touchscreen);
    }

    #[test]
    fn reads_battery_and_backlight_when_present() {
        let root = sysfs_with(
            "full",
            &[
                ("class/power_supply/BAT0/type", "Battery\n"),
                ("class/power_supply/BAT0/capacity", "73\n"),
                ("class/power_supply/BAT0/status", "Charging\n"),
                ("class/backlight/rpi_backlight/max_brightness", "255\n"),
                ("class/backlight/rpi_backlight/brightness", "128\n"),
                ("class/net/wlan0/operstate", "up\n"),
            ],
        );
        let hw = Hardware::probe_in(&root);
        assert_eq!(hw.battery_percent(), Some(73));
        assert_eq!(hw.charging(), Some(true));
        assert_eq!(hw.brightness_percent(), Some(50));
        assert_eq!(hw.network().as_deref(), Some("wlan0"));
    }

    #[test]
    fn a_step_on_a_nine_level_backlight_always_moves() {
        let root = sysfs_with(
            "ocp8178",
            &[
                ("class/backlight/backlight@0/max_brightness", "9\n"),
                ("class/backlight/backlight@0/brightness", "2\n"),
            ],
        );
        let hw = Hardware::probe_in(&root);
        assert_eq!(hw.brightness_percent(), Some(22));
        assert_eq!(hw.set_brightness_percent(32, 1), Some(33));
        assert_eq!(
            hw.set_brightness_percent(36, 1),
            Some(44),
            "33 + 3 rounds to 33"
        );
        assert_eq!(
            hw.set_brightness_percent(1, -1),
            Some(11),
            "never fully dark"
        );
        assert_eq!(hw.set_brightness_percent(1, -1), Some(11));
    }

    #[test]
    fn a_touchscreen_is_direct_and_a_touchpad_is_not() {
        let goodix = [
            ("class/input/input4/properties", "2\n"),
            ("class/input/input4/capabilities/abs", "260800000000003\n"),
        ];
        assert!(Hardware::probe_in(&sysfs_with("goodix", &goodix)).touchscreen);
        let touchpad = [
            ("class/input/input5/properties", "5\n"),
            ("class/input/input5/capabilities/abs", "260800000000003\n"),
        ];
        assert!(!Hardware::probe_in(&sysfs_with("touchpad", &touchpad)).touchscreen);
    }

    #[test]
    fn a_cable_beats_wifi_and_a_bridge_is_not_a_cable() {
        let root = sysfs_with(
            "wired",
            &[
                ("class/net/docker0/operstate", "up\n"),
                ("class/net/eth0/device/uevent", ""),
                ("class/net/eth0/operstate", "down\n"),
                ("class/net/wlan0/device/uevent", ""),
                ("class/net/wlan0/wireless/.keep", ""),
                ("class/net/wlan0/operstate", "up\n"),
            ],
        );
        let hw = Hardware::probe_in(&root);
        assert_eq!(hw.ethernet(), None, "unplugged");
        assert_eq!(hw.network().as_deref(), Some("wlan0"));

        fs::write(root.join("class/net/eth0/operstate"), "up\n").unwrap();
        assert_eq!(hw.ethernet().as_deref(), Some("eth0"));
        assert_eq!(hw.network().as_deref(), Some("eth0"));
    }

    #[test]
    fn signal_is_read_for_the_named_radio() {
        let text = "Inter-| sta-|   Quality\n face | tus | link level noise\n wlan0: 0000   62.  -48.  -256        0\n";
        assert_eq!(wireless_quality(text, "wlan0"), Some(88));
        assert_eq!(wireless_quality(text, "wlan1"), None);
    }

    #[test]
    fn derives_capacity_from_charge_counters() {
        let root = sysfs_with(
            "charge",
            &[
                ("class/power_supply/BAT0/type", "Battery\n"),
                ("class/power_supply/BAT0/charge_now", "2500\n"),
                ("class/power_supply/BAT0/charge_full", "5000\n"),
            ],
        );
        assert_eq!(Hardware::probe_in(&root).battery_percent(), Some(50));
    }

    #[test]
    fn ignores_a_usb_charger_that_is_not_a_battery() {
        let root = sysfs_with(
            "usb",
            &[
                ("class/power_supply/usb/type", "USB\n"),
                ("class/power_supply/usb/online", "1\n"),
            ],
        );
        assert!(Hardware::probe_in(&root).battery.is_none());
    }
}
