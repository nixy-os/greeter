# Nixy greeter

A minimal Rust/GTK4 login form for greetd, designed to run under a dedicated Wayland compositor. Empty manual username and password fields, keyboard navigation, and reboot/shutdown controls. No account list, session selector, autologin, or shell command entry.

## Develop and preview

```sh
nix develop --command cargo test --locked
nix develop --command cargo clippy --locked --all-targets -- -D warnings
nix develop --command cargo run --locked -- --demo
nix run . -- --demo
nix build
nix flake check
```

The flake exposes `packages.x86_64-linux.default` (also `nixy-greeter`), a pinned development shell, and a package check that runs the Rust tests. `shell.nix` remains available for callers supplying their own `pkgs`. Cargo.lock and `cargoHash` in `package.nix` pin Rust dependencies; update the hash when dependency changes require it.

Demo mode never connects to greetd or executes session/power commands. Any credentials simulate a successful login. It opens a regular window so it can be previewed without replacing your login manager. Production mode requests a fullscreen window.

## Production

Run as the dedicated greeter user inside a Wayland compositor launched by greetd:

```sh
nixy-greeter --session /absolute/session-launcher --systemctl /absolute/systemctl
```

The session launcher is an administrator-owned executable, typically `exec uwsm start hyprland-uwsm.desktop`. Arguments are not interpreted by a shell inside the client. The compositor must exit when the client exits; greetd then starts the authenticated user session. Optional `--style /path/style.css` overrides the built-in palette.

greetd/PAM owns authentication. The client supports secret and visible challenges, information/error messages, cancellation, and retries. It supplies the initial password only to the conventional `Password:` challenge; other prompts require an explicit response. PAM text is displayed as plain text. Protocol exchanges run on a worker thread, with a 30-second transport timeout. A failed connection is not silently reused. Passwords are never logged or passed as command-line arguments, and owned pending strings are cleared after use; GTK and serialization buffers are not guaranteed to be zeroized.

Power controls require two clicks and invoke only the configured systemctl executable with `--no-ask-password` and `reboot` or `poweroff`. Authorization belongs to logind/polkit in the OS integration. Denials are shown without invoking sudo. Demo mode simulates these actions.

## Integration ownership

This repository owns the application, Nix package, and Cargo dependency lock. `/home/peter/.nix-config` consumes the public `github:nixy-os/greeter` flake at a revision pinned in its lock file. It owns host display modes, early KMS/Plymouth, greetd/PAM, UWSM, power policy, and deployment.

After validating and publishing an application commit, run `make update-input INPUT=greeter` in the OS repository, then its evaluation, VM, and host build checks. Local application edits do not affect deployment until that pinned input is updated. For integration development, use `nix build --no-link .#nixy-greeter --override-input greeter path:/home/peter/Dev/nixy-os/greeter --no-write-lock-file` from the OS repository.

The OS input follows its own nixpkgs. Standalone and OS builds reuse the same store result when source, package settings, system, and nixpkgs revision match. The source fileset excludes documentation and flake metadata so documentation changes alone do not rebuild the application. Different nixpkgs revisions can require a rebuild; a remote flake does not itself provide a binary cache.

Hardware acceptance still requires boot/login/logout testing on each host after an explicitly authorized activation. Unit tests or a preview do not prove flicker-free physical GPU handoffs.
