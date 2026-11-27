// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The commands of the manager: every menu item and toolbar button.
//!
//! The menus and the toolbar are built from what is here, so the window
//! and its tests cannot disagree about what there is. Whether a command
//! can be chosen now, and if not why not, is the model's to say
//! ([`crate::Model::availability`]); the window asks and never decides
//! for itself.

/// A command.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Command {
    // File
    /// Connect to service…
    Connect,
    /// Disconnect
    Disconnect,
    /// Reload configuration
    ReloadConfig,
    /// Export statistics…
    ExportStatistics,
    /// Export event log…
    ExportLog,
    /// Start service
    ServiceStart,
    /// Stop service
    ServiceStop,
    /// Restart service
    ServiceRestart,
    /// Install service…
    ServiceInstall,
    /// Uninstall service
    ServiceUninstall,
    /// Exit
    Exit,
    // Lines
    /// Answer now
    Answer,
    /// Hang up
    HangUp,
    /// Reset
    Reset,
    /// Enable
    Enable,
    /// Disable
    Disable,
    /// New line…
    NewLine,
    /// Edit line…
    EditLine,
    /// Remove line…
    RemoveLine,
    // SIP
    /// Re-register
    Reregister,
    /// Trunk peers…
    TrunkPeers,
    /// Hunt groups…
    HuntGroups,
    /// Routes…
    Routes,
    /// SIP trace
    SipTrace,
    // Modem
    /// AT console…
    AtConsole,
    /// Rate cap and modulations…
    RateCap,
    // View
    /// Show: All lines
    ShowAll,
    /// Show: On a call
    ShowOnCall,
    /// Show: Idle
    ShowIdle,
    /// Show: Not ready or disabled
    ShowNotReady,
    /// Call history…
    CallHistory,
    /// Line diagnostics…
    LineDiagnostics,
    /// Toolbar
    ViewToolbar,
    /// Status bar
    ViewStatusBar,
    /// Refresh
    Refresh,
    // Tools
    /// Settings…
    Settings,
    /// Ports and drivers…
    PortsAndDrivers,
    /// Loopback self-test…
    LoopbackTest,
    /// Call capture…
    CallCapture,
    // Help
    /// About RustModem Manager…
    About,
}

/// One entry of a menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuEntry {
    /// A command.
    Item(Command),
    /// A line between groups of commands.
    Separator,
    /// A menu inside the menu: its text and its commands.
    Submenu(&'static str, Vec<Command>),
}

/// The id of the first command, as a Win32 command id. Below it are the
/// ids the dialog manager gives meaning to and the ids of controls.
const FIRST_ID: u16 = 1000;

impl Command {
    /// Every command.
    pub const ALL: [Command; 40] = [
        Command::Connect,
        Command::Disconnect,
        Command::ReloadConfig,
        Command::ExportStatistics,
        Command::ExportLog,
        Command::ServiceStart,
        Command::ServiceStop,
        Command::ServiceRestart,
        Command::ServiceInstall,
        Command::ServiceUninstall,
        Command::Exit,
        Command::Answer,
        Command::HangUp,
        Command::Reset,
        Command::Enable,
        Command::Disable,
        Command::NewLine,
        Command::EditLine,
        Command::RemoveLine,
        Command::Reregister,
        Command::TrunkPeers,
        Command::HuntGroups,
        Command::Routes,
        Command::SipTrace,
        Command::AtConsole,
        Command::RateCap,
        Command::ShowAll,
        Command::ShowOnCall,
        Command::ShowIdle,
        Command::ShowNotReady,
        Command::CallHistory,
        Command::LineDiagnostics,
        Command::ViewToolbar,
        Command::ViewStatusBar,
        Command::Refresh,
        Command::Settings,
        Command::PortsAndDrivers,
        Command::LoopbackTest,
        Command::CallCapture,
        Command::About,
    ];

    /// The id of the command, as Win32 knows it.
    pub fn id(self) -> u16 {
        let index = Command::ALL
            .iter()
            .position(|command| *command == self)
            .unwrap_or(0);
        FIRST_ID + index as u16
    }

    /// The command with an id.
    pub fn from_id(id: u16) -> Option<Command> {
        Command::ALL
            .get(usize::from(id.checked_sub(FIRST_ID)?))
            .copied()
    }

    /// The text of the command's menu item. An ampersand marks the
    /// mnemonic, an ellipsis says a dialog follows, and what is after the
    /// tab is the accelerator.
    pub fn text(self) -> &'static str {
        match self {
            Command::Connect => "&Connect to service…",
            Command::Disconnect => "&Disconnect",
            Command::ReloadConfig => "&Reload configuration\tCtrl+R",
            Command::ExportStatistics => "Export &statistics…",
            Command::ExportLog => "Export event &log…",
            Command::ServiceStart => "S&tart service",
            Command::ServiceStop => "Sto&p service",
            Command::ServiceRestart => "R&estart service",
            Command::ServiceInstall => "&Install service…",
            Command::ServiceUninstall => "&Uninstall service",
            Command::Exit => "E&xit\tAlt+F4",
            Command::Answer => "&Answer now",
            Command::HangUp => "&Hang up",
            Command::Reset => "&Reset",
            Command::Enable => "&Enable",
            Command::Disable => "&Disable",
            Command::NewLine => "&New line…\tCtrl+N",
            Command::EditLine => "Edi&t line…\tEnter",
            Command::RemoveLine => "Re&move line…\tDel",
            Command::Reregister => "&Re-register",
            Command::TrunkPeers => "&Trunk peers…",
            Command::HuntGroups => "&Hunt groups…",
            Command::Routes => "R&outes…",
            Command::SipTrace => "&SIP trace",
            Command::AtConsole => "&AT console…",
            Command::RateCap => "&Rate cap and modulations…",
            Command::ShowAll => "&All lines",
            Command::ShowOnCall => "&On a call",
            Command::ShowIdle => "&Idle",
            Command::ShowNotReady => "&Not ready or disabled",
            Command::CallHistory => "&Call history…",
            Command::LineDiagnostics => "Line &diagnostics…",
            Command::ViewToolbar => "&Toolbar",
            Command::ViewStatusBar => "Status &bar",
            Command::Refresh => "&Refresh\tF5",
            Command::Settings => "&Settings…",
            Command::PortsAndDrivers => "&Ports and drivers…",
            Command::LoopbackTest => "&Loopback self-test…",
            Command::CallCapture => "&Call capture…",
            Command::About => "&About RustModem Manager…",
        }
    }

    /// The label of the command's toolbar button, for the commands that
    /// have one.
    pub fn tool_label(self) -> Option<&'static str> {
        match self {
            Command::Answer => Some("Answer now"),
            Command::HangUp => Some("Hang up"),
            Command::Reset => Some("Reset line"),
            Command::NewLine => Some("New line…"),
            Command::Reregister => Some("Re-register"),
            Command::Settings => Some("Settings…"),
            _ => None,
        }
    }

    /// One line that says what the command does, for the status bar while
    /// the command is highlighted and for the tip of its toolbar button.
    pub fn description(self) -> &'static str {
        match self {
            Command::Connect => "Connects to a rustmodem service by the name of its control pipe.",
            Command::Disconnect => {
                "Closes the connection to the service and stops trying to connect."
            }
            Command::ReloadConfig => {
                "Has the service read its configuration file again and make its lines match it."
            }
            Command::ExportStatistics => {
                "Writes the statistics and the connect rates to a CSV file."
            }
            Command::ExportLog => "Writes the event log to a text file.",
            Command::ServiceStart => "Starts the rustmodem Windows service.",
            Command::ServiceStop => "Stops the rustmodem Windows service. Every call is hung up.",
            Command::ServiceRestart => {
                "Stops the rustmodem Windows service and starts it again. Every call is hung up."
            }
            Command::ServiceInstall => "Registers rustmodem as a Windows service.",
            Command::ServiceUninstall => "Removes the rustmodem Windows service.",
            Command::Exit => "Closes the manager. The service goes on running.",
            Command::Answer => "Answers the call ringing on the selected line.",
            Command::HangUp => {
                "Hangs up the call on the selected line, or turns away the call ringing on it."
            }
            Command::Reset => {
                "Resets the selected line: hangs up and returns its modem to its stored profile."
            }
            Command::Enable => "Puts the selected line in service.",
            Command::Disable => {
                "Takes the selected line out of service. Any call on it is hung up."
            }
            Command::NewLine => "Adds a line to the service.",
            Command::EditLine => "Changes the selected line's name, port and settings.",
            Command::RemoveLine => "Removes the selected line from the service.",
            Command::Reregister => {
                "Registers the selected line with its PBX again, or with no line selected, \
                 every line that registers."
            }
            Command::TrunkPeers => {
                "Says which addresses may send calls to the service, and where the lines' \
                 own calls go."
            }
            Command::HuntGroups => "Arranges the lines into groups that share numbers.",
            Command::Routes => "Says which numbers ring which lines and hunt groups.",
            Command::SipTrace => "Shows the SIP messages the service sends and receives.",
            Command::AtConsole => {
                "Opens a terminal on a COM port of this computer, to talk to a line by hand."
            }
            Command::RateCap => {
                "Sets the modulations the modems offer, V.8 and the level they send at."
            }
            Command::ShowAll => "Shows every line.",
            Command::ShowOnCall => "Shows the lines that are ringing, handshaking or online.",
            Command::ShowIdle => "Shows the lines that are waiting for a call.",
            Command::ShowNotReady => "Shows the lines that are not ready or are disabled.",
            Command::CallHistory => "Lists the calls that have ended.",
            Command::LineDiagnostics => {
                "Shows the modem of the selected line's call: its state, levels, carrier and scope."
            }
            Command::ViewToolbar => "Shows or hides the toolbar.",
            Command::ViewStatusBar => "Shows or hides the status bar.",
            Command::Refresh => "Asks the service for its lines, statistics and calls again.",
            Command::Settings => "Changes the settings of the service that are not lines.",
            Command::PortsAndDrivers => {
                "Lists the serial ports of the service's computer, with their pairs and modems."
            }
            Command::LoopbackTest => {
                "Joins the modems of two idle lines and checks what goes between them."
            }
            Command::CallCapture => {
                "Records every RTP packet of the calls of chosen lines to files."
            }
            Command::About => "Shows the version of the manager.",
        }
    }

    /// The phase that brings the command, for a command that is not built
    /// yet. Its place in the window is there, disabled, with this as the
    /// reason.
    pub fn blocked(self) -> Option<u8> {
        // Every command there is has been built. A command a later phase
        // brings is given its phase here when its place in the window is
        // made.
        let _ = self;
        None
    }

    /// Why a command that is not built yet cannot be chosen.
    pub fn blocked_reason(self) -> Option<String> {
        let phase = self.blocked()?;
        Some(format!("Not available yet: this comes in phase {phase}."))
    }

    /// Whether the command needs administrator rights, and so carries the
    /// shield when the manager does not have them.
    pub fn needs_elevation(self) -> bool {
        matches!(
            self,
            Command::ServiceStart
                | Command::ServiceStop
                | Command::ServiceRestart
                | Command::ServiceInstall
                | Command::ServiceUninstall
        )
    }

    /// The argument that names the command on the manager's command line,
    /// for the commands that are carried out after the manager has started
    /// itself again with administrator rights.
    pub fn argument(self) -> Option<&'static str> {
        match self {
            Command::ServiceStart => Some("start"),
            Command::ServiceStop => Some("stop"),
            Command::ServiceRestart => Some("restart"),
            Command::ServiceInstall => Some("install"),
            Command::ServiceUninstall => Some("uninstall"),
            _ => None,
        }
    }

    /// The command an argument of the manager's command line names.
    pub fn from_argument(argument: &str) -> Option<Command> {
        Command::ALL
            .into_iter()
            .find(|command| command.argument() == Some(argument))
    }
}

/// The menu bar: the text of each menu and its entries.
pub fn menu_bar() -> Vec<(&'static str, Vec<MenuEntry>)> {
    use Command as C;
    use MenuEntry::{Item, Separator};
    vec![
        (
            "&File",
            vec![
                Item(C::Connect),
                Item(C::Disconnect),
                Separator,
                Item(C::ReloadConfig),
                Separator,
                Item(C::ExportStatistics),
                Item(C::ExportLog),
                Separator,
                Item(C::ServiceStart),
                Item(C::ServiceStop),
                Item(C::ServiceRestart),
                Item(C::ServiceInstall),
                Item(C::ServiceUninstall),
                Separator,
                Item(C::Exit),
            ],
        ),
        ("&Lines", lines_menu()),
        (
            "&SIP",
            vec![
                Item(C::Reregister),
                Item(C::TrunkPeers),
                Item(C::HuntGroups),
                Item(C::Routes),
                Item(C::SipTrace),
            ],
        ),
        ("&Modem", vec![Item(C::AtConsole), Item(C::RateCap)]),
        (
            "&View",
            vec![
                MenuEntry::Submenu(
                    "&Show",
                    vec![C::ShowAll, C::ShowOnCall, C::ShowIdle, C::ShowNotReady],
                ),
                Item(C::CallHistory),
                Item(C::LineDiagnostics),
                Separator,
                Item(C::ViewToolbar),
                Item(C::ViewStatusBar),
                Separator,
                Item(C::Refresh),
            ],
        ),
        (
            "&Tools",
            vec![
                Item(C::Settings),
                Item(C::PortsAndDrivers),
                Item(C::LoopbackTest),
                Item(C::CallCapture),
            ],
        ),
        ("&Help", vec![Item(C::About)]),
    ]
}

/// The Lines menu, which is also the context menu of the grid.
pub fn lines_menu() -> Vec<MenuEntry> {
    use Command as C;
    use MenuEntry::{Item, Separator};
    vec![
        Item(C::Answer),
        Item(C::HangUp),
        Item(C::Reset),
        Separator,
        Item(C::Enable),
        Item(C::Disable),
        Separator,
        Item(C::NewLine),
        Item(C::EditLine),
        Item(C::RemoveLine),
    ]
}

/// The toolbar: its buttons, with `None` for a gap between groups.
pub fn toolbar() -> Vec<Option<Command>> {
    use Command as C;
    vec![
        Some(C::Answer),
        Some(C::HangUp),
        Some(C::Reset),
        None,
        Some(C::NewLine),
        Some(C::Reregister),
        Some(C::Settings),
        None,
    ]
}

/// Whether a command can be chosen now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Availability {
    /// It can.
    Enabled,
    /// It cannot, and this is why.
    Disabled(String),
}

impl Availability {
    /// Whether the command can be chosen.
    pub fn is_enabled(&self) -> bool {
        *self == Availability::Enabled
    }

    /// Why the command cannot be chosen, if it cannot.
    pub fn reason(&self) -> Option<&str> {
        match self {
            Availability::Enabled => None,
            Availability::Disabled(reason) => Some(reason),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    /// The letter after the ampersand.
    fn mnemonic(text: &str) -> Option<char> {
        let at = text.find('&')?;
        text[at + 1..]
            .chars()
            .next()
            .map(|c| c.to_ascii_lowercase())
    }

    fn items(entries: &[MenuEntry]) -> Vec<Command> {
        entries
            .iter()
            .flat_map(|entry| match entry {
                MenuEntry::Item(command) => vec![*command],
                MenuEntry::Submenu(_, commands) => commands.clone(),
                MenuEntry::Separator => Vec::new(),
            })
            .collect()
    }

    #[test]
    fn every_command_is_in_the_menu_bar_exactly_once() {
        let mut seen = Vec::new();
        for (_, entries) in menu_bar() {
            seen.extend(items(&entries));
        }
        let unique: BTreeSet<u16> = seen.iter().map(|command| command.id()).collect();
        assert_eq!(unique.len(), seen.len(), "a command is in two menus");
        assert_eq!(seen.len(), Command::ALL.len());
        for command in Command::ALL {
            assert!(seen.contains(&command), "{command:?} is in no menu");
        }
    }

    #[test]
    fn the_menus_are_the_ones_the_handoff_lists() {
        let bar = menu_bar();
        let titles: Vec<&str> = bar.iter().map(|(title, _)| *title).collect();
        assert_eq!(
            titles,
            [
                "&File", "&Lines", "&SIP", "&Modem", "&View", "&Tools", "&Help"
            ]
        );
        let texts = |menu: usize| -> Vec<String> {
            bar[menu]
                .1
                .iter()
                .map(|entry| match entry {
                    MenuEntry::Item(command) => command.text().replace('&', ""),
                    MenuEntry::Separator => "-".to_string(),
                    MenuEntry::Submenu(text, _) => format!("{}>", text.replace('&', "")),
                })
                .collect()
        };
        assert_eq!(
            texts(0),
            [
                "Connect to service…",
                "Disconnect",
                "-",
                "Reload configuration\tCtrl+R",
                "-",
                "Export statistics…",
                "Export event log…",
                "-",
                "Start service",
                "Stop service",
                "Restart service",
                "Install service…",
                "Uninstall service",
                "-",
                "Exit\tAlt+F4",
            ]
        );
        assert_eq!(
            texts(1),
            [
                "Answer now",
                "Hang up",
                "Reset",
                "-",
                "Enable",
                "Disable",
                "-",
                "New line…\tCtrl+N",
                "Edit line…\tEnter",
                "Remove line…\tDel",
            ]
        );
        assert_eq!(
            texts(2),
            [
                "Re-register",
                "Trunk peers…",
                "Hunt groups…",
                "Routes…",
                "SIP trace"
            ]
        );
        assert_eq!(texts(3), ["AT console…", "Rate cap and modulations…"]);
        assert_eq!(
            texts(4),
            [
                "Show>",
                "Call history…",
                "Line diagnostics…",
                "-",
                "Toolbar",
                "Status bar",
                "-",
                "Refresh\tF5",
            ]
        );
        assert_eq!(
            texts(5),
            [
                "Settings…",
                "Ports and drivers…",
                "Loopback self-test…",
                "Call capture…",
            ]
        );
        assert_eq!(texts(6), ["About RustModem Manager…"]);
        assert_eq!(lines_menu(), bar[1].1);
    }

    #[test]
    fn every_menu_item_has_a_mnemonic_that_is_its_own_within_its_menu() {
        let bar = menu_bar();
        let mut top = BTreeSet::new();
        for (title, entries) in &bar {
            let letter = mnemonic(title).unwrap_or_else(|| panic!("{title} has no mnemonic"));
            assert!(top.insert(letter), "{title} shares its mnemonic");
            let mut within = BTreeSet::new();
            for entry in entries {
                let texts: Vec<&str> = match entry {
                    MenuEntry::Item(command) => vec![command.text()],
                    MenuEntry::Submenu(text, commands) => {
                        // The submenu's own commands are a menu of their
                        // own.
                        let mut inside = BTreeSet::new();
                        for command in commands {
                            let letter = mnemonic(command.text()).unwrap();
                            assert!(inside.insert(letter), "{:?} in {text}", command);
                        }
                        vec![*text]
                    }
                    MenuEntry::Separator => Vec::new(),
                };
                for text in texts {
                    let letter = mnemonic(text).unwrap_or_else(|| panic!("{text} has no mnemonic"));
                    assert!(letter.is_ascii_alphabetic(), "{text}");
                    assert!(
                        within.insert(letter),
                        "{text} shares its mnemonic in {title}"
                    );
                }
            }
        }
    }

    #[test]
    fn every_command_has_an_id_a_text_and_a_description() {
        let mut ids = BTreeSet::new();
        for command in Command::ALL {
            assert!(ids.insert(command.id()));
            assert_eq!(Command::from_id(command.id()), Some(command));
            assert!(command.id() >= 1000, "clear of the dialog manager's ids");
            let description = command.description();
            assert!(description.ends_with('.'), "{description}");
            assert!(!description.contains('\n'));
            assert!(command.text().chars().count() < 40);
            // An accelerator is after a tab, and nowhere else.
            assert!(command.text().matches('\t').count() <= 1);
        }
        assert_eq!(Command::from_id(999), None);
        assert_eq!(Command::from_id(1000 + Command::ALL.len() as u16), None);
        assert_eq!(Command::from_id(0), None);
    }

    #[test]
    fn the_toolbar_is_the_handoff_s_without_reload_config() {
        let labels: Vec<&str> = toolbar()
            .iter()
            .map(|button| match button {
                Some(command) => command.tool_label().expect("a button has a label"),
                None => "|",
            })
            .collect();
        assert_eq!(
            labels,
            [
                "Answer now",
                "Hang up",
                "Reset line",
                "|",
                "New line…",
                "Re-register",
                "Settings…",
                "|",
            ]
        );
        // Only the toolbar's commands have labels.
        let with_labels = Command::ALL
            .iter()
            .filter(|command| command.tool_label().is_some())
            .count();
        assert_eq!(with_labels, 6);
        // Reloading the configuration is in the File menu only.
        assert_eq!(Command::ReloadConfig.tool_label(), None);
    }

    #[test]
    fn the_commands_that_are_not_built_yet_say_which_phase_brings_them() {
        let blocked: Vec<(Command, u8)> = Command::ALL
            .iter()
            .filter_map(|command| Some((*command, command.blocked()?)))
            .collect();
        // The first modulation brought the last of them.
        assert_eq!(blocked, []);
        for command in [
            Command::RateCap,
            Command::LineDiagnostics,
            Command::LoopbackTest,
        ] {
            assert_eq!(command.blocked_reason(), None);
        }
        assert_eq!(Command::Answer.blocked_reason(), None);
        // What came with telephony is built.
        assert_eq!(Command::Reregister.blocked_reason(), None);
        assert_eq!(Command::CallCapture.blocked_reason(), None);
    }

    #[test]
    fn the_service_commands_need_elevation_and_have_an_argument() {
        for command in Command::ALL {
            assert_eq!(command.needs_elevation(), command.argument().is_some());
            if let Some(argument) = command.argument() {
                assert_eq!(Command::from_argument(argument), Some(command));
            }
        }
        assert_eq!(Command::from_argument("explode"), None);
        let arguments: Vec<&str> = Command::ALL.iter().filter_map(|c| c.argument()).collect();
        assert_eq!(
            arguments,
            ["start", "stop", "restart", "install", "uninstall"]
        );
    }

    #[test]
    fn availability_says_whether_and_why_not() {
        assert!(Availability::Enabled.is_enabled());
        assert_eq!(Availability::Enabled.reason(), None);
        let disabled = Availability::Disabled("Select a line first.".to_string());
        assert!(!disabled.is_enabled());
        assert_eq!(disabled.reason(), Some("Select a line first."));
    }
}
