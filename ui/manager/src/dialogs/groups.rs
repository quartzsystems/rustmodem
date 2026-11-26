// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Hunt groups: lines that share a number.
//!
//! The dialog lists the groups, with Add, Edit and Remove. The editor has
//! the group's name, its pilot number, how it chooses among its idle
//! lines, and two lists: the lines that are in the group, in the order
//! they are hunted, and the lines that are not. Every change goes to the
//! service at once and is written to its configuration file; when the
//! service refuses one, its message is shown and the editor stays open
//! as it was.

use std::cell::RefCell;
use std::rc::Rc;

use ctlproto::{
    Call, Done, ErrorInfo, HuntGroupChange, HuntGroupInfo, HuntGroupList, HuntGroupRef,
    HuntGroupSpec,
};
use win32::controls::Combo;
use win32::metrics::scale;
use win32::{
    Align, Column, DialogTemplate, Event, Handler, Hwnd, ListBox, ListView, Reply, TaskButtons,
    TaskDialog, TaskIcon, code, id,
};

use super::{must_not_be_empty, watch, watched};
use crate::window::Manager;

/// The ids of the controls of the list of groups.
pub mod ids {
    /// The list.
    pub const LIST: u16 = 300;
    /// Add.
    pub const ADD: u16 = 301;
    /// Edit.
    pub const EDIT: u16 = 302;
    /// Remove.
    pub const REMOVE: u16 = 303;
}

/// The ids of the controls of the editor.
pub mod editor {
    /// The name.
    pub const NAME: u16 = 310;
    /// The pilot number.
    pub const PILOT: u16 = 311;
    /// The strategy.
    pub const STRATEGY: u16 = 312;
    /// The lines in the group.
    pub const MEMBERS: u16 = 313;
    /// The lines not in the group.
    pub const OTHERS: u16 = 314;
    /// Puts the line chosen among the others in the group.
    pub const INCLUDE: u16 = 315;
    /// Takes the line chosen in the group out of it.
    pub const EXCLUDE: u16 = 316;
    /// Moves the line chosen in the group up.
    pub const UP: u16 = 317;
    /// Moves the line chosen in the group down.
    pub const DOWN: u16 = 318;
}

use ids::{ADD as ID_ADD, EDIT as ID_EDIT, LIST as ID_LIST, REMOVE as ID_REMOVE};

/// The strategies, in the order the combo box lists them: what the
/// service calls each, and what the user is shown.
pub const STRATEGIES: [(&str, &str); 3] = [
    ("first-idle", "First idle line, in the order below"),
    (
        "round-robin",
        "The line after the one that took the last call",
    ),
    ("least-recent", "The line that has been idle longest"),
];

/// The columns of the list: their titles and their widths at 96 DPI.
const COLUMNS: [(&str, i32); 4] = [
    ("Name", 110),
    ("Pilot number", 100),
    ("Strategy", 100),
    ("Lines", 190),
];

fn template() -> DialogTemplate {
    DialogTemplate::new("Hunt groups", 350, 190)
        .list_view(ID_LIST, 7, 7, 336, 155)
        .button(ID_ADD, 7, 169, 50, 14, "&Add\u{2026}")
        .button(ID_EDIT, 61, 169, 50, 14, "&Edit\u{2026}")
        .button(ID_REMOVE, 115, 169, 50, 14, "&Remove")
        .close_button()
}

fn editor_template(title: &str) -> DialogTemplate {
    DialogTemplate::new(title, 300, 226)
        .label(7, 9, 60, 8, "&Name:")
        .edit(editor::NAME, 70, 7, 120, 14)
        .label(7, 27, 60, 8, "&Pilot number:")
        .edit(editor::PILOT, 70, 25, 120, 14)
        .label(7, 45, 60, 8, "&Hunt for:")
        .combo(editor::STRATEGY, 70, 43, 223, 60)
        .label(7, 66, 115, 8, "Lines &in the group, in order:")
        .list_box(editor::MEMBERS, 7, 77, 115, 100)
        .button(editor::INCLUDE, 127, 100, 46, 14, "< A&dd")
        .button(editor::EXCLUDE, 127, 118, 46, 14, "&Remove >")
        .label(178, 66, 115, 8, "&Lines not in the group:")
        .list_box(editor::OTHERS, 178, 77, 115, 100)
        .button(editor::UP, 7, 181, 55, 14, "Move &up")
        .button(editor::DOWN, 66, 181, 56, 14, "Move do&wn")
        .ok_cancel()
}

/// What a row of the list shows of a group.
pub fn row(group: &HuntGroupInfo) -> [String; 4] {
    [
        group.name.clone(),
        group.pilot.clone(),
        group.strategy.clone(),
        group
            .lines
            .iter()
            .map(u16::to_string)
            .collect::<Vec<_>>()
            .join(", "),
    ]
}

/// The lines of the editor's two lists: those in the group, in hunting
/// order, and those that are not, in id order.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Members {
    /// The lines in the group, in the order they are hunted.
    pub members: Vec<u16>,
    /// The lines not in the group.
    pub others: Vec<u16>,
}

impl Members {
    /// The two lists for a group with these lines, of a service that has
    /// `all` lines. A line of the group that the service does not have is
    /// kept: taking it out is the user's to do.
    pub fn new(group: &[u16], all: &[u16]) -> Members {
        let mut others: Vec<u16> = all
            .iter()
            .copied()
            .filter(|line| !group.contains(line))
            .collect();
        others.sort_unstable();
        Members {
            members: group.to_vec(),
            others,
        }
    }

    /// Puts the line at `index` of the others at the end of the group.
    /// Returns where in the group it is.
    pub fn include(&mut self, index: usize) -> Option<usize> {
        if index >= self.others.len() {
            return None;
        }
        let line = self.others.remove(index);
        self.members.push(line);
        Some(self.members.len() - 1)
    }

    /// Takes the line at `index` of the group out of it. Returns where
    /// among the others it is.
    pub fn exclude(&mut self, index: usize) -> Option<usize> {
        if index >= self.members.len() {
            return None;
        }
        let line = self.members.remove(index);
        let at = self.others.partition_point(|other| *other < line);
        self.others.insert(at, line);
        Some(at)
    }

    /// Moves the line at `index` of the group one place up or down.
    /// Returns where it is afterwards, or `None` if it is at the end
    /// already.
    pub fn shift(&mut self, index: usize, up: bool) -> Option<usize> {
        let to = if up { index.checked_sub(1)? } else { index + 1 };
        if index >= self.members.len() || to >= self.members.len() {
            return None;
        }
        self.members.swap(index, to);
        Some(to)
    }
}

/// What the editor holds, as it was typed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Form {
    /// The name.
    pub name: String,
    /// The pilot number.
    pub pilot: String,
    /// The strategy, as the service calls it.
    pub strategy: String,
    /// The lines in the group, in hunting order.
    pub lines: Vec<u16>,
}

impl Form {
    fn checked(&self) -> Result<(String, String), ErrorInfo> {
        let (name, pilot) = (self.name.trim(), self.pilot.trim());
        if name.is_empty() {
            return Err(must_not_be_empty("Name"));
        }
        if pilot.is_empty() {
            return Err(must_not_be_empty("Pilot number"));
        }
        Ok((name.to_string(), pilot.to_string()))
    }

    /// The group to add.
    pub fn spec(&self) -> Result<HuntGroupSpec, ErrorInfo> {
        let (name, pilot) = self.checked()?;
        Ok(HuntGroupSpec {
            name,
            pilot,
            lines: self.lines.clone(),
            strategy: Some(self.strategy.clone()),
        })
    }

    /// What to change of a group: only what is not as the group has it.
    /// `None` when nothing is.
    pub fn change(&self, group: &HuntGroupInfo) -> Result<Option<HuntGroupChange>, ErrorInfo> {
        let (name, pilot) = self.checked()?;
        let change = HuntGroupChange {
            name: group.name.clone(),
            new_name: (name != group.name).then_some(name),
            pilot: (pilot != group.pilot).then_some(pilot),
            lines: (self.lines != group.lines).then(|| self.lines.clone()),
            strategy: (self.strategy != group.strategy).then(|| self.strategy.clone()),
        };
        let changed = change.new_name.is_some()
            || change.pilot.is_some()
            || change.lines.is_some()
            || change.strategy.is_some();
        Ok(changed.then_some(change))
    }
}

/// A line as the editor's lists show it: its id, and its name if it has
/// one.
fn line_entry(id: u16, names: &[(u16, Option<String>)]) -> String {
    match names.iter().find(|(line, _)| *line == id) {
        Some((_, Some(name))) => format!("Line {id} ({name})"),
        Some((_, None)) => format!("Line {id}"),
        None => format!("Line {id} (not a line of the service)"),
    }
}

struct Editor {
    manager: Rc<Manager>,
    /// The group that is edited, or `None` for a new one.
    group: Option<HuntGroupInfo>,
    /// The lines of the service, with their names.
    names: Vec<(u16, Option<String>)>,
    members: RefCell<Members>,
}

impl Editor {
    /// Fills the two lists, with a line chosen in each if there is one
    /// to choose.
    fn show(&self, dialog: Hwnd, member: Option<usize>, other: Option<usize>) {
        let lists = self.members.borrow();
        let entries = |lines: &[u16]| -> Vec<String> {
            lines
                .iter()
                .map(|line| line_entry(*line, &self.names))
                .collect()
        };
        ListBox::of(dialog.item(editor::MEMBERS)).set_entries(&entries(&lists.members), member);
        ListBox::of(dialog.item(editor::OTHERS)).set_entries(&entries(&lists.others), other);
        drop(lists);
        self.show_buttons(dialog);
    }

    /// The buttons between and under the lists, as what is chosen allows.
    fn show_buttons(&self, dialog: Hwnd) {
        let member = ListBox::of(dialog.item(editor::MEMBERS)).selected();
        let other = ListBox::of(dialog.item(editor::OTHERS)).selected();
        let count = self.members.borrow().members.len();
        dialog.item(editor::INCLUDE).set_enabled(other.is_some());
        dialog.item(editor::EXCLUDE).set_enabled(member.is_some());
        dialog
            .item(editor::UP)
            .set_enabled(member.is_some_and(|index| index > 0));
        dialog
            .item(editor::DOWN)
            .set_enabled(member.is_some_and(|index| index + 1 < count));
    }

    fn read(&self, dialog: Hwnd) -> Form {
        let strategy = Combo::of(dialog.item(editor::STRATEGY))
            .selected()
            .and_then(|index| STRATEGIES.get(index))
            .map_or(STRATEGIES[0].0, |(name, _)| *name);
        Form {
            name: dialog.item(editor::NAME).text(),
            pilot: dialog.item(editor::PILOT).text(),
            strategy: strategy.to_string(),
            lines: self.members.borrow().members.clone(),
        }
    }

    /// Sends what the editor holds. `Ok` when there was nothing to send
    /// or the service took it.
    fn send(&self, dialog: Hwnd) -> Result<(), ErrorInfo> {
        let form = self.read(dialog);
        let call = match &self.group {
            None => Call::HuntGroupsAdd(form.spec()?),
            Some(group) => match form.change(group)? {
                Some(change) => Call::HuntGroupsEdit(change),
                None => return Ok(()),
            },
        };
        self.manager.call::<HuntGroupInfo>(dialog, call).map(|_| ())
    }
}

impl Handler for Editor {
    fn event(&self, dialog: Hwnd, event: Event<'_>) -> Reply {
        match event {
            Event::InitDialog => {
                let strategy = Combo::of(dialog.item(editor::STRATEGY));
                for (_, text) in STRATEGIES {
                    strategy.add(text);
                }
                let chosen = self.group.as_ref().and_then(|group| {
                    STRATEGIES
                        .iter()
                        .position(|(name, _)| *name == group.strategy)
                });
                strategy.select(Some(chosen.unwrap_or(0)));
                if let Some(group) = &self.group {
                    dialog.item(editor::NAME).set_text(&group.name);
                    dialog.item(editor::PILOT).set_text(&group.pilot);
                }
                self.show(dialog, None, None);
                watch(&self.manager, dialog);
                Reply::Default
            }
            Event::Timer { id } if watched(&self.manager, dialog, id, "hunt-group") => {
                Reply::Handled
            }
            Event::Command {
                id: editor::MEMBERS | editor::OTHERS,
                code: code::LIST_SELECTED,
            } => {
                self.show_buttons(dialog);
                Reply::Handled
            }
            // A double click moves a line across.
            Event::Command {
                id: editor::OTHERS,
                code: code::LIST_DOUBLE_CLICK,
            }
            | Event::Command {
                id: editor::INCLUDE,
                ..
            } => {
                let chosen = ListBox::of(dialog.item(editor::OTHERS)).selected();
                let moved = chosen.and_then(|index| self.members.borrow_mut().include(index));
                if moved.is_some() {
                    self.show(dialog, moved, None);
                }
                Reply::Handled
            }
            Event::Command {
                id: editor::MEMBERS,
                code: code::LIST_DOUBLE_CLICK,
            }
            | Event::Command {
                id: editor::EXCLUDE,
                ..
            } => {
                let chosen = ListBox::of(dialog.item(editor::MEMBERS)).selected();
                let moved = chosen.and_then(|index| self.members.borrow_mut().exclude(index));
                if moved.is_some() {
                    self.show(dialog, None, moved);
                }
                Reply::Handled
            }
            Event::Command {
                id: button @ (editor::UP | editor::DOWN),
                ..
            } => {
                let chosen = ListBox::of(dialog.item(editor::MEMBERS)).selected();
                let moved = chosen
                    .and_then(|index| self.members.borrow_mut().shift(index, button == editor::UP));
                if moved.is_some() {
                    self.show(dialog, moved, None);
                }
                Reply::Handled
            }
            Event::Command { id: id::OK, .. } => {
                match self.send(dialog) {
                    Ok(()) => dialog.end_dialog(id::OK),
                    // The editor stays, with what was typed.
                    Err(error) => self.manager.show_error(dialog, &error),
                }
                Reply::Handled
            }
            _ => Reply::Default,
        }
    }
}

struct Groups {
    manager: Rc<Manager>,
    groups: RefCell<Vec<HuntGroupInfo>>,
}

impl Groups {
    fn show(&self, dialog: Hwnd) {
        let list = ListView::of(dialog.item(ID_LIST));
        let count = self.groups.borrow().len();
        list.set_row_count(count);
        if list.selected().is_some_and(|row| row >= count) {
            list.select(None);
        }
        list.redraw_visible();
        self.show_buttons(dialog);
    }

    fn show_buttons(&self, dialog: Hwnd) {
        let chosen = ListView::of(dialog.item(ID_LIST)).selected().is_some();
        dialog.item(ID_EDIT).set_enabled(chosen);
        dialog.item(ID_REMOVE).set_enabled(chosen);
    }

    fn selected(&self, dialog: Hwnd) -> Option<HuntGroupInfo> {
        let row = ListView::of(dialog.item(ID_LIST)).selected()?;
        self.groups.borrow().get(row).cloned()
    }

    /// Asks the service for the groups again, and has the main window
    /// ask for the lines, which show the groups they are in.
    fn reload(&self, dialog: Hwnd) {
        match self
            .manager
            .call::<HuntGroupList>(dialog, Call::HuntGroupsList)
        {
            Ok(list) => *self.groups.borrow_mut() = list.groups,
            Err(error) => self.manager.show_error(dialog, &error),
        }
        self.manager.refresh_lines();
        self.show(dialog);
    }

    /// Opens the editor, for a group or for a new one.
    fn edit(&self, dialog: Hwnd, group: Option<HuntGroupInfo>) {
        let names: Vec<(u16, Option<String>)> = self.manager.with_model(|model| {
            model
                .lines
                .lines()
                .map(|line| (line.id, line.name.clone()))
                .collect()
        });
        let all: Vec<u16> = names.iter().map(|(id, _)| *id).collect();
        let lines = group.as_ref().map_or(&[][..], |group| &group.lines);
        let title = if group.is_some() {
            "Edit hunt group"
        } else {
            "New hunt group"
        };
        let handler = Rc::new(Editor {
            manager: Rc::clone(&self.manager),
            members: RefCell::new(Members::new(lines, &all)),
            names,
            group,
        });
        if editor_template(title).show_modal(Some(dialog), handler) == id::OK {
            self.reload(dialog);
        }
        // The list is what is up again: a test is told so.
        watch(&self.manager, dialog);
    }

    /// Remove: asks first, naming the group.
    fn remove(&self, dialog: Hwnd) {
        let Some(group) = self.selected(dialog) else {
            return;
        };
        let question = TaskDialog {
            title: "Remove hunt group".to_string(),
            instruction: format!("Remove the hunt group {}?", group.name),
            content: format!(
                "Calls to {} are no longer hunted over its lines. The lines themselves stay.",
                group.pilot
            ),
            expanded: String::new(),
            icon: TaskIcon::Warning,
            buttons: TaskButtons::YES_NO,
            links: Vec::new(),
            default: Some(id::NO),
        };
        if self.manager.task(Some(dialog), &question) != id::YES {
            return;
        }
        let call = Call::HuntGroupsRemove(HuntGroupRef { name: group.name });
        if let Err(error) = self.manager.call::<Done>(dialog, call) {
            self.manager.show_error(dialog, &error);
        }
        self.reload(dialog);
        watch(&self.manager, dialog);
    }
}

impl Handler for Groups {
    fn event(&self, dialog: Hwnd, event: Event<'_>) -> Reply {
        match event {
            Event::InitDialog => {
                let dpi = dialog.dpi();
                let list = ListView::of(dialog.item(ID_LIST));
                let columns: Vec<Column> = COLUMNS
                    .iter()
                    .map(|(title, width)| Column::new(title, scale(*width, dpi), Align::Left))
                    .collect();
                list.set_columns(&columns);
                self.show(dialog);
                watch(&self.manager, dialog);
                Reply::Default
            }
            Event::Timer { id } if watched(&self.manager, dialog, id, "hunt-groups") => {
                Reply::Handled
            }
            Event::ListSelection { id: ID_LIST } => {
                self.show_buttons(dialog);
                Reply::Handled
            }
            Event::Command { id: ID_ADD, .. } => {
                self.edit(dialog, None);
                Reply::Handled
            }
            Event::Command { id: ID_EDIT, .. } | Event::ListActivate { id: ID_LIST, .. } => {
                if let Some(group) = self.selected(dialog) {
                    self.edit(dialog, Some(group));
                }
                Reply::Handled
            }
            Event::Command { id: ID_REMOVE, .. } => {
                self.remove(dialog);
                Reply::Handled
            }
            Event::ListCell {
                id: ID_LIST,
                row: index,
                column,
                cell,
            } => {
                if let Ok(groups) = self.groups.try_borrow()
                    && let Some(group) = groups.get(index)
                    && let Some(text) = row(group).get(column)
                {
                    cell.text = text.clone();
                }
                Reply::Handled
            }
            _ => Reply::Default,
        }
    }
}

/// Shows the hunt groups, to add to, change and take from.
pub fn show(manager: &Rc<Manager>, owner: Hwnd) {
    // Asked for before the dialog opens, so that it opens filled in.
    let groups = match manager.call::<HuntGroupList>(owner, Call::HuntGroupsList) {
        Ok(list) => list.groups,
        Err(error) => {
            manager.show_error(owner, &error);
            return;
        }
    };
    let handler = Rc::new(Groups {
        manager: Rc::clone(manager),
        groups: RefCell::new(groups),
    });
    template().show_modal(Some(owner), handler);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group() -> HuntGroupInfo {
        HuntGroupInfo {
            name: "dialup".to_string(),
            pilot: "5551000".to_string(),
            lines: vec![3, 1],
            strategy: "round-robin".to_string(),
        }
    }

    #[test]
    fn a_row_is_the_group_and_its_lines_in_hunting_order() {
        assert_eq!(row(&group()), ["dialup", "5551000", "round-robin", "3, 1"]);
        assert_eq!(COLUMNS.len(), row(&group()).len());
    }

    #[test]
    fn the_lines_go_between_the_two_lists_and_up_and_down() {
        let mut lists = Members::new(&[3, 1], &[1, 2, 3, 4, 5]);
        assert_eq!(lists.members, [3, 1]);
        assert_eq!(lists.others, [2, 4, 5]);

        // Into the group at its end; out of it to its place by id.
        assert_eq!(lists.include(1), Some(2));
        assert_eq!(lists.members, [3, 1, 4]);
        assert_eq!(lists.others, [2, 5]);
        assert_eq!(lists.exclude(0), Some(1));
        assert_eq!(lists.members, [1, 4]);
        assert_eq!(lists.others, [2, 3, 5]);

        assert_eq!(lists.shift(1, true), Some(0));
        assert_eq!(lists.members, [4, 1]);
        assert_eq!(lists.shift(0, true), None);
        assert_eq!(lists.shift(0, false), Some(1));
        assert_eq!(lists.members, [1, 4]);
        assert_eq!(lists.shift(1, false), None);

        // What is not there cannot be moved.
        assert_eq!(lists.include(9), None);
        assert_eq!(lists.exclude(9), None);
        assert_eq!(lists.shift(9, true), None);
    }

    #[test]
    fn a_line_of_the_group_that_the_service_does_not_have_stays_in_it() {
        let lists = Members::new(&[7, 1], &[1, 2]);
        assert_eq!(lists.members, [7, 1]);
        assert_eq!(lists.others, [2]);
        let names = vec![(1, Some("Front desk".to_string())), (2, None)];
        assert_eq!(line_entry(1, &names), "Line 1 (Front desk)");
        assert_eq!(line_entry(2, &names), "Line 2");
        assert_eq!(line_entry(7, &names), "Line 7 (not a line of the service)");
    }

    #[test]
    fn a_new_group_needs_a_name_and_a_pilot_number() {
        let form = Form {
            name: " night ".to_string(),
            pilot: " 5552000 ".to_string(),
            strategy: "least-recent".to_string(),
            lines: vec![2, 1],
        };
        let spec = form.spec().unwrap();
        assert_eq!(spec.name, "night");
        assert_eq!(spec.pilot, "5552000");
        assert_eq!(spec.lines, [2, 1]);
        assert_eq!(spec.strategy.as_deref(), Some("least-recent"));

        let nameless = Form {
            name: "  ".to_string(),
            ..form.clone()
        };
        assert_eq!(nameless.spec().unwrap_err().text, "Name must not be empty.");
        let pilotless = Form {
            pilot: String::new(),
            ..form
        };
        assert_eq!(
            pilotless.spec().unwrap_err().text,
            "Pilot number must not be empty."
        );
    }

    #[test]
    fn only_what_was_changed_of_a_group_is_sent() {
        let group = group();
        let same = Form {
            name: "dialup".to_string(),
            pilot: "5551000".to_string(),
            strategy: "round-robin".to_string(),
            lines: vec![3, 1],
        };
        assert_eq!(same.change(&group), Ok(None));

        let renamed = Form {
            name: "modems".to_string(),
            lines: vec![1, 3],
            ..same.clone()
        };
        let change = renamed.change(&group).unwrap().unwrap();
        assert_eq!(change.name, "dialup");
        assert_eq!(change.new_name.as_deref(), Some("modems"));
        assert_eq!(change.lines, Some(vec![1, 3]));
        assert_eq!((change.pilot, change.strategy), (None, None));

        let other = Form {
            pilot: "5551001".to_string(),
            strategy: "first-idle".to_string(),
            ..same
        };
        let change = other.change(&group).unwrap().unwrap();
        assert_eq!(change.pilot.as_deref(), Some("5551001"));
        assert_eq!(change.strategy.as_deref(), Some("first-idle"));
        assert_eq!((change.new_name, change.lines), (None, None));
    }

    #[test]
    fn every_strategy_of_the_service_can_be_chosen() {
        let names: Vec<&str> = STRATEGIES.iter().map(|(name, _)| *name).collect();
        assert_eq!(names, ["first-idle", "round-robin", "least-recent"]);
    }
}
