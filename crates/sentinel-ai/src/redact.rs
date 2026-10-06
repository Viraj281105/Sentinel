//! Redaction of text before it can reach any model. See docs/AI_POLICY.md, decision 4.
//!
//! The design fails closed: a Windows path component survives only if it is on a short
//! allow-list of generic names; everything else becomes a stable placeholder. Secrets are
//! removed irreversibly. Placeholders for names, paths, addresses and e-mails map back to
//! the original so an answer can be shown to the person in their own terms.
//!
//! Limits, stated plainly: a path containing spaces is only matched up to its first space
//! unless the space is inside an allow-listed name (`Program Files`), and free text naming
//! a person or project with no path or e-mail is not detected. That is why the model is
//! not given filesystem-derived text at all yet (decision 3): this is the second line of
//! defense, not the first.

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::{Captures, Regex};

/// Path components that are generic enough to keep.
const SAFE_COMPONENTS: &[&str] = &[
    "windows",
    "system32",
    "syswow64",
    "winsxs",
    "program files",
    "program files (x86)",
    "programdata",
    "users",
    "appdata",
    "local",
    "locallow",
    "roaming",
    "temp",
    "tmp",
    "packages",
    "microsoft",
    "desktop",
    "documents",
    "downloads",
    "pictures",
    "videos",
    "music",
    "onedrive",
    "node_modules",
    ".git",
    ".cache",
    ".cargo",
    ".rustup",
    ".npm",
    ".gradle",
    ".m2",
    ".venv",
    "venv",
    "__pycache__",
    "target",
    "build",
    "dist",
    "bin",
    "obj",
    "logs",
    "cache",
    "caches",
    "$recycle.bin",
    "windows.old",
    "dotnet",
    "sdk",
    "lib",
    "src",
];

/// A Windows path. Directory names may contain spaces (up to four words, none with a
/// colon, so prose followed by another drive letter is not swallowed); the final name may
/// contain spaces only when it ends in a short extension. Over-matching hides a little
/// extra prose, which is the safe direction.
fn path_pattern() -> String {
    let tok = r#"[^\\\s"'<>|*?]"#;
    let tok_nc = r#"[^\\\s"'<>|*?:]"#;
    let dir = format!(r"{tok}+(?: {tok_nc}+){{0,3}}\\");
    let last = format!(r"(?:{tok_nc}+ ){{0,2}}{tok}+\.[A-Za-z0-9]{{1,5}}\b|{tok}*");
    format!(r"\b[A-Za-z]:\\(?:{dir})*(?:{last})")
}

struct Patterns {
    secrets: Vec<(Regex, &'static str)>,
    path: Regex,
    unc: Regex,
    email: Regex,
    ipv4: Regex,
    sid: Regex,
}

// The patterns are constants checked by this module's tests, so a failure here is a bug in
// the source, not a runtime condition.
#[allow(clippy::expect_used)]
static PATTERNS: LazyLock<Patterns> = LazyLock::new(|| {
    let re = |s: &str| Regex::new(s).expect("redaction pattern is valid");
    Patterns {
        secrets: vec![
            (
                re(
                    r"-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?(?:-----END [A-Z ]*PRIVATE KEY-----|$)",
                ),
                "<SECRET>",
            ),
            (re(r"\bAKIA[0-9A-Z]{16}\b"), "<SECRET>"),
            (re(r"\bsk-[A-Za-z0-9_\-]{20,}"), "<SECRET>"),
            (re(r"\bgh[pousr]_[A-Za-z0-9]{30,}"), "<SECRET>"),
            (re(r"\bxox[baprs]-[A-Za-z0-9\-]{10,}"), "<SECRET>"),
            (
                re(r"\beyJ[A-Za-z0-9_\-]{8,}\.[A-Za-z0-9_\-]{8,}\.[A-Za-z0-9_\-]{8,}"),
                "<SECRET>",
            ),
            (
                re(
                    r#"(?i)\b(password|passwd|pwd|secret|token|api[_-]?key|access[_-]?key)(\s*[:=]\s*)("[^"]*"|'[^']*'|\S+)"#,
                ),
                "$1$2<SECRET>",
            ),
        ],
        path: re(&path_pattern()),
        unc: re(r#"\\\\[^\s"'<>|*?\\]+(?:\\[^\s"'<>|*?\\]+)*"#),
        email: re(r"[A-Za-z0-9._%+\-]+@[A-Za-z0-9.\-]+\.[A-Za-z]{2,}"),
        ipv4: re(r"\b(?:\d{1,3}\.){3}\d{1,3}\b"),
        sid: re(r"\bS-1-\d+(?:-\d+){2,}\b"),
    }
});

/// Names to hide wherever they appear as words.
#[derive(Debug, Clone, Default)]
pub struct RedactionContext {
    pub user_names: Vec<String>,
    pub host_names: Vec<String>,
}

impl RedactionContext {
    /// The current account, profile folder and computer name.
    pub fn from_env() -> Self {
        let get = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        let mut users: Vec<String> = get("USERNAME").into_iter().collect();
        if let Some(p) = get("USERPROFILE")
            && let Some(leaf) = p.rsplit('\\').next()
        {
            users.push(leaf.to_owned());
        }
        users.sort_by_key(|u| u.to_lowercase());
        users.dedup_by_key(|u| u.to_lowercase());
        Self {
            user_names: users,
            host_names: get("COMPUTERNAME").into_iter().collect(),
        }
    }
}

/// Redacts text and remembers how to map placeholders back. One per conversation, so the
/// same name always gets the same placeholder.
#[derive(Debug, Default)]
pub struct Redactor {
    ctx: RedactionContext,
    /// lower-cased original -> placeholder, per kind.
    forward: HashMap<(&'static str, String), String>,
    /// placeholder -> original.
    back: HashMap<String, String>,
    counters: HashMap<&'static str, u32>,
}

impl Redactor {
    pub fn new(ctx: RedactionContext) -> Self {
        Self {
            ctx,
            ..Self::default()
        }
    }

    fn alias(&mut self, kind: &'static str, original: &str) -> String {
        let key = (kind, original.to_lowercase());
        if let Some(p) = self.forward.get(&key) {
            return p.clone();
        }
        let n = self.counters.entry(kind).or_insert(0);
        *n += 1;
        let placeholder = format!("<{kind}-{n}>");
        self.forward.insert(key, placeholder.clone());
        self.back.insert(placeholder.clone(), original.to_owned());
        placeholder
    }

    fn is_user(&self, s: &str) -> bool {
        self.ctx
            .user_names
            .iter()
            .any(|u| u.eq_ignore_ascii_case(s))
    }

    fn is_host(&self, s: &str) -> bool {
        self.ctx
            .host_names
            .iter()
            .any(|h| h.eq_ignore_ascii_case(s))
    }

    fn component(&mut self, c: &str, last: bool) -> String {
        if c.is_empty() || c.starts_with('<') {
            return c.to_owned(); // already a placeholder
        }
        if self.is_user(c) {
            return "<USER>".to_owned();
        }
        if self.is_host(c) {
            return "<HOST>".to_owned();
        }
        if SAFE_COMPONENTS.contains(&c.to_lowercase().as_str()) {
            return c.to_owned();
        }
        // A file keeps only a short alphanumeric extension.
        if last
            && let Some((stem, ext)) = c.rsplit_once('.')
            && !stem.is_empty()
            && (1..=5).contains(&ext.len())
            && ext.bytes().all(|b| b.is_ascii_alphanumeric())
        {
            return format!("{}.{ext}", self.alias("file", stem));
        }
        self.alias("dir", c)
    }

    fn path(&mut self, matched: &str) -> String {
        // Trailing sentence punctuation is not part of the path.
        let trimmed = matched.trim_end_matches(['.', ',', ';', ':', ')', ']', '}', '!']);
        let tail = &matched[trimmed.len()..];
        let comps: Vec<&str> = trimmed.split('\\').collect();
        let n = comps.len();
        let out: Vec<String> = comps
            .iter()
            .enumerate()
            .map(|(i, c)| {
                if i == 0 {
                    (*c).to_owned() // the drive letter
                } else {
                    self.component(c, i == n - 1)
                }
            })
            .collect();
        format!("{}{tail}", out.join("\\"))
    }

    fn unc(&mut self, matched: &str) -> String {
        let body = matched.trim_start_matches('\\');
        let comps: Vec<&str> = body.split('\\').collect();
        let n = comps.len();
        let mut out = Vec::with_capacity(n);
        for (i, c) in comps.iter().enumerate() {
            out.push(if i == 0 {
                if self.is_host(c) {
                    "<HOST>".to_owned()
                } else {
                    self.alias("host", c)
                }
            } else {
                self.component(c, i == n - 1)
            });
        }
        format!(r"\\{}", out.join("\\"))
    }

    /// Redact `text`. Safe to call repeatedly (idempotent on its own output).
    pub fn redact(&mut self, text: &str) -> String {
        let mut s = text.to_owned();
        for (re, rep) in &PATTERNS.secrets {
            s = re.replace_all(&s, *rep).into_owned();
        }
        // Network paths before drive paths, so `\\server\share` is not half-matched.
        s = PATTERNS
            .unc
            .replace_all(&s, |c: &Captures| self.unc(&c[0]))
            .into_owned();
        s = PATTERNS
            .path
            .replace_all(&s, |c: &Captures| self.path(&c[0]))
            .into_owned();
        s = PATTERNS
            .email
            .replace_all(&s, |c: &Captures| self.alias("email", &c[0]))
            .into_owned();
        s = PATTERNS
            .ipv4
            .replace_all(&s, |c: &Captures| {
                let ip = &c[0];
                let ok = ip.split('.').all(|o| o.parse::<u8>().is_ok());
                if !ok || ip.starts_with("127.") || ip == "0.0.0.0" {
                    ip.to_owned()
                } else {
                    self.alias("ip", ip)
                }
            })
            .into_owned();
        s = PATTERNS.sid.replace_all(&s, "<SID>").into_owned();
        // Names appearing as plain words anywhere else.
        let names: Vec<(String, &str)> = self
            .ctx
            .user_names
            .iter()
            .map(|n| (n.clone(), "<USER>"))
            .chain(self.ctx.host_names.iter().map(|n| (n.clone(), "<HOST>")))
            .collect();
        for (name, rep) in names {
            if name.len() < 2 {
                continue;
            }
            if let Ok(re) = Regex::new(&format!(r"(?i)\b{}\b", regex::escape(&name))) {
                s = re.replace_all(&s, rep).into_owned();
            }
        }
        s
    }

    /// Put original names, paths, addresses and e-mails back for display. Secrets,
    /// `<USER>` and `<HOST>` are not restored.
    pub fn restore(&self, text: &str) -> String {
        let mut keys: Vec<&String> = self.back.keys().collect();
        keys.sort_by_key(|k| std::cmp::Reverse(k.len()));
        let mut s = text.to_owned();
        for k in keys {
            if let Some(orig) = self.back.get(k) {
                s = s.replace(k.as_str(), orig);
            }
        }
        s
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn redactor() -> Redactor {
        Redactor::new(RedactionContext {
            user_names: vec!["VIRAJ".into(), "Jane Doe".into()],
            host_names: vec!["DESKTOP-9KQ2".into()],
        })
    }

    #[test]
    fn nothing_sensitive_survives_realistic_text() {
        let text = r#"
            Open C:\Users\VIRAJ\Documents\Tax 2025\returns.pdf and D:\Clients\Acme Corp\invoice-114.xlsx.
            Mail jane.doe@example.com, server at 192.168.1.44 (also 127.0.0.1), host DESKTOP-9KQ2.
            Account S-1-5-21-1004336348-1177238915-682003330-1001 logged in as viraj.
            aws AKIAIOSFODNN7EXAMPLE key; token=ghp_abcdefghijklmnopqrstuvwxyz0123456789AB
            password: "hunter2 hunter2"
            -----BEGIN RSA PRIVATE KEY-----
            MIIEowIBAAKCAQEA7
            -----END RSA PRIVATE KEY-----
            \\fileserver\finance\budget-q3.xlsx and C:\Windows\System32\drivers\etc\hosts
        "#;
        let out = redactor().redact(text);
        for secret in [
            "VIRAJ",
            "viraj",
            "returns",
            "Clients",
            "invoice-114",
            "jane.doe@example.com",
            "192.168.1.44",
            "DESKTOP-9KQ2",
            "1004336348",
            "AKIAIOSFODNN7EXAMPLE",
            "ghp_abcdefghijklmnopqrstuvwxyz",
            "hunter2",
            "MIIEowIBAAKCAQEA7",
            "fileserver",
            "finance",
            "budget-q3",
        ] {
            assert!(!out.contains(secret), "{secret:?} leaked in:\n{out}");
        }
        // Generic names and loopback survive, so the text is still meaningful.
        // Allow-listed names survive; ones Sentinel has not been told are generic do not.
        assert!(out.contains(r"C:\Windows\System32\<dir-"), "{out}");
        assert!(!out.contains("drivers") && !out.contains("hosts"), "{out}");
        assert!(out.contains("127.0.0.1"));
        assert!(out.contains(r"C:\Users\<USER>\Documents"));
        assert!(
            out.contains(".pdf") && out.contains(".xlsx"),
            "extensions are kept"
        );
    }

    #[test]
    fn placeholders_are_stable_and_reversible_but_secrets_are_not() {
        let mut r = redactor();
        let a = r.redact(r"C:\Clients\Acme\plan.docx and C:\Clients\Acme\notes.docx");
        // The same component gets the same placeholder both times.
        assert_eq!(a.matches("<dir-1>").count(), 2, "{a}");
        assert!(a.contains("<dir-2>"), "{a}");
        assert_eq!(
            r.restore(&a),
            r"C:\Clients\Acme\plan.docx and C:\Clients\Acme\notes.docx"
        );
        let b = r.redact("mail jane@example.com and 10.0.0.7");
        assert!(b.contains("<email-1>") && b.contains("<ip-1>"));
        assert_eq!(r.restore(&b), "mail jane@example.com and 10.0.0.7");
        let s = r.redact("key AKIAIOSFODNN7EXAMPLE");
        assert!(s.contains("<SECRET>"));
        assert!(!r.restore(&s).contains("AKIA"), "secrets never come back");
    }

    #[test]
    fn redaction_is_idempotent() {
        let mut r = redactor();
        let once = r.redact(
            r"C:\Users\VIRAJ\AppData\Local\Temp\x7f3.tmp, mail a@b.io, \\srv\share\a.txt, token=abc123def456",
        );
        let twice = r.redact(&once);
        assert_eq!(once, twice);
    }

    #[test]
    fn unknown_names_are_hidden_not_leaked() {
        let mut r = Redactor::default();
        let out = r.redact(r"C:\Program Files\Secret Project\src\main.rs");
        assert!(out.starts_with(r"C:\Program"), "{out}");
        assert!(!out.contains("main"), "{out}");
        let out = r.redact(r"C:\Stuff\MyStartupIdea\node_modules\left-pad");
        assert!(
            !out.contains("Stuff") && !out.contains("MyStartupIdea"),
            "{out}"
        );
        assert!(out.contains("node_modules"), "generic names survive: {out}");
        assert!(!out.contains("left-pad"), "{out}");
    }

    #[test]
    fn trailing_punctuation_stays_outside_the_path() {
        let out = redactor().redact(r"See C:\Windows\Temp\abc.log, then C:\Windows\Temp.");
        assert!(out.contains(r"C:\Windows\Temp\<file-1>.log,"), "{out}");
        assert!(out.ends_with(r"C:\Windows\Temp."), "{out}");
    }

    #[test]
    fn ordinary_text_is_untouched() {
        let mut r = redactor();
        let t = "Which folders grew the most in the last two weeks? Show 5 over 2 GB.";
        assert_eq!(r.redact(t), t);
        // Version-like numbers are not addresses.
        assert_eq!(
            r.redact("node 22.4.0 and 999.1.1.1"),
            "node 22.4.0 and 999.1.1.1"
        );
    }

    #[test]
    fn short_names_do_not_mangle_words() {
        let mut r = Redactor::new(RedactionContext {
            user_names: vec!["al".into()],
            host_names: vec![],
        });
        assert_eq!(
            r.redact("all folders, also alpha"),
            "all folders, also alpha"
        );
    }
}
