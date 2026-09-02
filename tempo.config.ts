import { defineConfig, presets, task } from "@xevion/tempo";

export default defineConfig({
  tasks: [
    ...presets.rust({
      name: "server",
      override: {
        lint: "cargo clippy --workspace --all-targets -- -D warnings",
      },
    }),
    task({
      name: "server:dep-check",
      body: "cargo machete --with-metadata",
      tags: ["check"],
      requires: [{ tool: "cargo-machete", hint: "cargo install cargo-machete" }],
    }),
    task({
      name: "server:deny",
      body: "cargo deny check bans sources licenses",
      tags: ["check"],
      requires: [{ tool: "cargo-deny", hint: "cargo install cargo-deny" }],
    }),
  ],
  commands: {
    check: { description: "Run every check", tags: ["check"] },
    fmt: { description: "Apply every formatter", tags: ["format"], concurrency: 1 },
    lint: { description: "Clippy only", tags: ["lint"] },
  },
});
