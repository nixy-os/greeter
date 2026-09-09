use greetd_ipc::{AuthMessageType, Request, Response, codec::SyncCodec};
use gtk::{gio, glib, prelude::*};
use nixy_greeter::{Login, Step};
use std::{
    cell::{Cell, RefCell},
    os::unix::net::UnixStream,
    rc::Rc,
    sync::mpsc,
    time::Duration,
};
use zeroize::Zeroize;

enum Work {
    Auth(Request),
    Power(&'static str),
}
enum Event {
    Auth(Result<Response, ()>),
    Power(bool),
}

struct Options {
    demo: bool,
    session: String,
    systemctl: String,
    style: Option<String>,
}

fn options() -> Result<Options, String> {
    let mut result = Options {
        demo: false,
        session: String::new(),
        systemctl: String::new(),
        style: None,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--demo" => result.demo = true,
            "--session" => result.session = args.next().ok_or("Missing session executable")?,
            "--systemctl" => {
                result.systemctl = args.next().ok_or("Missing systemctl executable")?
            }
            "--style" => result.style = Some(args.next().ok_or("Missing stylesheet")?),
            "--help" => {
                println!(
                    "nixy-greeter [--demo] --session /absolute/session-launcher --systemctl /absolute/systemctl [--style path]"
                );
                std::process::exit(0);
            }
            _ => return Err("Unknown argument; use --help".into()),
        }
    }
    if !result.demo && (!result.session.starts_with('/') || !result.systemctl.starts_with('/')) {
        return Err("Absolute --session and --systemctl executables are required".into());
    }
    Ok(result)
}

fn worker(options: &Options) -> (mpsc::Sender<Work>, mpsc::Receiver<Event>) {
    let (tx, work) = mpsc::channel();
    let (events, rx) = mpsc::channel();
    let demo = options.demo;
    let systemctl = options.systemctl.clone();
    let socket_path = std::env::var_os("GREETD_SOCK");
    std::thread::spawn(move || {
        let mut socket = if demo {
            None
        } else {
            socket_path.and_then(|path| UnixStream::connect(path).ok())
        };
        if let Some(socket) = &socket {
            let _ = socket.set_read_timeout(Some(Duration::from_secs(30)));
            let _ = socket.set_write_timeout(Some(Duration::from_secs(30)));
        }
        for work in work {
            let event = match work {
                Work::Auth(mut request) => {
                    let response = if demo {
                        Ok(match request {
                            Request::CreateSession { .. } => Response::AuthMessage {
                                auth_message_type: AuthMessageType::Secret,
                                auth_message: "Password:".into(),
                            },
                            _ => Response::Success,
                        })
                    } else if let Some(stream) = socket.as_mut() {
                        let result = request
                            .write_to(stream)
                            .and_then(|_| Response::read_from(stream))
                            .map_err(|_| ());
                        if result.is_err() {
                            socket = None;
                        }
                        result
                    } else {
                        Err(())
                    };
                    if let Request::PostAuthMessageResponse {
                        response: Some(ref mut secret),
                    } = request
                    {
                        secret.zeroize();
                    }
                    Event::Auth(response)
                }
                Work::Power(action) => Event::Power(
                    demo || std::process::Command::new(&systemctl)
                        .args(["--no-ask-password", action])
                        .stdin(std::process::Stdio::null())
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .status()
                        .is_ok_and(|s| s.success()),
                ),
            };
            if events.send(event).is_err() {
                break;
            }
        }
    });
    (tx, rx)
}

struct Form {
    login: RefCell<Login>,
    tx: mpsc::Sender<Work>,
    app: gtk::Application,
    demo: bool,
    busy: Cell<bool>,
    pending_mask: Cell<bool>,
    connected: Cell<bool>,
    challenge: Cell<bool>,
    notice: Cell<bool>,
    power_confirm: Cell<Option<&'static str>>,
    username: gtk::Entry,
    password: gtk::Entry,
    password_label: gtk::Label,
    message: gtk::Label,
    submit: gtk::Button,
    cancel: gtk::Button,
    reboot: gtk::Button,
    shutdown: gtk::Button,
}

impl Form {
    fn clear_password(&self) {
        self.password.set_text("");
        self.pending_mask.set(false);
    }

    fn take_answer(&self) -> String {
        let answer = self.password.text().to_string();
        self.busy(true);
        self.clear_password();
        if !EntryExt::is_visible(&self.password) && !answer.is_empty() {
            // Preserve only the displayed length while waiting, never the credential.
            self.password.set_text(&"•".repeat(answer.chars().count()));
            self.pending_mask.set(true);
        }
        answer
    }

    fn busy(&self, busy: bool) {
        if busy && let Some(window) = self.password.root().and_downcast::<gtk::Window>() {
            GtkWindowExt::set_focus(&window, None::<&gtk::Widget>);
        }
        if !busy && self.pending_mask.get() {
            self.clear_password();
        }
        self.busy.set(busy);
        let can_login = !busy && self.connected.get();
        self.username
            .set_sensitive(can_login && !self.challenge.get());
        self.password.set_sensitive(can_login && !self.notice.get());
        self.submit.set_sensitive(can_login);
        self.cancel.set_sensitive(can_login && self.challenge.get());
        self.submit.set_visible(self.notice.get());
        self.cancel.set_visible(self.challenge.get());
        self.reboot.set_sensitive(!busy);
        self.shutdown.set_sensitive(!busy);
    }

    fn send(&self, request: Request) {
        if matches!(request, Request::CancelSession) {
            self.clear_password();
        }
        self.busy(true);
        if self.tx.send(Work::Auth(request)).is_err() {
            self.disconnected();
        }
    }

    fn disconnected(&self) {
        self.connected.set(false);
        self.clear_password();
        // Drop all pending password material and do not reuse a broken IPC stream.
        self.login.borrow_mut().cancel("");
        self.message
            .set_text("Login service unavailable. Use another console to recover.");
        self.busy(true);
        self.reboot.set_sensitive(true);
        self.shutdown.set_sensitive(true);
    }

    fn reset(&self, message: &str) {
        self.challenge.set(false);
        self.notice.set(false);
        self.clear_password();
        self.password.set_visibility(false);
        self.password.set_input_purpose(gtk::InputPurpose::Password);
        self.password_label.set_text("Password:");
        self.submit.set_label("Log in");
        self.message.set_text(message);
        self.busy(false);
        self.username.grab_focus();
    }

    fn apply(&self, step: Step) {
        match step {
            Step::Send(request) => self.send(request),
            Step::Prompt { text, secret } => {
                self.challenge.set(true);
                self.notice.set(false);
                self.clear_password();
                self.password.set_visibility(!secret);
                self.password.set_input_purpose(if secret {
                    gtk::InputPurpose::Password
                } else {
                    gtk::InputPurpose::FreeForm
                });
                self.password_label.set_text(&text);
                self.submit.set_label("Continue");
                self.message.set_text("");
                self.busy(false);
                self.password.grab_focus();
            }
            Step::Notice { text } => {
                self.clear_password();
                self.challenge.set(true);
                self.notice.set(true);
                self.message.set_text(&text);
                self.submit.set_label("Continue");
                self.busy(false);
                self.submit.grab_focus();
            }
            Step::Reset { message } => self.reset(&message),
            Step::Finished if self.demo => self.reset("Demo complete — no session was started."),
            Step::Finished => self.app.quit(),
            Step::Fatal => self.disconnected(),
        }
    }

    fn submit(&self) {
        if self.busy.get() || self.pending_mask.get() || !self.connected.get() {
            return;
        }
        self.power_confirm.set(None);
        let step = if self.notice.get() {
            self.login.borrow_mut().acknowledge()
        } else if self.challenge.get() {
            let answer = self.take_answer();
            self.login.borrow_mut().answer(answer)
        } else {
            let username = self.username.text().trim().to_string();
            if username.is_empty() {
                self.username.grab_focus();
                return;
            }
            let password = self.take_answer();
            self.message.set_text("Logging in…");
            self.login.borrow_mut().begin(username, password)
        };
        self.apply(step);
    }

    fn power(&self, action: &'static str) {
        if self.power_confirm.get() != Some(action) {
            self.power_confirm.set(Some(action));
            self.message.set_text(if action == "reboot" {
                "Press Reboot again to confirm."
            } else {
                "Press Shutdown again to confirm."
            });
            return;
        }
        self.power_confirm.set(None);
        self.busy(true);
        if self.tx.send(Work::Power(action)).is_err() {
            self.disconnected();
        }
    }
}

fn ui(app: &gtk::Application, options: &Options) {
    let provider = gtk::CssProvider::new();
    provider.load_from_data(include_str!("../assets/style.css"));
    gtk::style_context_add_provider_for_display(
        &gtk::gdk::Display::default().expect("GTK display"),
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
    if let Some(path) = &options.style {
        let override_css = gtk::CssProvider::new();
        override_css.load_from_path(path);
        gtk::style_context_add_provider_for_display(
            &gtk::gdk::Display::default().expect("GTK display"),
            &override_css,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
        );
    }
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("Nixy login")
        .default_width(900)
        .default_height(650)
        .decorated(options.demo)
        .build();
    if !options.demo {
        window.fullscreen();
    }
    let layout = gtk::Overlay::new();
    let outer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    outer.set_halign(gtk::Align::Center);
    outer.set_valign(gtk::Align::Center);
    outer.set_margin_start(24);
    outer.set_margin_end(24);
    let brand = gtk::Label::new(Some(include_str!("../assets/nixy-logo.txt").trim_end()));
    brand.add_css_class("brand");
    brand.set_xalign(0.0);
    outer.append(&brand);
    let hostname = gtk::Label::new(Some(&glib::host_name()));
    hostname.add_css_class("hostname");
    outer.append(&hostname);
    let panel = gtk::Box::new(gtk::Orientation::Vertical, 12);
    panel.set_halign(gtk::Align::Center);
    let fields = gtk::Grid::builder()
        .column_spacing(12)
        .row_spacing(12)
        .build();
    let username = gtk::Entry::builder().hexpand(true).width_chars(22).build();
    let username_label = gtk::Label::new(Some("Username:"));
    username_label.set_xalign(1.0);
    username_label.set_mnemonic_widget(Some(&username));
    let password = gtk::Entry::builder()
        .visibility(false)
        .input_purpose(gtk::InputPurpose::Password)
        .build();
    password.set_invisible_char(Some('•'));
    let password_label = gtk::Label::new(Some("Password:"));
    password_label.set_xalign(1.0);
    password_label.set_mnemonic_widget(Some(&password));
    password_label.set_wrap(true);
    password_label.set_max_width_chars(18);
    fields.attach(&username_label, 0, 0, 1, 1);
    fields.attach(&username, 1, 0, 1, 1);
    fields.attach(&password_label, 0, 1, 1, 1);
    fields.attach(&password, 1, 1, 1, 1);
    panel.append(&fields);
    let message = gtk::Label::new(None);
    message.add_css_class("message");
    message.set_wrap(true);
    message.set_max_width_chars(38);
    message.set_lines(4);
    panel.append(&message);
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    buttons.set_halign(gtk::Align::End);
    let cancel = gtk::Button::with_label("Cancel");
    cancel.set_sensitive(false);
    cancel.set_visible(false);
    let submit = gtk::Button::with_label("Log in");
    submit.set_visible(false);
    buttons.append(&cancel);
    buttons.append(&submit);
    panel.append(&buttons);
    outer.append(&panel);
    let power = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    power.set_halign(gtk::Align::Center);
    power.set_valign(gtk::Align::End);
    power.set_margin_bottom(28);
    let shutdown = gtk::Button::with_label("Shutdown");
    let reboot = gtk::Button::with_label("Reboot");
    power.append(&reboot);
    power.append(&shutdown);
    layout.set_child(Some(&outer));
    layout.add_overlay(&power);
    window.set_child(Some(&layout));
    let (tx, rx) = worker(options);
    let form = Rc::new(Form {
        login: RefCell::new(Login::new(vec![options.session.clone()])),
        tx,
        app: app.clone(),
        demo: options.demo,
        busy: Cell::new(false),
        pending_mask: Cell::new(false),
        connected: Cell::new(true),
        challenge: Cell::new(false),
        notice: Cell::new(false),
        power_confirm: Cell::new(None),
        username,
        password,
        password_label,
        message,
        submit,
        cancel,
        reboot,
        shutdown,
    });
    let f = form.clone();
    form.submit.connect_clicked(move |_| f.submit());
    let f = form.clone();
    form.password.connect_activate(move |_| f.submit());
    let f = form.clone();
    form.username.connect_activate(move |_| {
        f.password.grab_focus();
    });
    let f = form.clone();
    form.cancel.connect_clicked(move |_| {
        let step = f.login.borrow_mut().cancel("");
        f.apply(step);
    });
    let f = form.clone();
    form.reboot.connect_clicked(move |_| f.power("reboot"));
    let f = form.clone();
    form.shutdown.connect_clicked(move |_| f.power("poweroff"));
    let f = form.clone();
    let timer = glib::timeout_add_local(Duration::from_millis(25), move || {
        while let Ok(event) = rx.try_recv() {
            match event {
                Event::Auth(Ok(response)) => {
                    let step = f.login.borrow_mut().handle(response);
                    f.apply(step);
                }
                Event::Auth(Err(())) => f.disconnected(),
                Event::Power(success) => {
                    f.message.set_text(if f.demo {
                        "Demo only — no power action was performed."
                    } else if success {
                        "Power action requested."
                    } else {
                        "Power action was denied or failed."
                    });
                    f.busy(false);
                }
            }
        }
        glib::ControlFlow::Continue
    });
    let timer = RefCell::new(Some(timer));
    app.connect_shutdown(move |_| {
        if let Some(timer) = timer.borrow_mut().take() {
            timer.remove();
        }
    });
    window.present();
    form.username.grab_focus();
}

fn main() -> glib::ExitCode {
    let options = match options() {
        Ok(o) => o,
        Err(e) => {
            eprintln!("{e}");
            return glib::ExitCode::FAILURE;
        }
    };
    if !options.demo && std::env::var_os("GREETD_SOCK").is_none() {
        eprintln!("GREETD_SOCK is missing; launch through greetd or use --demo");
        return glib::ExitCode::FAILURE;
    }
    let app = gtk::Application::builder()
        .application_id("org.nixy.Greeter")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.connect_activate(move |app| ui(app, &options));
    app.run_with_args::<&str>(&[])
}

#[cfg(test)]
mod tests {
    use super::*;
    use greetd_ipc::ErrorType;

    #[test]
    #[ignore = "requires an isolated GTK display (run with xvfb-run)"]
    fn pending_password_is_only_a_mask_and_clears_at_auth_boundaries() {
        gtk::init().unwrap();
        let (tx, rx) = mpsc::channel();
        let form = Form {
            login: RefCell::new(Login::new(vec!["/session".into()])),
            tx,
            app: gtk::Application::default(),
            demo: true,
            busy: Cell::new(false),
            pending_mask: Cell::new(false),
            connected: Cell::new(true),
            challenge: Cell::new(false),
            notice: Cell::new(false),
            power_confirm: Cell::new(None),
            username: gtk::Entry::new(),
            password: gtk::Entry::builder().visibility(false).build(),
            password_label: gtk::Label::new(None),
            message: gtk::Label::new(None),
            submit: gtk::Button::new(),
            cancel: gtk::Button::new(),
            reboot: gtk::Button::new(),
            shutdown: gtk::Button::new(),
        };
        let respond = |response| {
            let step = form.login.borrow_mut().handle(response);
            form.apply(step);
        };
        let prompt = |text: &str, secret| Response::AuthMessage {
            auth_message_type: if secret {
                AuthMessageType::Secret
            } else {
                AuthMessageType::Visible
            },
            auth_message: text.into(),
        };
        form.username.set_text("alice");
        // GTK masks Unicode scalar values, not UTF-8 bytes or grapheme clusters.
        let password = "é🔑e\u{301}";
        form.password.set_text(password);
        form.submit();
        assert_eq!(form.password.text(), "••••");
        assert!(!form.password.is_sensitive());
        assert_eq!(form.message.text(), "Logging in…");
        assert!(matches!(
            rx.try_recv(),
            Ok(Work::Auth(Request::CreateSession { .. }))
        ));
        form.submit();
        assert!(rx.try_recv().is_err());
        respond(prompt("Password:", true));
        assert!(
            matches!(rx.try_recv(), Ok(Work::Auth(Request::PostAuthMessageResponse { response: Some(s) })) if s == password)
        );
        assert_eq!(form.password.text(), "••••");

        respond(prompt("Verification code:", true));
        assert!(form.password.text().is_empty());
        assert!(!form.pending_mask.get());
        assert!(form.password.is_sensitive());
        form.password.set_text("123456");
        form.submit();
        assert_eq!(form.password.text(), "••••••");
        assert!(
            matches!(rx.try_recv(), Ok(Work::Auth(Request::PostAuthMessageResponse { response: Some(s) })) if s == "123456")
        );

        respond(prompt("Account:", false));
        assert!(form.password.text().is_empty());
        form.password.set_text("public-response");
        form.submit();
        assert!(form.password.text().is_empty());
        assert!(
            matches!(rx.try_recv(), Ok(Work::Auth(Request::PostAuthMessageResponse { response: Some(s) })) if s == "public-response")
        );

        respond(prompt("Password:", true));
        form.submit();
        assert!(form.password.text().is_empty());
        assert!(
            matches!(rx.try_recv(), Ok(Work::Auth(Request::PostAuthMessageResponse { response: Some(s) })) if s.is_empty())
        );

        respond(prompt("Password:", true));
        form.password.set_text("another-secret");
        form.submit();
        rx.try_recv().unwrap();
        respond(Response::AuthMessage {
            auth_message_type: AuthMessageType::Info,
            auth_message: "Notice".into(),
        });
        assert!(form.password.text().is_empty());
        form.submit();
        assert!(matches!(
            rx.try_recv(),
            Ok(Work::Auth(Request::PostAuthMessageResponse {
                response: None
            }))
        ));

        respond(prompt("Password:", true));
        form.password.set_text("bad-password");
        form.submit();
        rx.try_recv().unwrap();
        respond(Response::Error {
            error_type: ErrorType::AuthError,
            description: "private".into(),
        });
        assert!(form.password.text().is_empty());
        assert!(matches!(
            rx.try_recv(),
            Ok(Work::Auth(Request::CancelSession))
        ));
        respond(Response::Success);
        assert!(form.password.is_sensitive());
        assert!(!form.pending_mask.get());

        for ending in [
            Step::Reset {
                message: String::new(),
            },
            Step::Finished,
            Step::Send(Request::CancelSession),
            Step::Fatal,
        ] {
            form.reset("");
            form.password.set_text("secret");
            form.submit();
            assert!(form.pending_mask.get());
            form.apply(ending);
            assert!(form.password.text().is_empty());
            assert!(!form.pending_mask.get());
        }
    }
}
