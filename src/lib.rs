//! Authentication policy. All verification is delegated to greetd/PAM.
//! No request, response, username, or password is logged.
use greetd_ipc::{AuthMessageType, Request, Response};
use zeroize::Zeroizing;

#[derive(PartialEq)]
enum State {
    Idle,
    Authenticating,
    Starting,
    Cancelling,
}

pub enum Step {
    Send(Request),
    Prompt { text: String, secret: bool },
    Notice { text: String },
    Reset { message: String },
    Finished,
    Fatal,
}

pub struct Login {
    state: State,
    password: Option<Zeroizing<String>>,
    command: Vec<String>,
    failure: String,
    cancel_retried: bool,
}

impl Login {
    pub fn new(command: Vec<String>) -> Self {
        Self {
            state: State::Idle,
            password: None,
            command,
            failure: String::new(),
            cancel_retried: false,
        }
    }

    pub fn begin(&mut self, username: String, password: String) -> Step {
        self.state = State::Authenticating;
        self.password = Some(Zeroizing::new(password));
        Step::Send(Request::CreateSession { username })
    }

    pub fn answer(&mut self, answer: String) -> Step {
        Step::Send(Request::PostAuthMessageResponse {
            response: Some(answer),
        })
    }

    pub fn acknowledge(&mut self) -> Step {
        Step::Send(Request::PostAuthMessageResponse { response: None })
    }

    pub fn cancel(&mut self, message: &str) -> Step {
        self.password = None;
        self.failure = message.into();
        self.state = State::Cancelling;
        self.cancel_retried = false;
        Step::Send(Request::CancelSession)
    }

    pub fn handle(&mut self, response: Response) -> Step {
        if self.state == State::Cancelling {
            return match response {
                Response::Success => {
                    self.state = State::Idle;
                    Step::Reset {
                        message: std::mem::take(&mut self.failure),
                    }
                }
                // greetd 0.10 takes the configuring session out of its state
                // before sending cancellation to the PAM child. If that child
                // already exited, the send fails although the state was cleared.
                // Confirm the now-empty state with one idempotent cancel request.
                Response::Error { .. } if !self.cancel_retried => {
                    self.cancel_retried = true;
                    Step::Send(Request::CancelSession)
                }
                _ => Step::Fatal,
            };
        }
        match response {
            Response::Success if self.state == State::Starting => {
                self.password = None;
                self.state = State::Idle;
                Step::Finished
            }
            Response::Success if self.state == State::Authenticating => {
                self.password = None;
                self.state = State::Starting;
                Step::Send(Request::StartSession {
                    cmd: self.command.clone(),
                    env: vec![],
                })
            }
            Response::AuthMessage {
                auth_message_type,
                auth_message,
            } if self.state == State::Authenticating => {
                match auth_message_type {
                    AuthMessageType::Secret => {
                        // Never feed a password to an OTP or arbitrary PAM challenge.
                        if auth_message.trim().eq_ignore_ascii_case("password:")
                            && let Some(password) = self.password.take()
                        {
                            return self.answer(password.to_string());
                        }
                        Step::Prompt {
                            text: auth_message,
                            secret: true,
                        }
                    }
                    AuthMessageType::Visible => Step::Prompt {
                        text: auth_message,
                        secret: false,
                    },
                    AuthMessageType::Info | AuthMessageType::Error => {
                        Step::Notice { text: auth_message }
                    }
                }
            }
            Response::Error { .. } => self.cancel(if self.state == State::Starting {
                "Unable to start the desktop. Please try again."
            } else {
                "Login failed. Check your username and password."
            }),
            _ => self.cancel("Unexpected login response. Please try again."),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use greetd_ipc::ErrorType;

    fn login() -> Login {
        Login::new(vec!["/session".into()])
    }
    fn secret(text: &str) -> Response {
        Response::AuthMessage {
            auth_message_type: AuthMessageType::Secret,
            auth_message: text.into(),
        }
    }
    fn error() -> Response {
        Response::Error {
            error_type: ErrorType::AuthError,
            description: "private PAM details".into(),
        }
    }

    #[test]
    fn password_login_starts_only_after_authentication() {
        let mut l = login();
        assert!(matches!(
            l.begin("alice".into(), "test-password".into()),
            Step::Send(Request::CreateSession { .. })
        ));
        assert!(
            matches!(l.handle(secret("Password:")), Step::Send(Request::PostAuthMessageResponse { response: Some(ref s) }) if s == "test-password")
        );
        assert!(
            matches!(l.handle(Response::Success), Step::Send(Request::StartSession { ref cmd, ref env }) if cmd == &["/session"] && env.is_empty())
        );
        assert!(matches!(l.handle(Response::Success), Step::Finished));
    }

    #[test]
    fn otp_does_not_receive_password_and_password_is_sent_only_once() {
        let mut l = login();
        l.begin("alice".into(), "test-password".into());
        assert!(matches!(
            l.handle(secret("Verification code:")),
            Step::Prompt { secret: true, .. }
        ));
        assert!(matches!(
            l.answer("123456".into()),
            Step::Send(Request::PostAuthMessageResponse { .. })
        ));
        assert!(matches!(l.handle(secret("Password:")), Step::Send(_)));
        assert!(matches!(
            l.handle(secret("Password:")),
            Step::Prompt { secret: true, .. }
        ));
    }

    #[test]
    fn failure_cancels_before_retry_and_discards_password() {
        let mut l = login();
        l.begin("alice".into(), "test-password".into());
        assert!(matches!(
            l.handle(error()),
            Step::Send(Request::CancelSession)
        ));
        assert!(l.password.is_none());
        assert!(
            matches!(l.handle(Response::Success), Step::Reset { ref message } if !message.contains("private"))
        );
        assert!(
            matches!(l.begin("bob".into(), "new-password".into()), Step::Send(Request::CreateSession { username }) if username == "bob")
        );
    }

    #[test]
    fn session_launch_failure_is_not_login_success() {
        let mut l = login();
        l.begin("alice".into(), "test-password".into());
        l.handle(Response::Success);
        assert!(matches!(
            l.handle(error()),
            Step::Send(Request::CancelSession)
        ));
        assert!(
            matches!(l.handle(Response::Success), Step::Reset { message } if message.contains("desktop"))
        );
    }

    #[test]
    fn failed_cancellation_does_not_allow_a_retry_on_unknown_state() {
        let mut l = login();
        l.begin("alice".into(), "test-password".into());
        assert!(matches!(l.cancel(""), Step::Send(Request::CancelSession)));
        assert!(matches!(
            l.handle(error()),
            Step::Send(Request::CancelSession)
        ));
        assert!(matches!(l.handle(error()), Step::Fatal));
        assert!(l.password.is_none());
    }

    #[test]
    fn cancellation_of_an_exited_pam_child_is_confirmed_before_retry() {
        let mut l = login();
        l.begin("alice".into(), "test-password".into());
        l.handle(error());
        assert!(matches!(
            l.handle(error()),
            Step::Send(Request::CancelSession)
        ));
        assert!(
            matches!(l.handle(Response::Success), Step::Reset { message } if message.contains("Login failed"))
        );
    }

    #[test]
    fn pam_info_and_visible_challenges_are_supported() {
        let mut l = login();
        l.begin("alice".into(), "test-password".into());
        assert!(matches!(
            l.handle(Response::AuthMessage {
                auth_message_type: AuthMessageType::Info,
                auth_message: "Notice".into()
            }),
            Step::Notice { .. }
        ));
        assert!(matches!(
            l.acknowledge(),
            Step::Send(Request::PostAuthMessageResponse { response: None })
        ));
        assert!(matches!(
            l.handle(Response::AuthMessage {
                auth_message_type: AuthMessageType::Visible,
                auth_message: "Account:".into()
            }),
            Step::Prompt { secret: false, .. }
        ));
    }
}
