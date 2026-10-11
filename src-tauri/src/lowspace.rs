//! When to warn that a drive is low on space.
//!
//! Pure decision logic, kept apart from the notification and the timer so it can be tested.
//! One warning per low spell: a drive that stays low is not announced again every check, and
//! it is only announced again after it has clearly recovered (the margin stops a drive
//! hovering around the threshold from warning over and over).

use std::collections::HashSet;

use sentinel_scanner::drives::{Drive, DriveKind, DriveStatus, LOW_SPACE_PERCENT};

/// A drive must reach this much free space (a few points above the warning threshold)
/// before it counts as recovered and may warn again.
const RECOVERED_PERCENT: u64 = LOW_SPACE_PERCENT + 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Warning {
    pub root: String,
    pub free_bytes: u64,
    pub total_bytes: u64,
}

#[derive(Default)]
pub(crate) struct LowSpaceTracker {
    warned: HashSet<String>,
}

impl LowSpaceTracker {
    /// The drives that have just become low, given the current drive list. Only ready fixed
    /// drives are considered: removable, network and optical media come and go, and a full
    /// USB stick is not a problem with the computer.
    pub(crate) fn update(&mut self, drives: &[Drive]) -> Vec<Warning> {
        let mut out = Vec::new();
        for d in drives {
            let Some(space) = d.space else { continue };
            if d.kind != DriveKind::Fixed
                || d.status != DriveStatus::Ready
                || space.total_bytes == 0
            {
                continue;
            }
            let key = d.root.to_lowercase();
            if space.low_space {
                if self.warned.insert(key) {
                    out.push(Warning {
                        root: d.root.clone(),
                        free_bytes: space.free_bytes,
                        total_bytes: space.total_bytes,
                    });
                }
            } else if u128::from(space.free_bytes) * 100
                >= u128::from(space.total_bytes) * u128::from(RECOVERED_PERCENT)
            {
                self.warned.remove(&key);
            }
        }
        out
    }
}

/// One periodic check: when alerts are on, announce drives that have just become low.
///
/// While alerts are off the tracker is left alone, so turning them on while a drive is
/// already low warns once, as a person would expect.
pub(crate) fn check(
    enabled: bool,
    drives: &[Drive],
    tracker: &mut LowSpaceTracker,
    notify: &mut dyn FnMut(&str, &str),
) {
    if !enabled {
        return;
    }
    for w in tracker.update(drives) {
        let (title, body) = message(&w);
        notify(&title, &body);
    }
}

/// The notification text for a warning. Names no folder or file: only the drive and its space.
pub(crate) fn message(w: &Warning) -> (String, String) {
    let gb = |b: u64| format!("{:.1} GB", b as f64 / (1024.0 * 1024.0 * 1024.0));
    (
        format!("{} is low on space", w.root.trim_end_matches('\\')),
        format!(
            "{} free of {}. Open Sentinel to see what is using it.",
            gb(w.free_bytes),
            gb(w.total_bytes)
        ),
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use sentinel_scanner::drives::Space;

    use super::*;

    fn drive(root: &str, kind: DriveKind, free_pct: u64) -> Drive {
        Drive {
            root: root.into(),
            kind,
            label: None,
            file_system: Some("NTFS".into()),
            space: Some(Space::new(1000, free_pct * 10, free_pct * 10)),
            is_system: false,
            status: DriveStatus::Ready,
        }
    }

    #[test]
    fn warns_once_per_low_spell_and_again_only_after_a_clear_recovery() {
        let mut t = LowSpaceTracker::default();
        assert!(t.update(&[drive(r"C:\", DriveKind::Fixed, 50)]).is_empty());
        let first = t.update(&[drive(r"C:\", DriveKind::Fixed, 8)]);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].root, r"C:\");
        // Still low, and dipping further: no repeat.
        assert!(t.update(&[drive(r"C:\", DriveKind::Fixed, 8)]).is_empty());
        assert!(t.update(&[drive(r"C:\", DriveKind::Fixed, 3)]).is_empty());
        // Hovering around the threshold is not a recovery.
        assert!(t.update(&[drive(r"C:\", DriveKind::Fixed, 11)]).is_empty());
        assert!(t.update(&[drive(r"C:\", DriveKind::Fixed, 9)]).is_empty());
        // A clear recovery, then low again: a new warning.
        assert!(t.update(&[drive(r"C:\", DriveKind::Fixed, 30)]).is_empty());
        assert_eq!(t.update(&[drive(r"C:\", DriveKind::Fixed, 5)]).len(), 1);
    }

    #[test]
    fn each_drive_has_its_own_spell_and_case_does_not_matter() {
        let mut t = LowSpaceTracker::default();
        let both = t.update(&[
            drive(r"C:\", DriveKind::Fixed, 5),
            drive(r"D:\", DriveKind::Fixed, 4),
        ]);
        assert_eq!(both.len(), 2);
        assert!(t.update(&[drive(r"c:\", DriveKind::Fixed, 5)]).is_empty());
    }

    #[test]
    fn only_ready_fixed_drives_with_a_measurement_are_considered() {
        let mut t = LowSpaceTracker::default();
        for kind in [
            DriveKind::Removable,
            DriveKind::Network,
            DriveKind::Optical,
            DriveKind::RamDisk,
        ] {
            assert!(t.update(&[drive(r"E:\", kind, 1)]).is_empty(), "{kind:?}");
        }
        let mut broken = drive(r"F:\", DriveKind::Fixed, 1);
        broken.status = DriveStatus::Error {
            message: "x".into(),
        };
        let mut unmeasured = drive(r"G:\", DriveKind::Fixed, 1);
        unmeasured.space = None;
        let mut empty = drive(r"H:\", DriveKind::Fixed, 1);
        empty.space = Some(Space::new(0, 0, 0));
        assert!(t.update(&[broken, unmeasured, empty]).is_empty());
    }

    #[test]
    fn nothing_is_announced_while_alerts_are_off_and_turning_them_on_warns_once() {
        let mut t = LowSpaceTracker::default();
        let low = [drive(r"C:\", DriveKind::Fixed, 4)];
        let sent = std::cell::RefCell::new(Vec::<String>::new());
        let mut notify = |title: &str, _: &str| sent.borrow_mut().push(title.to_owned());
        check(false, &low, &mut t, &mut notify);
        check(false, &low, &mut t, &mut notify);
        assert!(sent.borrow().is_empty());
        check(true, &low, &mut t, &mut notify);
        check(true, &low, &mut t, &mut notify);
        assert_eq!(*sent.borrow(), ["C: is low on space"]);
    }

    #[test]
    fn the_message_names_only_the_drive_and_its_space() {
        let (title, body) = message(&Warning {
            root: r"C:\".into(),
            free_bytes: 5 * 1024 * 1024 * 1024,
            total_bytes: 100 * 1024 * 1024 * 1024,
        });
        assert_eq!(title, "C: is low on space");
        assert_eq!(
            body,
            "5.0 GB free of 100.0 GB. Open Sentinel to see what is using it."
        );
    }
}
