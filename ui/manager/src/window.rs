// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The main window: what the service says goes into the model, and what
//! the model then holds goes into the controls.
//!
//! The window is one [`Manager`], which Windows calls for every event.
//! It keeps two things apart:
//!
//! - the [`State`], which is the model, the connection and the requests
//!   that are waiting. It is changed in short steps that touch no
//!   control.
//! - the [`View`], which is the controls. They are brought up to date
//!   after each step, from the state as it then is.
//!
//! A control that is being changed may call back for an event before the
//! change returns. The state is then lent out, and such an event is let
//! go: nothing it could say is news. A dialog is only ever shown while
//! nothing is lent out, because the window goes on taking events behind
//! it.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use ctlproto::{
    Call, CallList, CallsQuery, ErrorInfo, EventSelection, LineInfo, LineList, LineRef, LogTail,
    LogTailQuery, ServerFrame, ServiceInfo, Stats,
};
use model::{Column, Command, ConnectionState, Model, RowChange, filter_of};
use msg::message;
use serde_json::Value;
use win32::metrics::{self, scale};
use win32::{
    Accelerators, App, Event, FileFilter, Handler, Hwnd, Key, MenuHighlight, ModelessDialog, Reply,
    TaskButtons, TaskDialog, TaskIcon, UiReceiver, UiSender, Window, WindowOptions, channel, id,
    relaunch_elevated, task_dialog,
};

use crate::actions;
use crate::args::Args;
use crate::clock::Clock;
use crate::dialogs;
use crate::link::{Incoming, Link, Tagged};
use crate::message as messages;
use crate::requests::{Purpose, Requests, read};
use crate::settings::{self, Saved};
use crate::view::{self, View, ids};

/// The timer that ticks once a second.
const TICK: usize = 1;

/// How often the manager asks for every line again, in ticks. A line
/// that another client removed sends no event of its own.
const RESYNC_TICKS: u64 = 10;

/// The virtual key code of the Delete key.
const KEY_DELETE: u16 = 0x2E;

/// The Windows error for "the user said no".
const ERROR_CANCELLED: i32 = 1223;

/// The command the window sends itself when it has opened with a service
/// command on its command line. Below the ids of the commands, above
/// those of the controls.
const ID_START_ACTION: u16 = 990;

/// How long the window sleeps between looks while it waits for an answer
/// with a dialog up.
const WAIT_STEP: Duration = Duration::from_millis(5);

/// Told that a dialog is up: its name and its window.
pub type DialogHook = Rc<dyn Fn(&str, Hwnd)>;

/// Answers a task dialog: given what would be shown, the id of a button.
pub type TaskHook = Rc<dyn Fn(&TaskDialog) -> u16>;

/// Answers a file or folder dialog: given its title, the path chosen.
pub type FileHook = Rc<dyn Fn(&str) -> Option<PathBuf>>;

/// What a test puts in place of the things that would stop it: the
/// system's dialogs, which wait for a person.
#[derive(Clone, Default)]
pub struct Hooks {
    /// Told when a dialog of the manager is up, with the dialog's name
    /// and its window. For the settings the window is the first page of
    /// the property sheet, whose parent is the sheet. The test closes
    /// the dialog when it has seen enough of it.
    pub dialog: Option<DialogHook>,
    /// Stands in for a task dialog: given what would be shown, answers
    /// with the id of the button that was pressed.
    pub task: Option<TaskHook>,
    /// Stands in for the file and folder dialogs: given the title,
    /// answers with the path that was chosen.
    pub file: Option<FileHook>,
}

/// How the manager is started.
#[derive(Clone)]
pub struct Options {
    /// The name of the control pipe to connect to.
    pub pipe: String,
    /// The service command to carry out once the window is open.
    pub action: Option<Command>,
    /// The clock. The screenshot tests stop it.
    pub clock: Clock,
    /// Where under `HKEY_CURRENT_USER` what is remembered is kept, or
    /// `None` to remember nothing.
    pub settings_key: Option<String>,
    /// The size of the window in pixels at 96 DPI, in place of where it
    /// was last time.
    pub size: Option<(i32, i32)>,
    /// The name of this computer, for the title.
    pub machine: String,
    /// What a test puts in place of the system's dialogs.
    pub hooks: Hooks,
}

impl Options {
    /// The options the command line asks for.
    pub fn from_args(args: &Args) -> Options {
        Options {
            pipe: args.pipe.clone(),
            action: args.action,
            clock: Clock::system(),
            settings_key: Some(settings::KEY.to_string()),
            size: None,
            machine: std::env::var("COMPUTERNAME").unwrap_or_else(|_| "localhost".to_string()),
            hooks: Hooks::default(),
        }
    }
}

/// Something to show the user in a dialog.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Prompt {
    /// A message.
    Error(ErrorInfo),
    /// The pipe does not admit this user.
    Denied(ErrorInfo),
}

/// What a step changed, for the controls to show.
struct Update {
    rows: RowChange,
    log: bool,
    stats: bool,
    /// A second went by: durations and the clock moved.
    tick: bool,
    prompts: Vec<Prompt>,
    /// The answers something is waiting for, by its number.
    waited: Vec<(u32, Result<Value, ErrorInfo>)>,
}

impl Default for Update {
    fn default() -> Self {
        Update {
            rows: RowChange::None,
            log: false,
            stats: false,
            tick: false,
            prompts: Vec::new(),
            waited: Vec::new(),
        }
    }
}

impl Update {
    /// Adds a change to the rows to the ones so far.
    fn rows(&mut self, change: RowChange) {
        self.rows = match (self.rows, change) {
            (RowChange::None, other) | (other, RowChange::None) => other,
            // More than one row was touched: draw them all.
            _ => RowChange::Reset,
        };
    }
}

fn lost(reason: &str) -> ErrorInfo {
    ErrorInfo::from(message("RMD2009", &[reason]))
}

/// The model, the connection and the requests that are waiting.
struct State {
    model: Model,
    requests: Requests,
    link: Option<Link>,
    /// The number of the link: what an earlier link still says is not
    /// taken in.
    epoch: u64,
    pipe: String,
    clock: Clock,
    sender: Option<UiSender<Tagged>>,
    /// What the status bar says in place of "Ready": something the
    /// manager asked for by itself failed.
    notice: Option<String>,
    /// The user has been told that the pipe does not admit them, and is
    /// not told again at every attempt.
    denied_shown: bool,
    ticks: u64,
}

impl State {
    fn new(options: &Options) -> State {
        State {
            model: Model::new(),
            requests: Requests::new(),
            link: None,
            epoch: 0,
            pipe: options.pipe.clone(),
            clock: options.clock,
            sender: None,
            notice: None,
            denied_shown: false,
            ticks: 0,
        }
    }

    /// Forgets the requests that are waiting: their connection is gone.
    /// What waits for one of them is told so.
    fn drop_requests(&mut self, update: &mut Update, reason: &ErrorInfo) {
        for purpose in self.requests.abandon() {
            if let Purpose::Waited(token) = purpose {
                update.waited.push((token, Err(reason.clone())));
            }
        }
    }

    /// Starts connecting to the pipe, in place of any connection there
    /// is.
    fn connect(&mut self, update: &mut Update) {
        let Some(sender) = self.sender.clone() else {
            return;
        };
        self.epoch += 1;
        self.drop_requests(update, &lost("the manager is connecting again"));
        self.denied_shown = false;
        self.notice = None;
        self.model.disconnected(None);
        self.model.connecting();
        // The link that was there stops when it is dropped.
        self.link = Some(Link::start(&self.pipe, self.epoch, sender));
    }

    /// Closes the connection and stops trying to connect.
    fn disconnect(&mut self, update: &mut Update) {
        self.link = None;
        self.epoch += 1;
        self.drop_requests(update, &lost("the manager disconnected"));
        self.notice = None;
        self.model.disconnected(None);
    }

    /// Sends a request. Its answer comes back as a message from the link.
    fn request(&mut self, call: Call, purpose: Purpose) {
        if let Some(link) = &self.link {
            link.send(self.requests.begin(&call, purpose, Instant::now()));
        }
    }

    /// Asks for everything again.
    fn refresh(&mut self) {
        self.request(Call::ServiceInfo, Purpose::Service);
        self.request(Call::LinesList, Purpose::Lines);
        self.request(Call::StatsGet, Purpose::Stats);
        self.request(Call::CallsList(CallsQuery { last: None }), Purpose::Calls);
    }

    /// Sends a command for the selected line.
    fn line_command(&mut self, command: Command) {
        let Some(id) = self.model.lines.selected().map(|line| line.id) else {
            return;
        };
        let line = LineRef { id };
        let call = match command {
            Command::Answer => Call::LinesAnswer(line),
            Command::HangUp => Call::LinesHangup(line),
            Command::Reset => Call::LinesReset(line),
            Command::Enable => Call::LinesEnable(line),
            Command::Disable => Call::LinesDisable(line),
            _ => return,
        };
        self.request(call, Purpose::Line(command));
    }

    /// Takes in what the link says.
    fn take(&mut self, (epoch, incoming): Tagged, update: &mut Update) {
        if epoch != self.epoch {
            return;
        }
        match incoming {
            Incoming::Connecting => {
                // While the reason for the last failure is shown, it
                // stays: the link tries again every few seconds.
                if matches!(self.model.connection(), ConnectionState::Disconnected(None)) {
                    self.model.connecting();
                }
            }
            Incoming::Connected => {
                self.drop_requests(update, &lost("the connection was made again"));
                self.request(Call::ServiceInfo, Purpose::Service);
            }
            Incoming::Failed { error, denied } => {
                self.drop_requests(update, &error);
                if denied && !self.denied_shown {
                    self.denied_shown = true;
                    update.prompts.push(Prompt::Denied(error.clone()));
                }
                self.model.disconnected(Some(error));
            }
            Incoming::Lost(error) => {
                self.drop_requests(update, &error);
                // A reason that is already there says more than that the
                // connection then closed.
                if !matches!(
                    self.model.connection(),
                    ConnectionState::Disconnected(Some(_))
                ) {
                    self.model.disconnected(Some(error));
                }
            }
            Incoming::Frame(frame) => match *frame {
                ServerFrame::Event(event) => {
                    let changes = self.model.event(event);
                    update.rows(changes.rows);
                    update.log |= changes.log;
                }
                ServerFrame::Result { id, result } => {
                    if let Some(purpose) = self.requests.answered(id) {
                        self.answer(purpose, Ok(result), update);
                    }
                }
                ServerFrame::Error {
                    id: Some(id),
                    error,
                } => {
                    if let Some(purpose) = self.requests.answered(id) {
                        self.answer(purpose, Err(error), update);
                    }
                }
                // An error about no request: the service could not read
                // something the manager sent.
                ServerFrame::Error { id: None, error } => self.notice = Some(error.to_string()),
                ServerFrame::Hello { .. } => {}
            },
        }
    }

    /// Takes in the answer to a request, or its failure.
    fn answer(&mut self, purpose: Purpose, result: Result<Value, ErrorInfo>, update: &mut Update) {
        if let Purpose::Waited(token) = purpose {
            // What waits for it reads it.
            update.waited.push((token, result));
            return;
        }
        let outcome = result.and_then(|value| self.accept(purpose, value, update));
        match outcome {
            Ok(()) => {
                if !purpose.asked_for() {
                    self.notice = None;
                }
            }
            Err(error) if purpose.asked_for() => update.prompts.push(Prompt::Error(error)),
            Err(error) => {
                if purpose == Purpose::Service && !self.model.connected() {
                    // The service said hello and then not who it is.
                    // The link closes, says so, and tries again.
                    self.model.disconnected(Some(error));
                    if let Some(link) = &self.link {
                        link.hang_up();
                    }
                } else {
                    self.notice = Some(error.to_string());
                }
            }
        }
    }

    /// Takes in a result.
    fn accept(
        &mut self,
        purpose: Purpose,
        value: Value,
        update: &mut Update,
    ) -> Result<(), ErrorInfo> {
        match purpose {
            Purpose::Service => {
                let info: ServiceInfo = read(value)?;
                if self.model.connected() {
                    self.model.service_info(info);
                } else {
                    self.model.connected_to(info);
                    self.denied_shown = false;
                    // The events first: what happens between the lists
                    // and the subscription would otherwise be missed.
                    self.request(
                        Call::Subscribe(EventSelection { events: None }),
                        Purpose::Subscribe,
                    );
                    self.request(Call::LinesList, Purpose::Lines);
                    self.request(Call::StatsGet, Purpose::Stats);
                    self.request(Call::LogTail(LogTailQuery { last: None }), Purpose::LogTail);
                    self.request(Call::CallsList(CallsQuery { last: None }), Purpose::Calls);
                    update.log = true;
                }
                update.stats = true;
            }
            Purpose::Subscribe | Purpose::Waited(_) => {}
            Purpose::Lines => {
                let list: LineList = read(value)?;
                update.rows(self.model.lines_listed(list.lines));
            }
            Purpose::Stats => {
                let stats: Stats = read(value)?;
                self.model.stats_received(stats);
                update.stats = true;
            }
            Purpose::LogTail => {
                let tail: LogTail = read(value)?;
                self.model.log_tailed(tail.lines);
                update.log = true;
            }
            Purpose::Calls => {
                let list: CallList = read(value)?;
                self.model.calls_listed(list.calls);
            }
            Purpose::Line(_) => {
                let line: LineInfo = read(value)?;
                update.rows(self.model.line_updated(line));
            }
            Purpose::Remove(id) => update.rows(self.model.line_removed(id)),
            Purpose::Reload => {
                // The lines may be others now, and the service says when
                // it loaded its configuration.
                self.request(Call::LinesList, Purpose::Lines);
                self.request(Call::ServiceInfo, Purpose::Service);
            }
        }
        Ok(())
    }

    /// A second went by.
    fn tick(&mut self, update: &mut Update) {
        self.ticks += 1;
        update.tick = true;
        update.stats = true;
        for (purpose, error) in self.requests.expire(Instant::now()) {
            self.answer(purpose, Err(error), update);
        }
        if !self.model.connected() {
            return;
        }
        // One at a time: a service that is slow to answer is not asked
        // again until it has.
        if !self.requests.waiting_for(Purpose::Stats) {
            self.request(Call::StatsGet, Purpose::Stats);
        }
        if self.ticks.is_multiple_of(RESYNC_TICKS) && !self.requests.waiting_for(Purpose::Lines) {
            self.request(Call::LinesList, Purpose::Lines);
        }
    }
}

/// The main window.
pub struct Manager {
    app: App,
    /// This manager, for the dialogs it opens to keep.
    me: Weak<Manager>,
    options: Options,
    state: RefCell<State>,
    view: RefCell<Option<View>>,
    window: Cell<Option<Hwnd>>,
    receiver: RefCell<Option<UiReceiver<Tagged>>>,
    /// What is waiting to be shown in a dialog.
    prompts: RefCell<VecDeque<Prompt>>,
    /// A dialog of the prompts is up.
    prompting: Cell<bool>,
    /// The answers that have come for what is waiting for them.
    waited: RefCell<HashMap<u32, Result<Value, ErrorInfo>>>,
    /// The number of the last thing that waited.
    tokens: Cell<u32>,
    /// The AT console, while it is open.
    console: RefCell<Option<ModelessDialog>>,
}

/// The main window, once it is open.
pub struct Opened {
    /// The window.
    pub window: Hwnd,
    /// What takes its events.
    pub manager: Rc<Manager>,
    /// The keys that send commands to it.
    pub accelerators: Accelerators,
}

/// Opens the main window and starts connecting. The window is made at
/// the size and place it had last time, and shown.
pub fn open(app: App, options: Options) -> Option<Opened> {
    let saved = settings::load(options.settings_key.as_deref());
    let dpi = metrics::system_dpi();
    let (width, height) = options.size.unwrap_or(view::DEFAULT_SIZE);
    let fixed_size = options.size.is_some();
    let start_action = options.action.is_some();
    let manager = Rc::new_cyclic(|me| Manager {
        app,
        me: me.clone(),
        state: RefCell::new(State::new(&options)),
        options,
        view: RefCell::new(None),
        window: Cell::new(None),
        receiver: RefCell::new(None),
        prompts: RefCell::new(VecDeque::new()),
        prompting: Cell::new(false),
        waited: RefCell::new(HashMap::new()),
        tokens: Cell::new(0),
        console: RefCell::new(None),
    });
    let window = Window::create(
        WindowOptions {
            title: &Model::new().title(&manager.options.machine),
            size: Some((scale(width, dpi), scale(height, dpi))),
            menu: Some(view::menu_bar()),
            owner: None,
            dialog_keys: true,
        },
        manager.clone(),
    )?;
    // A placement shows the window as it says; without one the window is
    // shown as it was made.
    if fixed_size || !window.set_placement(&saved.placement) {
        window.set_visible(true);
    }
    if start_action {
        // Once the window is up and the loop is running.
        window.command(ID_START_ACTION);
    }
    let accelerators = Accelerators::new(&[
        (Key::Ctrl('R'), Command::ReloadConfig.id()),
        (Key::Ctrl('N'), Command::NewLine.id()),
        (Key::Function(5), Command::Refresh.id()),
    ]);
    Some(Opened {
        window,
        manager,
        accelerators,
    })
}

/// What carries a command out.
type Action = fn(&Rc<Manager>, Hwnd, Command);

/// What carries a command out, or `None` for a command a later phase
/// brings. Every command is listed: one that is added and not given its
/// place here does not build.
fn action(command: Command) -> Option<Action> {
    let action: Action = match command {
        Command::Connect => |manager, window, _| manager.connect_dialog(window),
        Command::Disconnect => |manager, window, _| {
            manager.change(window, |state, update| state.disconnect(update));
        },
        Command::ReloadConfig => |manager, window, _| {
            manager.change(window, |state, _| {
                state.request(Call::ConfigReload, Purpose::Reload);
            });
        },
        Command::ExportStatistics => |manager, window, _| {
            actions::export_statistics(manager, window);
        },
        Command::ExportLog => |manager, window, _| actions::export_log(manager, window),
        Command::ServiceStart
        | Command::ServiceStop
        | Command::ServiceRestart
        | Command::ServiceInstall
        | Command::ServiceUninstall => |manager, window, command| {
            actions::service_command(manager, window, command, false);
        },
        Command::Exit => |_, window, _| window.close(),

        Command::Answer | Command::HangUp | Command::Reset | Command::Enable | Command::Disable => {
            |manager, window, command| {
                manager.change(window, |state, _| state.line_command(command));
            }
        }
        Command::NewLine => |manager, window, _| dialogs::line::new_line(manager, window),
        Command::EditLine => |manager, window, _| {
            if let Some(line) = manager.selected_line() {
                dialogs::line::edit_line(manager, window, line);
            }
        },
        Command::RemoveLine => |manager, window, _| manager.remove_line(window),

        Command::AtConsole => |manager, window, _| manager.open_console(window),

        Command::ShowAll | Command::ShowOnCall | Command::ShowIdle | Command::ShowNotReady => {
            |manager, window, command| {
                if let Some(filter) = filter_of(command) {
                    manager.change(window, |state, update| {
                        update.rows(state.model.lines.set_filter(filter));
                    });
                }
            }
        }
        Command::CallHistory => |manager, window, _| dialogs::history::show(manager, window),
        Command::ViewToolbar => |manager, window, _| {
            manager.toggle(window, |view| view.set_toolbar_shown(!view.toolbar_shown()));
        },
        Command::ViewStatusBar => |manager, window, _| {
            manager.toggle(window, |view| view.set_status_shown(!view.status_shown()));
        },
        Command::Refresh => |manager, window, _| {
            manager.change(window, |state, _| state.refresh());
        },

        Command::Settings => |manager, window, _| {
            if let Some(settings) = dialogs::sheet::show(manager, window) {
                actions::offer_restart(manager, window, &settings);
            }
        },
        Command::PortsAndDrivers => |manager, window, _| dialogs::ports::show(manager, window),
        Command::About => |manager, window, _| dialogs::about::show(manager, window),

        // What later phases bring. The model never lets these be chosen,
        // and says why in the status bar.
        // BLOCKED(phase2): everything under the SIP menu, and call
        // capture, come with telephony.
        Command::Reregister
        | Command::TrunkPeers
        | Command::HuntGroups
        | Command::Routes
        | Command::SipTrace
        | Command::CallCapture => return None,
        // BLOCKED(phase3): the rate cap, line diagnostics and the
        // loopback self-test come with the first modulation.
        Command::RateCap | Command::LineDiagnostics | Command::LoopbackTest => return None,
    };
    Some(action)
}

/// Whether the manager has something that carries a command out. A
/// command without is one a later phase brings: the model never lets it
/// be chosen.
pub fn has_handler(command: Command) -> bool {
    action(command).is_some()
}

impl Manager {
    /// Reads the model. For the dialogs, and for tests, which look at
    /// what the window knows.
    pub fn with_model<R>(&self, read: impl FnOnce(&Model) -> R) -> R {
        read(&self.state.borrow().model)
    }

    /// How many requests are waiting for their answers.
    pub fn requests_waiting(&self) -> usize {
        self.state.borrow().requests.len()
    }

    /// The name of the pipe the manager connects to.
    pub fn pipe(&self) -> String {
        self.state.borrow().pipe.clone()
    }

    /// The window of the AT console, while it is open.
    pub fn console_window(&self) -> Option<Hwnd> {
        self.console
            .borrow()
            .as_ref()
            .map(ModelessDialog::hwnd)
            .filter(|window| window.exists())
    }

    pub(crate) fn hooks(&self) -> &Hooks {
        &self.options.hooks
    }

    pub(crate) fn clock(&self) -> Clock {
        self.options.clock
    }

    // ----------------------------------------------------------------------
    // What the dialogs use
    // ----------------------------------------------------------------------

    /// Shows a task dialog and returns the id of the button pressed.
    pub(crate) fn task(&self, owner: Option<Hwnd>, dialog: &TaskDialog) -> u16 {
        match &self.options.hooks.task {
            Some(hook) => hook(dialog),
            None => task_dialog(owner, dialog),
        }
    }

    /// Shows an `RMD` message over a window and waits until the user has
    /// closed it.
    pub(crate) fn show_error(&self, owner: Hwnd, error: &ErrorInfo) {
        self.task(Some(owner), &messages::dialog(error));
    }

    /// Asks where to save a file.
    pub(crate) fn save_path(
        &self,
        owner: Hwnd,
        title: &str,
        file_name: &str,
        extension: &str,
        filters: &[FileFilter],
    ) -> Option<PathBuf> {
        match &self.options.hooks.file {
            Some(hook) => hook(title),
            None => win32::save_file(Some(owner), title, file_name, extension, filters),
        }
    }

    /// Asks for a folder.
    pub(crate) fn pick_folder(&self, owner: Hwnd, title: &str) -> Option<PathBuf> {
        match &self.options.hooks.file {
            Some(hook) => hook(title),
            None => win32::pick_folder(Some(owner), title),
        }
    }

    /// Runs the window's events until `done` has something to give, with
    /// `owner` taking no input in the meantime. Returns `None` if the
    /// program was told to end first.
    fn wait<T>(&self, owner: Hwnd, mut done: impl FnMut() -> Option<T>) -> Option<T> {
        let focus = win32::focused();
        let was_enabled = owner.enabled();
        owner.set_enabled(false);
        let result = loop {
            if let Some(result) = done() {
                break Some(result);
            }
            let running = match self.window.get() {
                Some(main) => self.app.pump(main, None),
                None => false,
            };
            if !running {
                // The loop that is waiting for the program to end is the
                // one outside this one: tell it again.
                App::quit(0);
                break None;
            }
            std::thread::sleep(WAIT_STEP);
        };
        if was_enabled {
            owner.set_enabled(true);
            // A window that took no input has lost the focus.
            if let Some(focus) = focus.filter(|focus| focus.exists()) {
                focus.focus();
            }
        }
        result
    }

    /// Asks the service something and waits for the answer, with `owner`
    /// taking no input in the meantime. The window goes on taking what
    /// the service says. A request with no answer fails as any other
    /// does, after ten seconds.
    pub(crate) fn call<T: serde::de::DeserializeOwned>(
        &self,
        owner: Hwnd,
        call: Call,
    ) -> Result<T, ErrorInfo> {
        let token = self.tokens.get() + 1;
        self.tokens.set(token);
        {
            let Ok(mut state) = self.state.try_borrow_mut() else {
                return Err(lost("the manager is busy"));
            };
            if !state.model.connected() {
                return Err(lost("the manager is not connected"));
            }
            state.request(call, Purpose::Waited(token));
        }
        let answer = self.wait(owner, || self.waited.borrow_mut().remove(&token));
        answer
            .unwrap_or_else(|| Err(lost("the manager is closing")))
            .and_then(read)
    }

    /// Does something that takes a while on a thread of its own and
    /// waits for it, with `owner` taking no input in the meantime.
    /// Returns `None` if it did not finish.
    pub(crate) fn wait_for<T: Send + 'static>(
        &self,
        owner: Hwnd,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> Option<T> {
        let mut thread = Some(std::thread::spawn(work));
        self.wait(owner, || {
            if thread.as_ref().is_some_and(|thread| thread.is_finished()) {
                // A thread that panicked gives nothing.
                thread.take().map(|thread| thread.join().ok())
            } else {
                None
            }
        })
        .flatten()
    }

    /// The service answered a dialog with a line as it is now.
    pub(crate) fn line_changed(&self, line: LineInfo) {
        if let Some(window) = self.window.get() {
            self.change(window, |state, update| {
                update.rows(state.model.line_updated(line));
            });
        }
    }

    /// Starts the manager again with administrator rights, to carry out
    /// `action` if there is one, and closes this one once the other has
    /// started.
    pub(crate) fn restart_elevated(&self, window: Hwnd, action: Option<Command>) {
        let arguments = Args {
            pipe: self.pipe(),
            action,
        }
        .arguments();
        match relaunch_elevated(Some(window), &arguments) {
            Ok(()) => window.close(),
            // The user was asked and said no.
            Err(e) if e.raw_os_error() == Some(ERROR_CANCELLED) => {}
            Err(e) => {
                let error = ErrorInfo::from(message("RMD4004", &[&e.to_string()]));
                self.show_error(window, &error);
            }
        }
    }

    // ----------------------------------------------------------------------
    // The window
    // ----------------------------------------------------------------------

    fn create(&self, window: Hwnd) {
        self.window.set(Some(window));
        let saved = settings::load(self.options.settings_key.as_deref());
        let view = View::create(window, &saved);
        {
            let mut state = self.state.borrow_mut();
            state.model.lines.set_filter(saved.filter);
            state.model.lines.set_sort(saved.sort);
            let (sender, receiver) = channel::<Tagged>(window);
            *self.receiver.borrow_mut() = Some(receiver);
            state.sender = Some(sender);
            state.connect(&mut Update::default());
        }
        if !win32::is_elevated()
            && let Some(menu) = win32::Menu::of_window(window)
        {
            for command in Command::ALL {
                if command.needs_elevation() {
                    menu.set_shield(command.id());
                }
            }
        }
        *self.view.borrow_mut() = Some(view);
        window.set_timer(TICK, 1000);
        let update = Update {
            log: true,
            stats: true,
            ..Update::default()
        };
        if let Some(view) = self.view.borrow().as_ref() {
            view.show_sort(&self.state.borrow().model);
        }
        self.apply(window, update);
    }

    /// Remembers how the window is, for the next run.
    fn save(&self, window: Hwnd) {
        let Some(key) = self.options.settings_key.as_deref() else {
            return;
        };
        let (Ok(state), Ok(view)) = (self.state.try_borrow(), self.view.try_borrow()) else {
            return;
        };
        let Some(view) = view.as_ref() else {
            return;
        };
        let saved = Saved {
            widths: view.column_widths(),
            sort: state.model.lines.sort(),
            filter: state.model.lines.filter(),
            toolbar: view.toolbar_shown(),
            status_bar: view.status_shown(),
            // A window given its size keeps where it was last time for
            // the run that is not.
            placement: if self.options.size.is_some() {
                Vec::new()
            } else {
                window.placement()
            },
        };
        settings::save(Some(key), &saved);
    }

    /// Changes the state in one step, and shows what the step changed.
    fn change(&self, window: Hwnd, step: impl FnOnce(&mut State, &mut Update)) {
        let mut update = Update::default();
        {
            let Ok(mut state) = self.state.try_borrow_mut() else {
                return;
            };
            step(&mut state, &mut update);
        }
        self.apply(window, update);
    }

    /// Brings the controls up to date, and then shows what there is to
    /// show in a dialog.
    fn apply(&self, window: Hwnd, update: Update) {
        if !update.waited.is_empty() {
            self.waited.borrow_mut().extend(update.waited);
        }
        if let (Ok(state), Ok(view)) = (self.state.try_borrow(), self.view.try_borrow())
            && let Some(view) = view.as_ref()
        {
            view.show_rows(&state.model, update.rows);
            if update.tick {
                view.show_ticking(&state.model);
            }
            if update.log {
                view.show_log(&state.model, &state.clock);
            }
            if update.stats {
                view.show_stats(&state.model, &state.clock);
            }
            view.show_titles(window, &state.model, &self.options.machine);
            view.show_status(&state.model, &state.clock, state.notice.as_deref());
            view.show_commands(window, &state.model);
        }
        if update.prompts.is_empty() {
            return;
        }
        {
            let mut prompts = self.prompts.borrow_mut();
            for prompt in update.prompts {
                // The same thing is said once.
                if !prompts.contains(&prompt) {
                    prompts.push_back(prompt);
                }
            }
        }
        self.show_prompts(window);
    }

    /// Shows the dialogs that are waiting, one after the other. The
    /// window takes events behind each: what they add to the queue is
    /// shown by the loop that is already running.
    fn show_prompts(&self, window: Hwnd) {
        if self.prompting.replace(true) {
            return;
        }
        loop {
            let next = self.prompts.borrow_mut().pop_front();
            match next {
                None => break,
                Some(Prompt::Error(error)) => self.show_error(window, &error),
                Some(Prompt::Denied(error)) => {
                    let dialog = messages::denied_dialog(&error);
                    if self.task(Some(window), &dialog) == messages::ID_ELEVATE {
                        self.restart_elevated(window, None);
                    }
                }
            }
        }
        self.prompting.set(false);
    }

    /// Takes in everything the link has said.
    fn wake(&self, window: Hwnd) {
        let items = match self.receiver.borrow().as_ref() {
            Some(receiver) => receiver.drain(),
            None => return,
        };
        if items.is_empty() {
            return;
        }
        self.change(window, |state, update| {
            for item in items {
                state.take(item, update);
            }
        });
    }

    /// Carries out a command, if the model says it can be chosen now.
    fn run(&self, window: Hwnd, command: Command) {
        let Ok(available) = self
            .state
            .try_borrow()
            .map(|state| state.model.availability(command).is_enabled())
        else {
            return;
        };
        if !available {
            return;
        }
        if let (Some(action), Some(manager)) = (action(command), self.me.upgrade()) {
            action(&manager, window, command);
        }
    }

    /// The selected line, as the model has it now.
    fn selected_line(&self) -> Option<LineInfo> {
        self.state
            .try_borrow()
            .ok()?
            .model
            .lines
            .selected()
            .cloned()
    }

    /// Connect to service: asks for the pipe and connects to it.
    fn connect_dialog(self: &Rc<Self>, window: Hwnd) {
        let pipe = self.pipe();
        if let Some(pipe) = dialogs::connect::show(self, window, &pipe) {
            self.change(window, |state, update| {
                state.pipe = pipe;
                state.connect(update);
            });
        }
    }

    /// Remove line: asks first, naming the line.
    fn remove_line(&self, window: Hwnd) {
        let Some(line) = self.selected_line() else {
            return;
        };
        let id = line.id;
        let named = match &line.name {
            Some(name) => format!("line {id} ({name})"),
            None => format!("line {id}"),
        };
        let dialog = TaskDialog {
            title: "Remove line".to_string(),
            instruction: format!("Remove {named}?"),
            content: format!(
                "The line on {} is taken out of the service and out of its configuration file, \
                 and its stored profile is deleted.",
                line.port
            ),
            expanded: String::new(),
            icon: TaskIcon::Warning,
            buttons: TaskButtons::YES_NO,
            links: Vec::new(),
            default: Some(id::NO),
        };
        if self.task(Some(window), &dialog) == id::YES {
            self.change(window, |state, _| {
                state.request(Call::LinesRemove(LineRef { id }), Purpose::Remove(id));
            });
        }
    }

    /// AT console: opens it, or brings the one that is open to the
    /// front.
    fn open_console(self: &Rc<Self>, window: Hwnd) {
        if let Some(console) = self.console_window() {
            console.bring_to_front();
            return;
        }
        // The one that was closed goes before the new one is made.
        let closed = self.console.borrow_mut().take();
        drop(closed);
        let console = dialogs::console::open(self, window);
        *self.console.borrow_mut() = console;
    }

    /// Shows or hides a bar and places everything again.
    fn toggle(&self, window: Hwnd, change: impl FnOnce(&View)) {
        if let Ok(view) = self.view.try_borrow()
            && let Some(view) = view.as_ref()
        {
            change(view);
            view.layout(window);
            window.redraw();
        }
        self.apply(window, Update::default());
    }

    /// What the status bar says about a command: what it does, or why it
    /// cannot be chosen.
    fn help(&self, id: u16) -> Option<String> {
        let command = Command::from_id(id)?;
        let state = self.state.try_borrow().ok()?;
        Some(state.model.help(command))
    }

    /// The grid's context menu.
    fn context_menu(&self, window: Hwnd, source: Hwnd, position: Option<(i32, i32)>) -> Reply {
        let (popup, position) = {
            let (Ok(state), Ok(view)) = (self.state.try_borrow(), self.view.try_borrow()) else {
                return Reply::Default;
            };
            let Some(view) = view.as_ref() else {
                return Reply::Default;
            };
            if view.list().hwnd() != source || !state.model.connected() {
                return Reply::Default;
            }
            // Asked for from the keyboard, the menu opens under the
            // selected row.
            let position = position.or_else(|| {
                let row = state.model.lines.selected_row()?;
                let rect = view.list().row_screen_rect(row)?;
                Some((rect.x + rect.height, rect.y + rect.height))
            });
            (view::lines_popup(&state.model), position)
        };
        // Nothing is lent out while the menu is up.
        popup.show_popup(window, position);
        Reply::Handled
    }
}

impl Handler for Manager {
    fn event(&self, window: Hwnd, event: Event<'_>) -> Reply {
        match event {
            Event::Create => {
                self.create(window);
                Reply::Handled
            }
            Event::Close => {
                self.save(window);
                // Not handled: the window is destroyed.
                Reply::Default
            }
            Event::Destroy => {
                window.kill_timer(TICK);
                let console = self.console.borrow_mut().take();
                drop(console);
                if let Ok(mut state) = self.state.try_borrow_mut() {
                    state.link = None;
                }
                App::quit(0);
                Reply::Handled
            }
            Event::Size { minimized, .. } => {
                if !minimized
                    && let Ok(view) = self.view.try_borrow()
                    && let Some(view) = view.as_ref()
                {
                    view.layout(window);
                }
                Reply::Handled
            }
            Event::DpiChanged { dpi } => {
                if let Ok(mut view) = self.view.try_borrow_mut()
                    && let Some(view) = view.as_mut()
                {
                    view.dpi_changed(window, dpi);
                }
                Reply::Handled
            }
            Event::MinSize(size) => {
                *size = View::min_size(window.dpi());
                Reply::Handled
            }
            Event::SettingsChanged => {
                // The colours may be others now.
                if let Ok(view) = self.view.try_borrow()
                    && let Some(view) = view.as_ref()
                {
                    view.list().redraw_visible();
                }
                Reply::Default
            }
            Event::Timer { id: TICK } => {
                self.change(window, |state, update| state.tick(update));
                Reply::Handled
            }
            Event::Wake => {
                self.wake(window);
                Reply::Handled
            }
            Event::Command {
                id: ids::FILTER,
                code: win32::code::COMBO_SELECTION,
            } => {
                let chosen = self
                    .view
                    .try_borrow()
                    .ok()
                    .and_then(|view| view.as_ref().and_then(View::chosen_filter));
                if let Some(filter) = chosen {
                    self.change(window, |state, update| {
                        update.rows(state.model.lines.set_filter(filter));
                    });
                }
                Reply::Handled
            }
            Event::Command {
                id: ID_START_ACTION,
                ..
            } => {
                if let (Some(command), Some(manager)) = (self.options.action, self.me.upgrade()) {
                    actions::service_command(&manager, window, command, true);
                }
                Reply::Handled
            }
            Event::Command { id, .. } => match Command::from_id(id) {
                Some(command) => {
                    self.run(window, command);
                    Reply::Handled
                }
                None => Reply::Default,
            },
            Event::MenuSelect(highlight) => {
                let text = match highlight {
                    MenuHighlight::Item(id) => Some(self.help(id).unwrap_or_default()),
                    MenuHighlight::Other => Some(String::new()),
                    MenuHighlight::Closed => None,
                };
                if let Ok(view) = self.view.try_borrow()
                    && let Some(view) = view.as_ref()
                {
                    view.status().set_simple(text.as_deref());
                }
                Reply::Handled
            }
            Event::ToolTip { id, text } => {
                if let Some(help) = self.help(id) {
                    *text = help;
                }
                Reply::Handled
            }
            Event::ContextMenu { source, position } => self.context_menu(window, source, position),
            Event::ListCell {
                id: ids::LIST,
                row,
                column,
                cell,
            } => {
                let Ok(state) = self.state.try_borrow() else {
                    return Reply::Default;
                };
                if let Some(shown) = Column::at(column) {
                    cell.text = state.model.lines.cell(row, shown, state.clock.now_ms());
                }
                if column == 0 {
                    cell.image = state.model.lines.look(row).map(|look| look.image());
                }
                Reply::Handled
            }
            Event::ListCellColour {
                id: ids::LIST,
                row,
                column,
                colour,
            } => {
                let Ok(state) = self.state.try_borrow() else {
                    return Reply::Default;
                };
                // A grid that takes no part is drawn in the system's
                // colour for that.
                if Column::at(column) == Some(Column::State)
                    && state.model.connected()
                    && let Some(look) = state.model.lines.look(row)
                {
                    *colour = view::state_colour(look.colour());
                }
                Reply::Handled
            }
            Event::ListColumnClick {
                id: ids::LIST,
                column,
            } => {
                if let Some(column) = Column::at(column) {
                    self.change(window, |state, update| {
                        let sort = state.model.lines.sort_after_click(column);
                        update.rows(state.model.lines.set_sort(sort));
                    });
                    if let (Ok(state), Ok(view)) = (self.state.try_borrow(), self.view.try_borrow())
                        && let Some(view) = view.as_ref()
                    {
                        view.show_sort(&state.model);
                    }
                }
                Reply::Handled
            }
            Event::ListSelection { id: ids::LIST } => {
                let selected = match self.view.try_borrow() {
                    Ok(view) => view.as_ref().map(|view| view.list().selected()),
                    Err(_) => None,
                };
                // While the grid is being brought up to date its
                // selection is the manager's own doing.
                if let Some(selected) = selected {
                    self.change(window, |state, _| {
                        state.model.lines.select_row(selected);
                    });
                }
                Reply::Handled
            }
            Event::ListActivate { id: ids::LIST, .. } => {
                self.run(window, Command::EditLine);
                Reply::Handled
            }
            Event::ListKey {
                id: ids::LIST,
                key: KEY_DELETE,
            } => {
                self.run(window, Command::RemoveLine);
                Reply::Handled
            }
            _ => Reply::Default,
        }
    }
}
