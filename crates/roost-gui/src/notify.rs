//! Telling the user that something needs them while Roost is in the
//! background: an approval waiting, a run that ended.
//!
//! The decision (`Attention`) has no egui and no operating system in it. The
//! delivery uses the system's own tools, like `speech`: `osascript` on macOS,
//! `notify-send` on Linux, PowerShell on Windows. Titles and bodies go in as
//! separate arguments, never inside a shell string. A notification never
//! carries file contents, a command line or model output: the body is one of
//! a few fixed sentences.

use roost_core::chat::{ChatEvent, RunOutcome};
use roost_core::config::Notifications;
use roost_core::session::SessionId;
use std::process::{Child, Command, Stdio};

/// Why the user is being told.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Approval,
    /// The agent asked a question.
    Question,
    /// A tool needs a working directory.
    Folder,
    Finished,
    Stopped,
}

impl Kind {
    pub fn body(self) -> &'static str {
        match self {
            Kind::Approval => "Needs your approval",
            Kind::Question => "Has a question for you",
            Kind::Folder => "Needs a folder to work in",
            Kind::Finished => "Run finished",
            Kind::Stopped => "Run stopped",
        }
    }
}

/// The events that call for a nudge. A run you cancelled yourself does not.
pub fn kind_of(ev: &ChatEvent) -> Option<(SessionId, Kind)> {
    match ev {
        ChatEvent::ApprovalRequested { session, .. } => Some((*session, Kind::Approval)),
        ChatEvent::QuestionAsked { session, .. } => Some((*session, Kind::Question)),
        ChatEvent::WorkspaceNeeded { session, .. } => Some((*session, Kind::Folder)),
        ChatEvent::RunEnded {
            session, outcome, ..
        } => match outcome {
            RunOutcome::Completed => Some((*session, Kind::Finished)),
            RunOutcome::Failed | RunOutcome::BudgetExhausted | RunOutcome::TextToolCall => {
                Some((*session, Kind::Stopped))
            }
            RunOutcome::Cancelled => None,
        },
        _ => None,
    }
}

/// What the window should do about one alert.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    Nothing,
    /// Bounce the Dock / flash the taskbar and send a system notification.
    Notify,
}

/// Alerts are aggregated between visits: while the window is in the
/// background only the first alert notifies, and the rest are only counted
/// (the window title shows the count). Coming back resets it.
#[derive(Debug, Default)]
pub struct Attention {
    pending: u32,
}

impl Attention {
    pub fn on_alert(&mut self, setting: Notifications, focused: bool) -> Delivery {
        match (setting, focused) {
            (Notifications::Off, _) => Delivery::Nothing,
            (Notifications::Background, true) => Delivery::Nothing,
            (Notifications::Always, true) => Delivery::Notify,
            (_, false) => {
                self.pending += 1;
                if self.pending == 1 {
                    Delivery::Notify
                } else {
                    Delivery::Nothing
                }
            }
        }
    }

    /// The window came to the front.
    pub fn on_focus(&mut self) {
        self.pending = 0;
    }

    #[cfg(test)]
    pub fn pending(&self) -> u32 {
        self.pending
    }

    /// The window title: the product name, with how many alerts wait.
    pub fn title(&self, name: &str) -> String {
        match self.pending {
            0 => name.to_string(),
            n => format!("({n}) {name}"),
        }
    }
}

/// What the window should do this frame.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct FrameAlerts {
    /// Bounce the Dock icon / flash the taskbar.
    pub attention: bool,
    /// A new window title, when it changed.
    pub title: Option<String>,
}

impl crate::app::AppState {
    /// Settles the alerts that arrived since the last frame, given whether the
    /// window is in front. Sends the system notification itself.
    pub fn deliver_alerts(&mut self, focused: bool) -> FrameAlerts {
        if focused {
            self.attention.on_focus();
        }
        let mut out = FrameAlerts::default();
        for (session, kind) in std::mem::take(&mut self.alerts) {
            let setting = self.settings.notifications;
            if self.attention.on_alert(setting, focused) == Delivery::Notify {
                let title = self
                    .chat
                    .title_of(session)
                    .filter(|t| !t.is_empty())
                    .unwrap_or(crate::identity::PRODUCT_NAME)
                    .to_string();
                self.notifier.notify(&title, kind.body());
                out.attention = !focused;
            }
        }
        let title = self.attention.title(crate::identity::PRODUCT_NAME);
        if title != self.window_title {
            self.window_title = title.clone();
            out.title = Some(title);
        }
        out
    }
}

/// What the window needs from a notification service (a fake in tests).
pub trait Notifier {
    fn notify(&mut self, title: &str, body: &str);
}

/// Does nothing. Used by tests and when no tool is found.
pub struct NullNotifier;

impl Notifier for NullNotifier {
    fn notify(&mut self, _title: &str, _body: &str) {}
}

const WINDOWS_SCRIPT: &str = "Add-Type -AssemblyName System.Windows.Forms; \
$t = [Console]::In.ReadLine(); $b = [Console]::In.ReadLine(); \
$n = New-Object System.Windows.Forms.NotifyIcon; \
$n.Icon = [System.Drawing.SystemIcons]::Information; $n.Visible = $true; \
$n.ShowBalloonTip(5000, $t, $b, 'Info'); Start-Sleep -Seconds 6; $n.Dispose()";

const MAC_SCRIPT: &str = "on run argv\n\
display notification (item 2 of argv) with title (item 1 of argv)\n\
end run";

/// A program, its arguments, and whether the title and body go on stdin
/// (Windows) instead of after the arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spec {
    pub program: String,
    pub args: Vec<String>,
    pub on_stdin: bool,
}

/// The command for `os` (as in `std::env::consts::OS`), given a way to ask
/// whether a program exists.
pub fn choose(os: &str, has: impl Fn(&str) -> bool) -> Option<Spec> {
    let spec = |program: &str, args: &[&str], on_stdin: bool| Spec {
        program: program.to_string(),
        args: args.iter().map(|a| a.to_string()).collect(),
        on_stdin,
    };
    match os {
        "macos" => has("osascript").then(|| spec("osascript", &["-e", MAC_SCRIPT], false)),
        "windows" => Some(spec(
            "powershell.exe",
            &["-NoProfile", "-NonInteractive", "-Command", WINDOWS_SCRIPT],
            true,
        )),
        _ => has("notify-send").then(|| spec("notify-send", &["--app-name=Roost", "--"], false)),
    }
}

/// Sends notifications with the system's own tool.
pub struct SystemNotifier {
    spec: Option<Spec>,
    children: Vec<Child>,
}

impl SystemNotifier {
    pub fn detect() -> Self {
        let has = |program: &str| {
            std::env::var_os("PATH").is_some_and(|paths| {
                std::env::split_paths(&paths).any(|dir| dir.join(program).is_file())
            })
        };
        SystemNotifier {
            spec: choose(std::env::consts::OS, has),
            children: Vec::new(),
        }
    }
}

impl Notifier for SystemNotifier {
    fn notify(&mut self, title: &str, body: &str) {
        // Reap the ones that finished so they do not pile up as zombies.
        self.children
            .retain_mut(|c| matches!(c.try_wait(), Ok(None)));
        let Some(spec) = &self.spec else {
            return;
        };
        let mut command = Command::new(&spec.program);
        command
            .args(&spec.args)
            .stdin(if spec.on_stdin {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if !spec.on_stdin {
            command.arg(title).arg(body);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // No console window flashing up.
            command.creation_flags(0x0800_0000);
        }
        let Ok(mut child) = command.spawn() else {
            return;
        };
        if spec.on_stdin {
            if let Some(mut stdin) = child.stdin.take() {
                use std::io::Write;
                // Single lines: a newline in a title would shift the body.
                let one = |s: &str| s.replace(['\r', '\n'], " ");
                let _ = writeln!(stdin, "{}\n{}", one(title), one(body));
            }
        }
        self.children.push(child);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::runtime::UiEvent;
    use roost_core::config::Settings;
    use roost_core::fit::Machine;
    use roost_core::session::SessionId;
    use std::cell::RefCell;
    use std::rc::Rc;

    type Sent = Rc<RefCell<Vec<(String, String)>>>;

    struct Fake(Sent);

    impl Notifier for Fake {
        fn notify(&mut self, title: &str, body: &str) {
            self.0.borrow_mut().push((title.into(), body.into()));
        }
    }

    fn app_with_fake() -> (App, Sent) {
        let (tx, _req) = std::sync::mpsc::channel();
        let (_, rx) = std::sync::mpsc::channel();
        let mut app = App::new(
            &Settings::default(),
            std::path::PathBuf::from("/tmp/staging"),
            Machine {
                ram_gb: 64.0,
                vram_gb: None,
            },
            tx,
            rx,
        );
        let sent = Sent::default();
        app.state_mut().notifier = Box::new(Fake(sent.clone()));
        (app, sent)
    }

    fn approval() -> UiEvent {
        UiEvent::Chat(ChatEvent::ApprovalRequested {
            session: SessionId::new(),
            call_id: "c".into(),
            name: "write_file".into(),
            arguments: serde_json::json!({}),
            preview: None,
            run: None,
        })
    }

    fn ended(outcome: RunOutcome) -> ChatEvent {
        ChatEvent::RunEnded {
            session: SessionId::new(),
            outcome,
            message: None,
        }
    }

    #[test]
    fn only_approvals_and_runs_that_ended_call_for_a_nudge() {
        let approval = ChatEvent::ApprovalRequested {
            session: SessionId::new(),
            call_id: "c".into(),
            name: "write_file".into(),
            arguments: serde_json::json!({}),
            preview: None,
            run: None,
        };
        assert_eq!(kind_of(&approval).map(|k| k.1), Some(Kind::Approval));
        let asked = ChatEvent::QuestionAsked {
            session: SessionId::new(),
            call_id: "c".into(),
            question: roost_core::agent::ask::parse(
                &serde_json::json!({"question": "Q?", "kind": "text"}),
            )
            .unwrap(),
        };
        assert_eq!(kind_of(&asked).map(|k| k.1), Some(Kind::Question));
        let folder = ChatEvent::WorkspaceNeeded {
            session: SessionId::new(),
            call_id: "c".into(),
            name: "read_file".into(),
            arguments: serde_json::json!({}),
        };
        assert_eq!(kind_of(&folder).map(|k| k.1), Some(Kind::Folder));
        assert_eq!(
            kind_of(&ended(RunOutcome::Completed)).map(|k| k.1),
            Some(Kind::Finished)
        );
        assert_eq!(
            kind_of(&ended(RunOutcome::Failed)).map(|k| k.1),
            Some(Kind::Stopped)
        );
        assert_eq!(
            kind_of(&ended(RunOutcome::BudgetExhausted)).map(|k| k.1),
            Some(Kind::Stopped)
        );
        assert_eq!(
            kind_of(&ended(RunOutcome::TextToolCall)).map(|k| k.1),
            Some(Kind::Stopped)
        );
        assert!(kind_of(&ended(RunOutcome::Cancelled)).is_none());
        assert!(kind_of(&ChatEvent::StoreWarning("x".into())).is_none());
    }

    #[test]
    fn the_setting_and_focus_decide_whether_to_notify() {
        use Notifications::{Always, Background, Off};
        let once = |setting, focused| Attention::default().on_alert(setting, focused);
        assert_eq!(once(Off, false), Delivery::Nothing);
        assert_eq!(once(Off, true), Delivery::Nothing);
        assert_eq!(once(Background, true), Delivery::Nothing);
        assert_eq!(once(Background, false), Delivery::Notify);
        assert_eq!(once(Always, true), Delivery::Notify);
        assert_eq!(once(Always, false), Delivery::Notify);
    }

    #[test]
    fn alerts_are_aggregated_until_the_window_comes_back() {
        let mut a = Attention::default();
        let bg = Notifications::Background;
        assert_eq!(a.on_alert(bg, false), Delivery::Notify);
        assert_eq!(a.on_alert(bg, false), Delivery::Nothing);
        assert_eq!(a.on_alert(bg, false), Delivery::Nothing);
        assert_eq!(a.pending(), 3);
        assert_eq!(a.title("Roost"), "(3) Roost");
        a.on_focus();
        assert_eq!(a.title("Roost"), "Roost");
        assert_eq!(a.on_alert(bg, false), Delivery::Notify, "a new visit");
    }

    #[test]
    fn off_never_counts() {
        let mut a = Attention::default();
        a.on_alert(Notifications::Off, false);
        assert_eq!(a.pending(), 0);
        assert_eq!(a.title("Roost"), "Roost");
    }

    #[test]
    fn the_command_takes_text_as_arguments_not_in_a_script() {
        let mac = choose("macos", |p| p == "osascript").expect("mac");
        assert_eq!(mac.program, "osascript");
        assert!(!mac.on_stdin);
        assert!(
            !mac.args.iter().any(|a| a.contains("Roost")),
            "the text is added by the caller as separate arguments"
        );
        assert!(choose("macos", |_| false).is_none());
        let linux = choose("linux", |p| p == "notify-send").expect("linux");
        assert_eq!(linux.args.last().map(String::as_str), Some("--"));
        assert!(choose("linux", |_| false).is_none());
        let win = choose("windows", |_| false).expect("windows");
        assert!(win.on_stdin);
    }

    #[test]
    fn the_bodies_are_fixed_sentences() {
        for k in [
            Kind::Approval,
            Kind::Question,
            Kind::Finished,
            Kind::Stopped,
        ] {
            assert!(!k.body().is_empty() && k.body().len() < 40);
        }
    }

    #[test]
    fn a_chat_event_in_the_background_notifies_once_and_counts_the_rest() {
        let (mut app, sent) = app_with_fake();
        let st = app.state_mut();
        st.on_event(&approval());
        st.on_event(&approval());
        let out = st.deliver_alerts(false);
        assert!(out.attention);
        assert_eq!(out.title.as_deref(), Some("(2) Roost"));
        assert_eq!(
            sent.borrow().as_slice(),
            [("Roost".to_string(), "Needs your approval".to_string())],
            "one notification, a fixed sentence"
        );
        // Nothing new: nothing sent, title unchanged.
        let out = st.deliver_alerts(false);
        assert_eq!(out, FrameAlerts::default());
        // Back in front: the title resets.
        let out = st.deliver_alerts(true);
        assert_eq!(out.title.as_deref(), Some("Roost"));
        assert_eq!(sent.borrow().len(), 1);
    }

    #[test]
    fn nothing_is_sent_while_the_window_is_in_front_unless_always() {
        let (mut app, sent) = app_with_fake();
        let st = app.state_mut();
        st.on_event(&approval());
        assert_eq!(st.deliver_alerts(true), FrameAlerts::default());
        st.settings.notifications = Notifications::Always;
        st.on_event(&approval());
        let out = st.deliver_alerts(true);
        assert!(!out.attention, "no Dock bounce for the window in front");
        assert_eq!(sent.borrow().len(), 1);
    }

    #[test]
    fn off_sends_nothing_even_in_the_background() {
        let (mut app, sent) = app_with_fake();
        let st = app.state_mut();
        st.settings.notifications = Notifications::Off;
        st.on_event(&approval());
        assert_eq!(st.deliver_alerts(false), FrameAlerts::default());
        assert!(sent.borrow().is_empty());
    }
}
