//! Read-only checks against the real machine's drives. Nothing is modified.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use sentinel_scanner::drives::{DriveKind, DriveStatus, list_drives, query_drive};

#[test]
fn finds_the_system_drive_with_consistent_capacity() {
    let drives = list_drives().unwrap();
    let system: Vec<_> = drives.iter().filter(|d| d.is_system).collect();
    assert_eq!(system.len(), 1, "exactly one system drive: {drives:#?}");
    let sys = system[0];
    assert_eq!(sys.kind, DriveKind::Fixed);
    assert_eq!(sys.status, DriveStatus::Ready);
    assert!(sys.file_system.is_some());
    let space = sys.space.unwrap();
    assert!(space.total_bytes > 0);
    assert_eq!(space.used_bytes + space.free_bytes, space.total_bytes);
    assert!(space.available_bytes <= space.free_bytes);
}

#[test]
fn roots_are_unique_drive_letters() {
    let drives = list_drives().unwrap();
    let mut roots: Vec<_> = drives.iter().map(|d| d.root.to_ascii_uppercase()).collect();
    for r in &roots {
        assert!(
            r.len() == 3 && r.ends_with(":\\") && r.as_bytes()[0].is_ascii_alphabetic(),
            "{r}"
        );
    }
    roots.sort();
    roots.dedup();
    assert_eq!(roots.len(), drives.len());
}

#[test]
fn unused_letter_reports_unknown_without_space() {
    let used: Vec<char> = list_drives()
        .unwrap()
        .iter()
        .filter_map(|d| d.root.chars().next())
        .map(|c| c.to_ascii_uppercase())
        .collect();
    let Some(free) = ('A'..='Z').rev().find(|c| !used.contains(c)) else {
        eprintln!("SKIPPED: every drive letter is in use");
        return;
    };
    let d = query_drive(&format!("{free}:\\"));
    assert_eq!(d.kind, DriveKind::Unknown);
    assert_eq!(d.status, DriveStatus::NotQueried);
    assert!(d.space.is_none());
    assert!(!d.is_system);
}
