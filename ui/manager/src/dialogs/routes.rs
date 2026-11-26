// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Routes: which called numbers ring which lines and hunt groups.
//!
//! The dialog lists the routes in the order the service tries them: a
//! call takes the first route that matches the number it was made to.
//! Add, Edit, Remove, Move up and Move down each go to the service at
//! once and are written to its configuration file; the service answers
//! every one with the routes as they then are.

use std::cell::RefCell;
use std::rc::Rc;

use ctlproto::{
    Call, ErrorInfo, HuntGroupList, RouteChange, RouteInfo, RouteList, RouteMove, RouteRef,
    RouteSpec,
};
use win32::controls::Combo;
use win32::metrics::scale;
use win32::{Align, Column, DialogTemplate, Event, Handler, Hwnd, ListView, Reply, id};

use super::{must_not_be_empty, watch, watched};
use crate::window::Manager;

/// The ids of the controls of the list of routes.
pub mod ids {
    /// The list.
    pub const LIST: u16 = 300;
    /// Add.
    pub const ADD: u16 = 301;
    /// Edit.
    pub const EDIT: u16 = 302;
    /// Remove.
    pub const REMOVE: u16 = 303;
    /// Move up.
    pub const UP: u16 = 304;
    /// Move down.
    pub const DOWN: u16 = 305;
}

/// The ids of the controls of the editor.
pub mod editor {
    /// The called number.
    pub const CALLED: u16 = 310;
    /// Where the calls go.
    pub const TARGET: u16 = 311;
}

use ids::{
    ADD as ID_ADD, DOWN as ID_DOWN, EDIT as ID_EDIT, LIST as ID_LIST, REMOVE as ID_REMOVE,
    UP as ID_UP,
};

/// The columns of the list: their titles, their widths at 96 DPI, and
/// whether they hold numbers.
const COLUMNS: [(&str, i32, bool); 3] = [
    ("#", 40, true),
    ("Called number", 150, false),
    ("Rings", 190, false),
];

fn template() -> DialogTemplate {
    DialogTemplate::new("Routes", 300, 200)
        .label(
            7,
            7,
            286,
            8,
            "A call takes the first route, from the top, that matches the number called.",
        )
        .list_view(ID_LIST, 7, 20, 232, 152)
        .button(ID_UP, 243, 20, 50, 14, "Move &up")
        .button(ID_DOWN, 243, 38, 50, 14, "Move &down")
        .button(ID_ADD, 7, 179, 50, 14, "&Add\u{2026}")
        .button(ID_EDIT, 61, 179, 50, 14, "&Edit\u{2026}")
        .button(ID_REMOVE, 115, 179, 50, 14, "&Remove")
        .close_button()
}

fn editor_template(title: &str) -> DialogTemplate {
    DialogTemplate::new(title, 250, 96)
        .label(7, 9, 60, 8, "&Called number:")
        .edit(editor::CALLED, 70, 7, 120, 14)
        .label(
            70,
            24,
            173,
            16,
            "A whole number, or its first digits and an asterisk: 555* is every \
             number that begins with 555.",
        )
        .label(7, 47, 60, 8, "&Rings:")
        .combo(editor::TARGET, 70, 45, 173, 120)
        .ok_cancel()
}

/// A target as people read it: `line:3` is "Line 3", `group:dialup` is
/// "Hunt group dialup".
pub fn target_text(target: &str) -> String {
    match target.split_once(':') {
        Some(("line", id)) => format!("Line {id}"),
        Some(("group", name)) => format!("Hunt group {name}"),
        _ => target.to_string(),
    }
}

/// What a row of the list shows of a route. The routes are numbered from
/// one for people.
pub fn row(route: &RouteInfo) -> [String; 3] {
    [
        (route.index + 1).to_string(),
        route.called.clone(),
        target_text(&route.target),
    ]
}

/// The targets the editor offers: every line, then every hunt group, as
/// the service names each.
pub fn targets(lines: &[u16], groups: &[String]) -> Vec<String> {
    lines
        .iter()
        .map(|line| format!("line:{line}"))
        .chain(groups.iter().map(|group| format!("group:{group}")))
        .collect()
}

/// The route to add, at the end.
pub fn spec(called: &str, target: Option<&str>) -> Result<RouteSpec, ErrorInfo> {
    let called = called.trim();
    if called.is_empty() {
        return Err(must_not_be_empty("Called number"));
    }
    let target = target.ok_or_else(|| must_not_be_empty("Rings"))?;
    Ok(RouteSpec {
        called: called.to_string(),
        target: target.to_string(),
        index: None,
    })
}

/// What to change of a route: only what is not as the route has it.
/// `None` when nothing is.
pub fn change(
    route: &RouteInfo,
    called: &str,
    target: Option<&str>,
) -> Result<Option<RouteChange>, ErrorInfo> {
    let new = spec(called, target)?;
    let change = RouteChange {
        index: route.index,
        called: (new.called != route.called).then_some(new.called),
        target: (new.target != route.target).then_some(new.target),
    };
    Ok((change.called.is_some() || change.target.is_some()).then_some(change))
}

struct Editor {
    manager: Rc<Manager>,
    /// The route that is edited, or `None` for a new one.
    route: Option<RouteInfo>,
    /// The targets, in the order of the combo box.
    targets: Vec<String>,
    /// The routes as the service answered, once it has.
    answer: RefCell<Option<Vec<RouteInfo>>>,
}

impl Editor {
    fn send(&self, dialog: Hwnd) -> Result<(), ErrorInfo> {
        let called = dialog.item(editor::CALLED).text();
        let target = Combo::of(dialog.item(editor::TARGET))
            .selected()
            .and_then(|index| self.targets.get(index))
            .map(String::as_str);
        let call = match &self.route {
            None => Call::RoutesAdd(spec(&called, target)?),
            Some(route) => match change(route, &called, target)? {
                Some(change) => Call::RoutesEdit(change),
                None => return Ok(()),
            },
        };
        let list: RouteList = self.manager.call(dialog, call)?;
        *self.answer.borrow_mut() = Some(list.routes);
        Ok(())
    }
}

impl Handler for Editor {
    fn event(&self, dialog: Hwnd, event: Event<'_>) -> Reply {
        match event {
            Event::InitDialog => {
                let target = Combo::of(dialog.item(editor::TARGET));
                for entry in &self.targets {
                    target.add(&target_text(entry));
                }
                match &self.route {
                    Some(route) => {
                        dialog.item(editor::CALLED).set_text(&route.called);
                        target.select(self.targets.iter().position(|t| *t == route.target));
                    }
                    None => target.select((!self.targets.is_empty()).then_some(0)),
                }
                watch(&self.manager, dialog);
                Reply::Default
            }
            Event::Timer { id } if watched(&self.manager, dialog, id, "route") => Reply::Handled,
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

struct Routes {
    manager: Rc<Manager>,
    routes: RefCell<Vec<RouteInfo>>,
}

impl Routes {
    fn list(dialog: Hwnd) -> ListView {
        ListView::of(dialog.item(ID_LIST))
    }

    /// Shows the routes, with the one at `selected` chosen.
    fn show(&self, dialog: Hwnd, selected: Option<usize>) {
        let list = Self::list(dialog);
        let count = self.routes.borrow().len();
        list.set_row_count(count);
        list.select(selected.filter(|row| *row < count));
        list.redraw_visible();
        self.show_buttons(dialog);
    }

    fn show_buttons(&self, dialog: Hwnd) {
        let chosen = Self::list(dialog).selected();
        let count = self.routes.borrow().len();
        dialog.item(ID_EDIT).set_enabled(chosen.is_some());
        dialog.item(ID_REMOVE).set_enabled(chosen.is_some());
        dialog
            .item(ID_UP)
            .set_enabled(chosen.is_some_and(|row| row > 0));
        dialog
            .item(ID_DOWN)
            .set_enabled(chosen.is_some_and(|row| row + 1 < count));
    }

    fn selected(&self, dialog: Hwnd) -> Option<RouteInfo> {
        let row = Self::list(dialog).selected()?;
        self.routes.borrow().get(row).cloned()
    }

    /// Asks the service for something that answers with the routes, and
    /// shows them with the one at `then` chosen.
    fn ask(&self, dialog: Hwnd, call: Call, then: Option<usize>) {
        match self.manager.call::<RouteList>(dialog, call) {
            Ok(list) => {
                *self.routes.borrow_mut() = list.routes;
                self.show(dialog, then);
            }
            Err(error) => {
                self.manager.show_error(dialog, &error);
                // The routes may not be what the list shows any more.
                if let Ok(list) = self.manager.call::<RouteList>(dialog, Call::RoutesList) {
                    *self.routes.borrow_mut() = list.routes;
                }
                self.show(dialog, None);
            }
        }
        // The list has changed: a test is told that it is up again.
        watch(&self.manager, dialog);
    }

    /// Opens the editor, for a route or for a new one.
    fn edit(&self, dialog: Hwnd, route: Option<RouteInfo>) {
        let lines: Vec<u16> = self
            .manager
            .with_model(|model| model.lines.lines().map(|line| line.id).collect());
        // The groups are asked for now: one may have been made since the
        // lines last said which they are in, and one with no lines is in
        // none of them.
        let groups: Vec<String> = match self
            .manager
            .call::<HuntGroupList>(dialog, Call::HuntGroupsList)
        {
            Ok(list) => list.groups.into_iter().map(|group| group.name).collect(),
            Err(error) => {
                self.manager.show_error(dialog, &error);
                return;
            }
        };
        let title = if route.is_some() {
            "Edit route"
        } else {
            "New route"
        };
        let row = route.as_ref().map(|route| route.index as usize);
        let handler = Rc::new(Editor {
            manager: Rc::clone(&self.manager),
            route,
            targets: targets(&lines, &groups),
            answer: RefCell::new(None),
        });
        let closed = editor_template(title).show_modal(Some(dialog), handler.clone());
        // The list is what is up again: a test is told so.
        watch(&self.manager, dialog);
        if closed != id::OK {
            return;
        }
        if let Some(routes) = handler.answer.borrow_mut().take() {
            // A new route is the last; one that was edited is where it
            // was.
            let chosen = row.or(routes.len().checked_sub(1));
            *self.routes.borrow_mut() = routes;
            self.show(dialog, chosen);
        }
    }

    fn shift(&self, dialog: Hwnd, up: bool) {
        let Some(row) = Self::list(dialog).selected() else {
            return;
        };
        let to = if up {
            row.checked_sub(1)
        } else {
            Some(row + 1)
        };
        let Some(to) = to.filter(|to| *to < self.routes.borrow().len()) else {
            return;
        };
        let call = Call::RoutesMove(RouteMove {
            index: row as u32,
            to: to as u32,
        });
        self.ask(dialog, call, Some(to));
    }
}

impl Handler for Routes {
    fn event(&self, dialog: Hwnd, event: Event<'_>) -> Reply {
        match event {
            Event::InitDialog => {
                let dpi = dialog.dpi();
                let columns: Vec<Column> = COLUMNS
                    .iter()
                    .map(|(title, width, right)| {
                        let align = if *right { Align::Right } else { Align::Left };
                        Column::new(title, scale(*width, dpi), align)
                    })
                    .collect();
                Self::list(dialog).set_columns(&columns);
                self.show(dialog, None);
                watch(&self.manager, dialog);
                Reply::Default
            }
            Event::Timer { id } if watched(&self.manager, dialog, id, "routes") => Reply::Handled,
            Event::ListSelection { id: ID_LIST } => {
                self.show_buttons(dialog);
                Reply::Handled
            }
            Event::Command { id: ID_ADD, .. } => {
                self.edit(dialog, None);
                Reply::Handled
            }
            Event::Command { id: ID_EDIT, .. } | Event::ListActivate { id: ID_LIST, .. } => {
                if let Some(route) = self.selected(dialog) {
                    self.edit(dialog, Some(route));
                }
                Reply::Handled
            }
            Event::Command { id: ID_REMOVE, .. } => {
                if let Some(route) = self.selected(dialog) {
                    let left = self.routes.borrow().len().saturating_sub(1);
                    let then = left
                        .checked_sub(1)
                        .map(|last| (route.index as usize).min(last));
                    let call = Call::RoutesRemove(RouteRef { index: route.index });
                    self.ask(dialog, call, then);
                }
                Reply::Handled
            }
            Event::Command { id: ID_UP, .. } => {
                self.shift(dialog, true);
                Reply::Handled
            }
            Event::Command { id: ID_DOWN, .. } => {
                self.shift(dialog, false);
                Reply::Handled
            }
            Event::ListCell {
                id: ID_LIST,
                row: index,
                column,
                cell,
            } => {
                if let Ok(routes) = self.routes.try_borrow()
                    && let Some(route) = routes.get(index)
                    && let Some(text) = row(route).get(column)
                {
                    cell.text = text.clone();
                }
                Reply::Handled
            }
            _ => Reply::Default,
        }
    }
}

/// Shows the routes, to add to, change, order and take from.
pub fn show(manager: &Rc<Manager>, owner: Hwnd) {
    // Asked for before the dialog opens, so that it opens filled in.
    let routes = match manager.call::<RouteList>(owner, Call::RoutesList) {
        Ok(list) => list.routes,
        Err(error) => {
            manager.show_error(owner, &error);
            return;
        }
    };
    let handler = Rc::new(Routes {
        manager: Rc::clone(manager),
        routes: RefCell::new(routes),
    });
    template().show_modal(Some(owner), handler);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route() -> RouteInfo {
        RouteInfo {
            index: 0,
            called: "555*".to_string(),
            target: "group:dialup".to_string(),
        }
    }

    #[test]
    fn a_row_numbers_the_route_from_one_and_says_what_it_rings() {
        assert_eq!(row(&route()), ["1", "555*", "Hunt group dialup"]);
        let line = RouteInfo {
            index: 4,
            called: "5551234".to_string(),
            target: "line:3".to_string(),
        };
        assert_eq!(row(&line), ["5", "5551234", "Line 3"]);
        assert_eq!(target_text("somewhere"), "somewhere");
        assert_eq!(COLUMNS.len(), row(&line).len());
    }

    #[test]
    fn the_editor_offers_every_line_and_then_every_group() {
        assert_eq!(
            targets(&[1, 2], &["dialup".to_string(), "night".to_string()]),
            ["line:1", "line:2", "group:dialup", "group:night"]
        );
        assert!(targets(&[], &[]).is_empty());
    }

    #[test]
    fn a_new_route_needs_a_number_and_something_to_ring() {
        let new = spec(" 555* ", Some("line:2")).unwrap();
        assert_eq!(new.called, "555*");
        assert_eq!(new.target, "line:2");
        assert_eq!(new.index, None);
        assert_eq!(
            spec("  ", Some("line:2")).unwrap_err().text,
            "Called number must not be empty."
        );
        assert_eq!(
            spec("555*", None).unwrap_err().text,
            "Rings must not be empty."
        );
    }

    #[test]
    fn only_what_was_changed_of_a_route_is_sent() {
        let route = route();
        assert_eq!(change(&route, "555*", Some("group:dialup")), Ok(None));
        let number = change(&route, "556*", Some("group:dialup"))
            .unwrap()
            .unwrap();
        assert_eq!(number.index, 0);
        assert_eq!(number.called.as_deref(), Some("556*"));
        assert_eq!(number.target, None);
        let target = change(&route, "555*", Some("line:1")).unwrap().unwrap();
        assert_eq!(target.called, None);
        assert_eq!(target.target.as_deref(), Some("line:1"));
        assert!(change(&route, "", Some("line:1")).is_err());
    }
}
