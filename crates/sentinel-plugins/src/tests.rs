#![allow(clippy::unwrap_used, clippy::expect_used)]

use serde_json::{Value, json};

use super::*;

fn good() -> Value {
    json!({
        "format": 1,
        "id": "example.acme-render",
        "name": "Acme Render caches",
        "version": "1.0.0",
        "author": "Acme Tools",
        "description": "Frame and shader caches kept by Acme Render.",
        "capabilities": ["filesystem.read", "filesystem.cleanup"],
        "cleanup": [{
            "id": "shader-cache",
            "name": "Acme shader cache",
            "root": { "anchor": "localAppData", "path": "Acme\\Render\\ShaderCache" },
            "children": { "names": ["gl", "vk", "dx-*"] },
            "risk": "lowRisk",
            "minAgeDays": 14,
            "description": "Compiled shaders.",
            "onRemoval": "Acme Render recompiles them the next time it runs; the first start is slower."
        }]
    })
}

fn parse_value(v: &Value) -> Result<Plugin, PluginError> {
    parse(v.to_string().as_bytes())
}

/// Set `value` at a JSON pointer inside the good manifest.
fn with(pointer: &str, value: Value) -> Value {
    let mut v = good();
    *v.pointer_mut(pointer).unwrap() = value;
    v
}

fn refused(v: &Value) -> String {
    match parse_value(v) {
        Ok(_) => panic!("should have been refused: {v}"),
        Err(e) => e.to_string(),
    }
}

#[test]
fn a_good_plugin_is_accepted_and_normalized() {
    let p = parse_value(&good()).unwrap();
    assert_eq!(p.manifest.id, "example.acme-render");
    assert_eq!(p.manifest.capabilities, [CAP_CLEANUP, CAP_READ], "sorted");
    let c = &p.manifest.cleanup[0];
    assert_eq!(c.root.components(), ["Acme", "Render", "ShaderCache"]);
    assert_eq!(c.effective_min_age_days(), 14);
    assert_eq!(p.sha256.len(), 64);
}

#[test]
fn the_canonical_form_is_a_fixed_point_with_a_stable_fingerprint() {
    // The same plugin spelled differently (slashes, order of capabilities, no age) is the
    // same plugin once canonical, and re-validating canonical text changes nothing.
    let mut v = good();
    v["capabilities"] = json!(["filesystem.cleanup", "filesystem.read", "filesystem.read"]);
    v["cleanup"][0]["root"]["path"] = json!("Acme/Render/ShaderCache");
    let a = parse_value(&v).unwrap();
    let b = parse_value(&good()).unwrap();
    assert_eq!(a.canonical, b.canonical);
    assert_eq!(a.sha256, b.sha256);
    let again = parse(a.canonical.as_bytes()).unwrap();
    assert_eq!(again.canonical, a.canonical);
    assert_eq!(load_stored(&a.canonical, &a.sha256).unwrap(), a);

    // Any change at all changes the fingerprint.
    let c = parse_value(&with("/cleanup/0/minAgeDays", json!(15))).unwrap();
    assert_ne!(c.sha256, a.sha256);
}

#[test]
fn a_default_age_is_filled_in_and_a_stored_plugin_that_was_edited_is_refused() {
    let mut v = good();
    v["cleanup"][0]
        .as_object_mut()
        .unwrap()
        .remove("minAgeDays");
    let p = parse_value(&v).unwrap();
    assert_eq!(p.manifest.cleanup[0].min_age_days, Some(DEFAULT_AGE_DAYS));

    let edited = p
        .canonical
        .replace("Acme shader cache", "Acme shader cachf");
    assert_eq!(
        load_stored(&edited, &p.sha256),
        Err(PluginError::HashMismatch)
    );
    assert_eq!(
        load_stored(&p.canonical, &"0".repeat(64)),
        Err(PluginError::HashMismatch)
    );
    // A tampered file that is re-hashed to match still has to pass validation.
    let evil = p.canonical.replace("lowRisk", "safe");
    assert!(load_stored(&evil, &fingerprint(&evil)).is_err());
}

#[test]
fn risk_is_never_safe_or_above_medium() {
    for r in ["safe", "highRisk", "protected", "low", ""] {
        let v = with("/cleanup/0/risk", json!(r));
        assert!(parse_value(&v).is_err(), "{r}");
    }
    for r in ["lowRisk", "mediumRisk"] {
        assert!(
            parse_value(&with("/cleanup/0/risk", json!(r))).is_ok(),
            "{r}"
        );
    }
}

#[test]
fn the_age_floor_cannot_be_lowered() {
    for d in [0, 1, 2] {
        assert!(refused(&with("/cleanup/0/minAgeDays", json!(d))).contains("minAgeDays"));
    }
    assert!(parse_value(&with("/cleanup/0/minAgeDays", json!(3))).is_ok());
    assert!(parse_value(&with("/cleanup/0/minAgeDays", json!(3651))).is_err());
    assert!(parse_value(&with("/cleanup/0/minAgeDays", json!(-5))).is_err());
}

#[test]
fn roots_cannot_escape_alias_or_name_anything_personal() {
    let bad_paths = [
        "",
        "\\Acme",
        "Acme\\",
        "Acme\\\\Render",
        "..",
        "Acme\\..\\Other",
        ".",
        "C:\\Acme",
        "C:",
        "Acme:stream",
        "\\\\server\\share",
        "\\\\?\\C:\\x",
        "Acme\\CON",
        "nul.txt",
        "Acme\\COM1.log",
        "Acme\\name.",
        "Acme\\name ",
        "Acme\\PROGRA~1",
        "Acme\\*",
        "Acme\\a?b",
        "Acme\\a|b",
        "Acme\\a<b",
        "Acme\\.git",
        "Acme\\.ssh\\keys",
        "Acme\\.env",
        "Acme\\.env.local",
        "Acme\\disk.vhdx",
        "Acme\\keys.kdbx",
        "a\\b\\c\\d\\e\\f\\g\\h\\i",
        "Acme\u{202e}gnp.exe",
        "Acme\u{200b}",
        "Acme\tTab",
        "Acme\nLine",
    ];
    for p in bad_paths {
        let v = with("/cleanup/0/root/path", json!(p));
        assert!(parse_value(&v).is_err(), "{p:?} should be refused");
    }
    for ok in [
        "Acme",
        "Acme/Render",
        "Acme Tools\\Render Cache",
        "a.b\\c-d_e",
    ] {
        assert!(
            parse_value(&with("/cleanup/0/root/path", json!(ok))).is_ok(),
            "{ok}"
        );
    }
}

#[test]
fn personal_folders_are_not_available_under_the_profile() {
    for first in [
        "Documents",
        "documents",
        "Desktop",
        "Downloads",
        "Pictures",
        "Videos",
        "Music",
        "AppData",
        "OneDrive",
        "OneDrive - Contoso",
        "Saved Games",
    ] {
        let mut v = good();
        v["cleanup"][0]["root"] =
            json!({ "anchor": "userProfile", "path": format!("{first}\\Cache") });
        assert!(parse_value(&v).is_err(), "{first}");
    }
    let mut v = good();
    v["cleanup"][0]["root"] = json!({ "anchor": "userProfile", "path": ".acme\\cache" });
    assert!(parse_value(&v).is_ok());
}

#[test]
fn anchors_are_a_closed_list() {
    for a in [
        "documents",
        "desktop",
        "programFiles",
        "windows",
        "C:\\",
        "temp",
        "",
    ] {
        let mut v = good();
        v["cleanup"][0]["root"]["anchor"] = json!(a);
        assert!(parse_value(&v).is_err(), "{a}");
    }
    for a in ["localAppData", "roamingAppData", "userProfile"] {
        let mut v = good();
        v["cleanup"][0]["root"]["anchor"] = json!(a);
        v["cleanup"][0]["root"]["path"] = json!(".acme\\cache");
        assert!(parse_value(&v).is_ok(), "{a}");
    }
}

#[test]
fn child_patterns_are_names_with_at_most_one_wildcard() {
    for bad in [
        json!([]),
        json!([""]),
        json!(["a*b*c"]),
        json!(["**"]),
        json!(["a\\b"]),
        json!(["a/b"]),
        json!([".."]),
        json!(["x:y"]),
        json!(["CON"]),
        json!(["a", "A"]),
        json!(["tab\tname"]),
    ] {
        assert!(
            parse_value(&with("/cleanup/0/children/names", bad.clone())).is_err(),
            "{bad}"
        );
    }
    let too_many: Vec<String> = (0..=MAX_PATTERNS).map(|i| format!("n{i}")).collect();
    assert!(parse_value(&with("/cleanup/0/children/names", json!(too_many))).is_err());
    assert!(parse_value(&with("/cleanup/0/children/names", json!(["*"]))).is_ok());
}

#[test]
fn matching_is_case_insensitive_linear_and_never_crosses_a_separator() {
    assert!(name_matches("gl", "GL"));
    assert!(name_matches("dx-*", "dx-12"));
    assert!(name_matches("dx-*", "DX-"));
    assert!(name_matches("*.tmp", "a.TMP"));
    assert!(name_matches("a*z", "az"));
    assert!(!name_matches("a*z", "a"), "head and tail must not overlap");
    assert!(!name_matches("abc*abc", "abc"));
    assert!(!name_matches("dx-*", "gl"));
    assert!(!name_matches("*", r"a\b"), "never across a separator");
    assert!(!name_matches("*", "a/b"));
    assert!(!name_matches("gl", "gl2"));
    // A long name against a wildcard pattern is immediate.
    let long = "a".repeat(100_000);
    assert!(name_matches("*a", &long));
    assert!(!name_matches("*b", &long));
}

#[test]
fn capabilities_are_exactly_the_two_and_both_are_needed() {
    for caps in [
        json!([]),
        json!(["filesystem.read"]),
        json!(["filesystem.cleanup"]),
        json!(["filesystem.read", "filesystem.cleanup", "network.none"]),
        json!(["filesystem.read", "filesystem.cleanup", "process.inspect"]),
        json!(["filesystem.write"]),
        json!(["*"]),
    ] {
        assert!(
            parse_value(&with("/capabilities", caps.clone())).is_err(),
            "{caps}"
        );
    }
    assert!(refused(&with("/capabilities", json!(["network.access"]))).contains("not available"));
}

#[test]
fn identity_and_text_fields_are_bounded_and_plain() {
    for (ptr, val) in [
        ("/id", json!("")),
        ("/id", json!("ab")),
        ("/id", json!("Acme")),
        ("/id", json!("acme..x")),
        ("/id", json!("-acme")),
        ("/id", json!("acme-")),
        ("/id", json!("acme_x")),
        ("/id", json!("a".repeat(65))),
        ("/id", json!("acme\u{202e}x")),
        ("/name", json!("")),
        ("/name", json!(" padded")),
        ("/name", json!("a".repeat(65))),
        ("/name", json!("two\nlines")),
        ("/name", json!("bidi \u{202e}override")),
        ("/name", json!("zero\u{200b}width")),
        ("/author", json!("")),
        ("/description", json!("d".repeat(401))),
        ("/version", json!("1")),
        ("/version", json!("1.0")),
        ("/version", json!("1.0.0-beta")),
        ("/version", json!("a.b.c")),
        ("/format", json!(2)),
        ("/format", json!(0)),
        ("/cleanup/0/id", json!("Shader")),
        ("/cleanup/0/id", json!("")),
        ("/cleanup/0/name", json!("x\ty")),
        ("/cleanup/0/description", json!("d".repeat(301))),
        ("/cleanup/0/onRemoval", json!("")),
        ("/cleanup/0/onRemoval", json!("o".repeat(401))),
    ] {
        assert!(
            parse_value(&with(ptr, val.clone())).is_err(),
            "{ptr} = {val}"
        );
    }
}

#[test]
fn unknown_fields_wrong_types_and_oversize_input_are_refused() {
    let mut v = good();
    v["extra"] = json!(true);
    assert!(parse_value(&v).is_err(), "unknown top-level field");
    let mut v = good();
    v["cleanup"][0]["command"] = json!("del /s *");
    assert!(parse_value(&v).is_err(), "unknown provider field");
    let mut v = good();
    v["cleanup"][0]["root"]["drive"] = json!("C");
    assert!(parse_value(&v).is_err(), "unknown root field");
    assert!(parse_value(&with("/cleanup", json!("x"))).is_err());
    assert!(parse_value(&with("/format", json!("1"))).is_err());
    assert!(parse_value(&with("/cleanup/0/minAgeDays", json!(7.5))).is_err());

    assert_eq!(
        parse(b"").unwrap_err(),
        PluginError::Json("EOF while parsing a value at line 1 column 0".into())
    );
    assert!(matches!(parse(b"[]"), Err(PluginError::Json(_))));
    assert!(matches!(parse(b"null"), Err(PluginError::Json(_))));
    assert_eq!(
        parse(&[0xff, 0xfe, 0x00]).unwrap_err(),
        PluginError::NotUtf8
    );
    let big = vec![b' '; MAX_MANIFEST_BYTES + 1];
    assert_eq!(parse(&big).unwrap_err(), PluginError::TooLarge);
    // Duplicate keys: the last wins in serde_json, but the result is still fully validated.
    let dup = good()
        .to_string()
        .replace("\"format\":1", "\"format\":1,\"format\":2");
    assert!(parse(dup.as_bytes()).is_err());
}

#[test]
fn a_byte_order_mark_is_tolerated_and_the_provider_count_is_bounded() {
    let mut bytes = vec![0xEF, 0xBB, 0xBF];
    bytes.extend_from_slice(good().to_string().as_bytes());
    assert!(parse(&bytes).is_ok());

    let mut v = good();
    let one = v["cleanup"][0].clone();
    let many: Vec<Value> = (0..=MAX_PROVIDERS)
        .map(|i| {
            let mut c = one.clone();
            c["id"] = json!(format!("p{i}"));
            c
        })
        .collect();
    v["cleanup"] = json!(many);
    assert!(parse_value(&v).is_err());
    v["cleanup"] = json!([]);
    assert!(parse_value(&v).is_err());
    let mut dup = good();
    let first = dup["cleanup"][0].clone();
    dup["cleanup"] = json!([first.clone(), first]);
    assert!(refused(&dup).contains("duplicate id"));
}

// ---- seeded fuzz ---------------------------------------------------------------------

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

#[test]
fn whatever_the_root_path_says_an_accepted_plugin_has_only_plain_relative_folders() {
    const PIECES: &[&str] = &[
        "Acme",
        "render",
        "..",
        ".",
        "\\",
        "/",
        "C:",
        ":",
        "CON",
        "nul",
        "x ",
        "x.",
        "*",
        "?",
        "<",
        "|",
        "\"",
        "\u{0}",
        "\u{202e}",
        "\u{200b}",
        "é",
        "名前",
        ".git",
        ".ssh",
        ".env",
        "a.vhdx",
        "PROGRA~1",
        "Documents",
        "\\\\",
        "~",
        "...",
        "tab\t",
    ];
    let mut rng = Rng(0xFEED_FACE);
    let (mut accepted, mut refused_n) = (0u32, 0u32);
    for i in 0..60_000u32 {
        let path: String = (0..=rng.below(7))
            .map(|_| PIECES[rng.below(PIECES.len())])
            .collect();
        let mut v = good();
        v["cleanup"][0]["root"]["path"] = json!(path);
        let Ok(p) = parse_value(&v) else {
            refused_n += 1;
            continue;
        };
        accepted += 1;
        let comps = p.manifest.cleanup[0].root.components();
        assert!(
            (1..=MAX_ROOT_COMPONENTS).contains(&comps.len()),
            "iteration {i}: {path:?}"
        );
        for c in comps {
            let ctx = format!("iteration {i}: {path:?} -> component {c:?}");
            assert!(!c.is_empty() && c != "." && c != "..", "{ctx}");
            assert!(
                !c.chars().any(|ch| ch.is_control() || deceptive(ch)),
                "{ctx}"
            );
            assert!(
                !c.contains([':', '*', '?', '<', '>', '|', '"', '~']),
                "{ctx}"
            );
            assert!(!c.ends_with('.') && !c.ends_with(' '), "{ctx}");
            assert!(
                !RESERVED.contains(&c.split('.').next().unwrap_or(c).to_lowercase().as_str()),
                "{ctx}"
            );
        }
    }
    assert!(
        accepted > 50,
        "the generator must produce acceptable paths too ({accepted})"
    );
    assert!(refused_n > 30_000, "and hostile ones ({refused_n})");
}
