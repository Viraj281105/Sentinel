import type { Runtime } from "../bindings/Runtime";

/** Display names, in the order runtimes are listed. */
export const RUNTIME_NAMES: Record<Runtime, string> = {
  node: "Node.js",
  python: "Python",
  rust: "Rust",
  go: "Go",
  java: "Java",
  dotNet: ".NET",
  androidSdk: "Android SDK",
};
