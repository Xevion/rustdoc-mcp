import { defineConfig, presets } from "@xevion/tempo";

export default defineConfig({
  subsystems: {
    server: {
      ...presets.rust(),
      aliases: ["s", "srv"],
      commands: {
        ...presets.rust().commands,
        lint: "cargo clippy --workspace --all-targets",
        "dep-check": {
          cmd: "cargo machete --with-metadata",
          hint: "Remove unused dependencies from Cargo.toml",
        },
        deny: {
          cmd: "cargo deny check",
          hint: "Run `cargo deny init` to create deny.toml if missing",
          requires: ["cargo-deny"],
        },
      },
    },
  },
  check: {
    autoFixStrategy: "fix-first",
  },
});
