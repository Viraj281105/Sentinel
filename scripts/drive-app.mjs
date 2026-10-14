// Developer tool, not shipped. Drives the REAL Sentinel window (WebView2) over the DevTools protocol, so the UI and
// the real backend can be exercised and screenshotted. See docs/DEVELOPMENT.md, "Driving the real app".
// Tiny DevTools-protocol driver for the Sentinel window (WebView2 remote debugging).
//   node cdp.mjs eval "<js expression>"        evaluate in the page, print JSON result
//   node cdp.mjs shot <file.png>               screenshot of the page
//   node cdp.mjs text                          visible text of the page
//   node cdp.mjs click "<visible text>" [nth]  click a button/link/tab by its text
//   node cdp.mjs nav "<page label>"            click a sidebar item
//   node cdp.mjs type "<aria-label or placeholder>" "<value>"   set an input like a person typing
//   node cdp.mjs wait "<text>" [seconds]       wait until text appears
import fs from "node:fs";

const PORT = 9222;
const [cmd, ...args] = process.argv.slice(2);

async function target() {
  const list = await (await fetch(`http://127.0.0.1:${PORT}/json`)).json();
  const page = list.find((t) => t.type === "page" && !t.url.startsWith("devtools:"));
  if (!page) throw new Error("no page target: " + JSON.stringify(list));
  return page;
}

function connect(url) {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(url);
    let id = 0;
    const pending = new Map();
    const handlers = [];
    ws.onmessage = (m) => {
      const d = JSON.parse(m.data);
      if (d.method) handlers.forEach((h) => h(d.method, d.params));
      if (d.id && pending.has(d.id)) {
        const { res, rej } = pending.get(d.id);
        pending.delete(d.id);
        d.error ? rej(new Error(JSON.stringify(d.error))) : res(d.result);
      }
    };
    ws.onerror = (e) => reject(new Error("ws error"));
    ws.onopen = () =>
      resolve({
        send: (method, params = {}) =>
          new Promise((res, rej) => {
            const i = ++id;
            pending.set(i, { res, rej });
            ws.send(JSON.stringify({ id: i, method, params }));
          }),
        onEvent: (name, fn) => handlers.push((m, p) => m === name && fn(p)),
        close: () => ws.close(),
      });
  });
}

const page = await connect((await target()).webSocketDebuggerUrl);
const evaluate = async (expression) => {
  const r = await page.send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
  if (r.exceptionDetails) throw new Error("page error: " + JSON.stringify(r.exceptionDetails.exception?.description ?? r.exceptionDetails));
  return r.result.value;
};

// Find a clickable element by its visible text (exact match preferred, then contains).
const FIND = `
(text, nth) => {
  const q = [...document.querySelectorAll('button, a, [role="tab"], [role="button"], label, summary')];
  const norm = (s) => (s || '').replace(/\\s+/g, ' ').trim();
  const exact = q.filter((e) => norm(e.textContent) === text || norm(e.getAttribute('aria-label')) === text);
  const part = q.filter((e) => norm(e.textContent).includes(text) || norm(e.getAttribute('aria-label')).includes(text));
  const list = exact.length ? exact : part;
  return list[nth || 0] || null;
}`;

try {
  if (cmd === "eval") {
    console.log(JSON.stringify(await evaluate(args[0]), null, 2));
  } else if (cmd === "text") {
    console.log(await evaluate("document.body.innerText"));
  } else if (cmd === "shot") {
    if (args[1]) {
      await page.send("Emulation.setEmulatedMedia", { features: [{ name: "prefers-color-scheme", value: args[1] }] });
      await new Promise((r) => setTimeout(r, 700));
    }
    const r = await page.send("Page.captureScreenshot", { format: "png" });
    fs.writeFileSync(args[0], Buffer.from(r.data, "base64"));
    console.log("saved " + args[0]);
  } else if (cmd === "click" || cmd === "nav") {
    const text = args[0];
    const nth = Number(args[1] || 0);
    const scope = cmd === "nav" ? "aside " : "";
    const ok = await evaluate(`(() => {
      const find = ${FIND};
      const root = ${cmd === "nav" ? "document.querySelector('aside')" : "document"};
      const orig = document.querySelectorAll;
      const el = (function() {
        const q = [...root.querySelectorAll('button, a, [role="tab"], [role="button"], label, summary')];
        const norm = (s) => (s || '').replace(/\\s+/g, ' ').trim();
        const exact = q.filter((e) => norm(e.textContent) === ${JSON.stringify(text)} || norm(e.getAttribute('aria-label')) === ${JSON.stringify(text)});
        const part = q.filter((e) => norm(e.textContent).includes(${JSON.stringify(text)}) || norm(e.getAttribute('aria-label')).includes(${JSON.stringify(text)}));
        return (exact.length ? exact : part)[${nth}] || null;
      })();
      if (!el) return false;
      if (el.disabled) return 'disabled';
      el.scrollIntoView({ block: 'center' });
      el.click();
      return true;
    })()`);
    console.log(ok === true ? "clicked" : ok === "disabled" ? "DISABLED: " + text : "NOT FOUND: " + text);
    if (ok !== true) process.exitCode = 2;
  } else if (cmd === "type") {
    const [label, value] = args;
    const ok = await evaluate(`(() => {
      const els = [...document.querySelectorAll('input, textarea')];
      const el = els.find((e) => e.getAttribute('aria-label') === ${JSON.stringify(label)} || e.placeholder === ${JSON.stringify(label)} || (e.labels && [...e.labels].some((l) => l.textContent.includes(${JSON.stringify(label)}))));
      if (!el) return false;
      el.focus();
      const proto = el.tagName === 'TEXTAREA' ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
      Object.getOwnPropertyDescriptor(proto, 'value').set.call(el, ${JSON.stringify(value)});
      el.dispatchEvent(new Event('input', { bubbles: true }));
      return true;
    })()`);
    console.log(ok ? "typed" : "NOT FOUND: " + label);
    if (!ok) process.exitCode = 2;
  } else if (cmd === "wait") {
    const secs = Number(args[1] || 20);
    const end = Date.now() + secs * 1000;
    let found = false;
    while (Date.now() < end) {
      if (await evaluate(`document.body.innerText.includes(${JSON.stringify(args[0])})`)) { found = true; break; }
      await new Promise((r) => setTimeout(r, 300));
    }
    console.log(found ? "found" : "TIMEOUT waiting for: " + args[0]);
    if (!found) process.exitCode = 2;
  } else if (cmd === "media") {
    await page.send("Emulation.setEmulatedMedia", { features: [{ name: "prefers-color-scheme", value: args[0] }] });
    console.log("media " + args[0]);
    await new Promise((r) => setTimeout(r, 600));
  } else if (cmd === "key") {
    const name = args[0];
    const n = Number(args[1] || 1);
    const info = { Tab: { key: "Tab", code: "Tab", windowsVirtualKeyCode: 9 }, Enter: { key: "Enter", code: "Enter", windowsVirtualKeyCode: 13 } }[name];
    for (let i = 0; i < n; i++) {
      await page.send("Input.dispatchKeyEvent", { type: "keyDown", ...info });
      await page.send("Input.dispatchKeyEvent", { type: "keyUp", ...info });
    }
    console.log("pressed " + name + " x" + n);
  } else if (cmd === "intercept") {
    // node cdp.mjs intercept <substring of the ipc URL> <json body to answer with> <seconds>
    const [needle, body, secs] = args;
    await page.send("Fetch.enable", { patterns: [{ urlPattern: "*ipc.localhost*" }] });
    page.onEvent("Fetch.requestPaused", async (ev) => {
      const url = decodeURIComponent(ev.request.url);
      if (!url.includes(needle)) {
        await page.send("Fetch.continueRequest", { requestId: ev.requestId });
        return;
      }
      const headers = [
        { name: "Content-Type", value: "application/json" },
        { name: "Tauri-Response", value: "ok" },
        { name: "Access-Control-Allow-Origin", value: "*" },
        { name: "Access-Control-Allow-Headers", value: "*" },
        { name: "Access-Control-Expose-Headers", value: "Tauri-Response" },
      ];
      console.log("answered " + ev.request.method + " " + url.slice(0, 80));
      await page.send("Fetch.fulfillRequest", {
        requestId: ev.requestId,
        responseCode: 200,
        responseHeaders: headers,
        body: ev.request.method === "OPTIONS" ? "" : Buffer.from(body).toString("base64"),
      });
    });
    console.log("ready");
    await new Promise((r) => setTimeout(r, Number(secs || 30) * 1000));
    await page.send("Fetch.disable");
  } else {
    console.log("unknown command");
  }
} finally {
  page.close();
}
