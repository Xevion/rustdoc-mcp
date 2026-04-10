import { defineConfig, presets, runners } from "@xevion/tempo";

const serverPreset = presets.rust();

export default defineConfig({
  subsystems: {
    server: {
      ...serverPreset,
      aliases: ["s", "srv"],
      commands: {
        ...serverPreset.commands,
        lint: "cargo clippy --workspace --all-targets",
        "dep-check": {
          cmd: "cargo machete --with-metadata",
        },
        deny: {
          cmd: "cargo deny check",
          requires: [{ tool: "cargo-deny", hint: "Install with `cargo install cargo-deny`" }],
        },
      },
    },
  },
  commands: {
    check: runners.check({ autoFixStrategy: "fix-first" }),
    fmt: runners.sequential("format-apply", { description: "Sequential per-subsystem formatting", autoFixFallback: true }),
    lint: runners.sequential("lint", { description: "Sequential per-subsystem linting" }),
    "pre-commit": runners.preCommit(),
  },
});
