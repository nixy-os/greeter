# Nixy greeter

A minimal Rust/GTK4 login form for greetd, designed to run under a dedicated Wayland compositor. Empty manual username and password fields, keyboard navigation, and reboot/shutdown controls. No account list, session selector, autologin, or shell command entry.

## Develop and preview

```sh
nix-shell --run 'cargo test --locked'
nix-shell --run 'cargo run --locked -- --demo'
```

`shell.nix` accepts a `pkgs` argument to use a pinned nixpkgs checkout. The NixOS integration pins dependencies with its flake lock and this repository's Cargo.lock.

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

This repository owns the application and Cargo dependency lock. `/home/peter/.nix-config` owns host display modes, early KMS/Plymouth, greetd/PAM, UWSM, power policy, and deployment. Until a remote release is published, it carries a complete versioned source archive of this application so builds never depend on this mutable development directory. Update the archive only after tests and UI checks pass; do not edit its extracted contents independently.

Hardware acceptance still requires boot/login/logout testing on each host after an explicitly authorized activation. Unit tests or a preview do not prove flicker-free physical GPU handoffs.
