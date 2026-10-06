# AI policy (Phase 8 decisions)

Status: **decided 2026-10-06** by the assistant under the maintainer's instruction to make
these calls without asking ("your choice, your decisions"). Every decision below is
reversible and states what reversing it would require. The maintainer can overrule any of
them; until then the code enforces them.

## Decisions

### 1. Local models only (reversible: needs a threat-model review)

Only a model running on this computer may be used. The provider layer accepts an endpoint
only if its host is a literal loopback address (`127.0.0.1`, `[::1]`, or `localhost`
normalized to `127.0.0.1`), over plain HTTP, and the HTTP client is built **without TLS
support**, so an `https://` or remote endpoint cannot work even by mistake. No OpenAI-
compatible *cloud* provider is implemented. A local server that speaks the OpenAI
protocol could be added later behind the same loopback rule.

Why: the data Sentinel handles (folder names, project names, installed software) describes
a person's life and work, and the value of the AI features here is advisory. Keeping it on
the machine removes a whole class of disclosure risk (T6 in the threat model) rather than
mitigating it.

Verify it any time with `cargo tree -p sentinel-ai | grep -i -E "rustls|native-tls|openssl|schannel|webpki|ring"`,
which must print nothing (checked when this was written). The provider additionally
disables proxies and redirects, because an environment proxy variable would otherwise carry
a "local" request elsewhere and a redirect could bounce it off the machine.

Reversing it: adding a remote provider needs an explicit per-provider consent flow, a
preview of the exact bytes to be sent, and a new row in THREAT_MODEL.md.

### 2. Off by default; everything works without it

`AiConfig::default()` has `enabled: false`. No Sentinel feature depends on AI, and none
shows AI-derived text without labelling it as such.

### 3. The model gets no file data in this milestone

The only input is the person's own typed question (redacted anyway) plus a fixed
vocabulary (the category names). The model's job is to translate that question into a
structured query. Because no file names, paths, project names or file contents are ever put
into a prompt, there is **no prompt-injection surface from the filesystem** yet.

Reversing it: any feature that puts filesystem-derived text in a prompt must (a) pass it
through the redactor, (b) show the person exactly what will be sent before it is sent, and
(c) be reviewed against T6 for injection, since file names are attacker-controlled.

### 4. Redaction always applies, even to a local model

Defense in depth: a local model can still log prompts, and a later provider change must not
silently start leaking. The redactor:

- removes secrets (cloud keys, API tokens, JWTs, private-key blocks, `password=` style
  assignments), e-mail addresses, IPv4 addresses (except loopback), Windows SIDs;
- replaces the account name and computer name with `<USER>` and `<HOST>`;
- rewrites every Windows path with an **allow-list**: a path component survives only if it
  is a well-known, generic name (`Windows`, `Program Files`, `AppData`, `node_modules`,
  `.git`, `Downloads`...). Anything else becomes a stable placeholder (`<dir-1>`,
  `<file-2>.mp4`); a file keeps only its extension. An allow-list fails closed: a name
  Sentinel has never seen is hidden, not leaked;
- is idempotent, and secrets are not restorable (the placeholders for names, paths,
  addresses can be mapped back for display; secrets cannot).

Known limits, stated plainly: paths containing spaces are only partly matched when they
are not one of the allow-listed names (the unmatched remainder of such a path is plain
text), and free text that names a person or project without a path or an e-mail is not
detected. This is why decision 3 matters more than the redactor: the redactor is a second
line, not the first.

### 5. Output is a suggestion, strictly validated, never executed

Model output is parsed with `deny_unknown_fields`, every field is range-checked, and
category names must come from the fixed vocabulary. Anything that does not validate is an
error, never repaired by guessing. The AI has no handle to the executor or to any deletion
path; a query is read-only filtering of saved analyses.

### 6. How redaction is verified

Unit tests with realistic sample text assert that none of a list of sensitive strings
appears in the output (a "no leak" property), that the output is idempotent, that mapping
is stable and reversible for non-secrets, and that secrets are irreversibly removed. Any
future feature that sends filesystem-derived text adds a test of the same shape for its
inputs, plus the send-preview requirement above.

## What this milestone builds

`crates/sentinel-ai`: the `AiProvider` trait, the loopback-only Ollama provider, the
redactor, the structured-query schema and validator, and the translation pipeline joining
them. No UI, no Tauri command and no settings toggle yet.
