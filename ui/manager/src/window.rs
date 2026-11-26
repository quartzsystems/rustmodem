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
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::Instant;

use ctlproto::{
    Call, CallList, CallsQuery, ErrorInfo, EventSelection, LineInfo, LineList, LineRef, LogTail,
    LogTailQuery, ServerFrame, ServiceInfo, Stats,
};
use model::{Column, Command, ConnectionState, Model, RowChange, filter_of};
use msg::message;
use serde_json::Value;
use win32::metrics::{self, scale};
use win32::{
    Accelerators, App, Event, Handler, Hwnd, Key, MenuHighlight, Reply, UiReceiver, UiSender,
    Window, WindowOptions, channel, relaunch_elevated,
};

use crate::args::Args;
use crate::clock::Clock;
use crate::link::{Incoming, Link, Tagged};
use crate::message as dialogs;
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

/// How the manager is started.
#[derive(Clone, Debug)]
pub struct Options {
    /// The name of the control pipe to connect to.
    pub pipe: String,
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
    /// The arguments that start the manager the same way again.
    pub arguments: Vec<String>,
}

impl Options {
    /// The options the command line asks for.
    pub fn from_args(args: &Args) -> Options {
        Options {
            pipe: args.pipe.clone(),
            clock: Clock::system(),
            settings_key: Some(settings::KEY.to_string()),
            size: None,
            machine: std::env::var("COMPUTERNAME").unwrap_or_else(|_| "localhost".to_string()),
            arguments: args.arguments(),
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
}

impl Default for Update {
    fn default() -> Self {
        Update {
            rows: RowChange::None,
            log: false,
            stats: false,
            tick: false,
            prompts: Vec::new(),
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

    /// Starts connecting to the pipe, in place of any connection there
    /// is.
    fn connect(&mut self) {
        let Some(sender) = self.sender.clone() else {
            return;
        };
        self.epoch += 1;
        self.requests.clear();
        self.denied_shown = false;
        self.notice = None;
        self.model.disconnected(None);
        self.model.connecting();
        // The link that was there stops when it is dropped.
        self.link = Some(Link::start(&self.pipe, self.epoch, sender));
    }

    /// Closes the connection and stops trying to connect.
    fn disconnect(&mut self) {
        self.link = None;
        self.epoch += 1;
        self.requests.clear();
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
                self.requests.clear();
                self.request(Call::ServiceInfo, Purpose::Service);
            }
            Incoming::Failed { error, denied } => {
                self.requests.clear();
                if denied && !self.denied_shown {
                    self.denied_shown = true;
                    update.prompts.push(Prompt::Denied(error.clone()));
                }
                self.model.disconnected(Some(error));
            }
            Incoming::Lost(error) => {
                self.requests.clear();
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
            Purpose::Subscribe => {}
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
    options: Options,
    state: RefCell<State>,
    view: RefCell<Option<View>>,
    receiver: RefCell<Option<UiReceiver<Tagged>>>,
    /// What is waiting to be shown in a dialog.
    prompts: RefCell<VecDeque<Prompt>>,
    /// A dialog of the prompts is up.
    prompting: Cell<bool>,
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
pub fn open(options: Options) -> Option<Opened> {
    let saved = settings::load(options.settings_key.as_deref());
    let dpi = metrics::system_dpi();
    let (width, height) = options.size.unwrap_or(view::DEFAULT_SIZE);
    let fixed_size = options.size.is_some();
    let manager = Rc::new(Manager {
        state: RefCell::new(State::new(&options)),
        options,
        view: RefCell::new(None),
        receiver: RefCell::new(None),
        prompts: RefCell::new(VecDeque::new()),
        prompting: Cell::new(false),
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

impl Manager {
    /// Reads the model. For tests, which look at what the window knows.
    pub fn with_model<R>(&self, read: impl FnOnce(&Model) -> R) -> R {
        read(&self.state.borrow().model)
    }

    /// How many requests are waiting for their answers.
    pub fn requests_waiting(&self) -> usize {
        self.state.borrow().requests.len()
    }

    fn create(&self, window: Hwnd) {
        let saved = settings::load(self.options.settings_key.as_deref());
        let view = View::create(window, &saved);
        {
            let mut state = self.state.borrow_mut();
            state.model.lines.set_filter(saved.filter);
            state.model.lines.set_sort(saved.sort);
            let (sender, receiver) = channel::<Tagged>(window);
            *self.receiver.borrow_mut() = Some(receiver);
            state.sender = Some(sender);
            state.connect();
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
                Some(Prompt::Error(error)) => dialogs::show(Some(window), &error),
                Some(Prompt::Denied(error)) => {
                    if dialogs::show_denied(Some(window), &error) {
                        self.restart_elevated(window);
                    }
                }
            }
        }
        self.prompting.set(false);
    }

    /// Starts the manager again with administrator rights, and closes
    /// this one once the other has started.
    fn restart_elevated(&self, window: Hwnd) {
        match relaunch_elevated(Some(window), &self.options.arguments) {
            Ok(()) => window.close(),
            // The user was asked and said no.
            Err(e) if e.raw_os_error() == Some(ERROR_CANCELLED) => {}
            Err(e) => {
                let error = ErrorInfo::from(message("RMD4004", &[&e.to_string()]));
                self.prompts.borrow_mut().push_back(Prompt::Error(error));
            }
        }
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
        match command {
            Command::Exit => window.close(),
            Command::Disconnect => self.change(window, |state, _| state.disconnect()),
            Command::ReloadConfig => self.change(window, |state, _| {
                state.request(Call::ConfigReload, Purpose::Reload);
            }),
            Command::Answer
            | Command::HangUp
            | Command::Reset
            | Command::Enable
            | Command::Disable => self.change(window, |state, _| state.line_command(command)),
            Command::ShowAll | Command::ShowOnCall | Command::ShowIdle | Command::ShowNotReady => {
                if let Some(filter) = filter_of(command) {
                    self.change(window, |state, update| {
                        update.rows(state.model.lines.set_filter(filter));
                    });
                }
            }
            Command::ViewToolbar => self.toggle(window, |view| {
                view.set_toolbar_shown(!view.toolbar_shown());
            }),
            Command::ViewStatusBar => self.toggle(window, |view| {
                view.set_status_shown(!view.status_shown());
            }),
            Command::Refresh => self.change(window, |state, _| state.refresh()),

            // The dialogs, which the next work package brings.
            Command::Connect
            | Command::ExportStatistics
            | Command::ExportLog
            | Command::ServiceStart
            | Command::ServiceStop
            | Command::ServiceRestart
            | Command::ServiceInstall
            | Command::ServiceUninstall
            | Command::NewLine
            | Command::EditLine
            | Command::RemoveLine
            | Command::AtConsole
            | Command::CallHistory
            | Command::Settings
            | Command::PortsAndDrivers
            | Command::About => {}

            // What later phases bring. The model never lets these be
            // chosen, and says why in the status bar.
            // BLOCKED(phase2): everything under the SIP menu, and call
            // capture.
            Command::Reregister
            | Command::TrunkPeers
            | Command::HuntGroups
            | Command::Routes
            | Command::SipTrace
            | Command::CallCapture => {}
            // BLOCKED(phase3): the rate cap, line diagnostics and the
            // loopback self-test.
            Command::RateCap | Command::LineDiagnostics | Command::LoopbackTest => {}
        }
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
