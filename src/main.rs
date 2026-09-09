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
    fn busy(&self, busy: bool) {
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
        self.busy(true);
        if self.tx.send(Work::Auth(request)).is_err() {
            self.disconnected();
        }
    }

    fn disconnected(&self) {
        self.connected.set(false);
        self.password.set_text("");
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
        self.password.set_text("");
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
                self.password.set_text("");
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
        if self.busy.get() || !self.connected.get() {
            return;
        }
        self.power_confirm.set(None);
        let step = if self.notice.get() {
            self.login.borrow_mut().acknowledge()
        } else if self.challenge.get() {
            let answer = self.password.text().to_string();
            self.password.set_text("");
            self.login.borrow_mut().answer(answer)
        } else {
            let username = self.username.text().trim().to_string();
            if username.is_empty() {
                self.username.grab_focus();
                return;
            }
            let password = self.password.text().to_string();
            self.password.set_text("");
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
