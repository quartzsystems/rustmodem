// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! One window that shows every wrapped control once.
//!
//! ```text
//! cargo run -p win32 --example sampler
//! ```
//!
//! This is the first thing to put beside a stock Windows program: every
//! font, height, colour and behaviour here should be the system's. The
//! File menu and the buttons of the "Dialogs" group open each kind of
//! dialog.
//!
//! With `--capture FILE.png` the window is shown, rendered to the file and
//! closed. `--capture-dialog FILE.png` and `--capture-sheet FILE.png` do
//! the same for the dialog made from a template and for the property
//! sheet.

#[cfg(not(windows))]
fn main() {
    eprintln!("The sampler runs on Windows only.");
}

#[cfg(windows)]
fn main() {
    sampler::run();
}

#[cfg(windows)]
mod sampler {
    use std::cell::RefCell;
    use std::rc::Rc;

    use glyph::Glyph;
    use win32::controls::{self, Combo, Progress, Tabs, UpDown};
    use win32::layout::{Placement, Track, columns, rows};
    use win32::metrics::{self, scale};
    use win32::{
        Accelerators, Align, App, Colour, Column, DialogTemplate, Event, FileFilter, Font, Handler,
        Hwnd, ImageList, Key, ListView, Menu, MenuHighlight, MenuItem, ModelessDialog, Rect, Reply,
        RichEdit, Sort, StatusBar, TaskButtons, TaskDialog, TaskIcon, ToolButton, Toolbar, Window,
        WindowOptions, capture, code, id, open_file, pick_folder, property_sheet, save_file,
        task_dialog,
    };

    const ID_EXIT: u16 = 100;
    const ID_REFRESH: u16 = 101;
    const ID_CHECKED: u16 = 102;
    const ID_FIRST: u16 = 110;
    const ID_SECOND: u16 = 111;
    const ID_THIRD: u16 = 112;
    const ID_TOOL_ANSWER: u16 = 120;
    const ID_TOOL_HANG_UP: u16 = 121;
    const ID_TOOL_SETTINGS: u16 = 122;
    const ID_LIST: u16 = 200;
    const ID_EDIT: u16 = 201;
    const ID_COMBO: u16 = 202;
    const ID_EDITABLE: u16 = 203;
    const ID_NUMBER: u16 = 204;
    const ID_BUTTON: u16 = 205;
    const ID_DEFAULT: u16 = 206;
    const ID_CHECK: u16 = 207;
    const ID_RADIO_ONE: u16 = 208;
    const ID_RADIO_TWO: u16 = 209;
    const ID_TABS: u16 = 210;
    const ID_LOG: u16 = 211;
    const ID_STATUS: u16 = 212;
    const ID_TOOLBAR: u16 = 213;
    const ID_FILTER: u16 = 214;

    const ID_DIALOG: u16 = 130;
    const ID_SHEET: u16 = 131;
    const ID_TASK: u16 = 132;
    const ID_OPEN: u16 = 133;
    const ID_SAVE: u16 = 134;
    const ID_FOLDER: u16 = 135;
    const ID_MODELESS: u16 = 136;

    const TIMER: usize = 1;

    /// The rows of the list: a number, a name, a state and a count.
    const ROWS: [(&str, &str, &str, &str); 6] = [
        ("1", "CNCB0", "Online", "15,200"),
        ("2", "CNCB1", "Online", "56,400"),
        ("3", "CNCB2", "Handshaking", "1,200"),
        ("4", "CNCB3", "Ringing", "1,300"),
        ("5", "CNCB4", "Idle", "48,300"),
        ("6", "CNCB5", "Port error", "0"),
    ];

    /// Everything the window holds once it has been created.
    struct Ui {
        dpi: u32,
        font: Font,
        log_font: Font,
        tool_images: ImageList,
        state_images: ImageList,
        toolbar: Toolbar,
        filter_label: Hwnd,
        filter: Combo,
        list: ListView,
        buttons_group: Hwnd,
        button: Hwnd,
        default_button: Hwnd,
        check: Hwnd,
        radio_one: Hwnd,
        radio_two: Hwnd,
        fields_group: Hwnd,
        edit_label: Hwnd,
        edit: Hwnd,
        combo_label: Hwnd,
        combo: Combo,
        editable_label: Hwnd,
        editable: Combo,
        number_label: Hwnd,
        number: Hwnd,
        up_down: UpDown,
        progress_label: Hwnd,
        progress: Progress,
        percent: Hwnd,
        tabs: Tabs,
        tab_text: Hwnd,
        log_group: Hwnd,
        log: RichEdit,
        status: StatusBar,
        ticks: u32,
        sort: (usize, Sort),
    }

    struct Sampler {
        ui: RefCell<Option<Ui>>,
        /// The modeless dialog, while it is open.
        modeless: RefCell<Option<ModelessDialog>>,
    }

    fn images(glyphs: &[Glyph], dpi: u32) -> ImageList {
        let size = scale(16, dpi);
        let mut list = ImageList::new(size);
        for glyph in glyphs {
            list.add(&glyph::rgba(*glyph, size as u32));
        }
        list
    }

    fn menu() -> Menu {
        Menu::bar(&[
            MenuItem::submenu(
                "&File",
                vec![
                    MenuItem::command(ID_REFRESH, "&Refresh\tF5"),
                    MenuItem::command(ID_CHECKED, "&Checked item"),
                    MenuItem::Separator,
                    MenuItem::command(ID_EXIT, "E&xit\tAlt+F4"),
                ],
            ),
            MenuItem::submenu(
                "&Dialogs",
                vec![
                    MenuItem::command(ID_DIALOG, "&Dialog from a template…"),
                    MenuItem::command(ID_MODELESS, "&Modeless dialog…"),
                    MenuItem::command(ID_SHEET, "&Property sheet…"),
                    MenuItem::command(ID_TASK, "&Task dialog…"),
                    MenuItem::Separator,
                    MenuItem::command(ID_OPEN, "&Open file…"),
                    MenuItem::command(ID_SAVE, "&Save file…"),
                    MenuItem::command(ID_FOLDER, "Pick &folder…"),
                ],
            ),
            MenuItem::submenu(
                "&View",
                vec![MenuItem::submenu(
                    "&Show",
                    vec![
                        MenuItem::command(ID_FIRST, "&First"),
                        MenuItem::command(ID_SECOND, "&Second"),
                        MenuItem::command(ID_THIRD, "&Third (disabled)"),
                    ],
                )],
            ),
        ])
    }

    impl Sampler {
        fn create(&self, window: Hwnd) {
            let dpi = window.dpi();
            let tool_images = images(&[Glyph::Answer, Glyph::HangUp, Glyph::Settings], dpi);
            let state_images = images(&Glyph::STATES, dpi);
            let toolbar = Toolbar::create(
                window,
                ID_TOOLBAR,
                &tool_images,
                &[
                    ToolButton::new(ID_TOOL_ANSWER, "Answer now", 0),
                    ToolButton::new(ID_TOOL_HANG_UP, "Hang up", 1),
                    ToolButton::Separator,
                    ToolButton::new(ID_TOOL_SETTINGS, "Settings…", 2),
                    ToolButton::Separator,
                ],
            );
            toolbar.set_enabled(ID_TOOL_HANG_UP, false);
            let filter_label = controls::label(window, "&Show:");
            let filter = Combo::drop_list(window, ID_FILTER);
            for entry in ["All lines", "On a call", "Idle"] {
                filter.add(entry);
            }
            filter.select(Some(0));

            let list = ListView::create(window, ID_LIST);
            list.set_columns(&[
                Column::new("#", scale(40, dpi), Align::Right),
                Column::new("COM port", scale(90, dpi), Align::Left),
                Column::new("State", scale(120, dpi), Align::Left),
                Column::new("Bytes", scale(90, dpi), Align::Right),
            ]);
            list.set_images(&state_images);
            list.set_row_count(ROWS.len());
            list.set_sort_arrow(Some((0, Sort::Ascending)));
            list.select(Some(1));

            let buttons_group = controls::group_box(window, "Buttons");
            let button = controls::button(window, ID_BUTTON, "&Button", false);
            let default_button = controls::button(window, ID_DEFAULT, "&Default", true);
            let check = controls::check_box(window, ID_CHECK, "&Check box");
            controls::set_checked(check, true);
            let radio_one = controls::radio_button(window, ID_RADIO_ONE, "Radio &one", true);
            let radio_two = controls::radio_button(window, ID_RADIO_TWO, "Radio &two", false);
            controls::set_checked(radio_one, true);

            let fields_group = controls::group_box(window, "Fields");
            let edit_label = controls::label(window, "&Edit:");
            let edit = controls::edit(window, ID_EDIT, false);
            edit.set_text("Line 1");
            let combo_label = controls::label(window, "Drop &list:");
            let combo = Combo::drop_list(window, ID_COMBO);
            let editable_label = controls::label(window, "Co&mbo:");
            let editable = Combo::editable(window, ID_EDITABLE);
            for entry in ["CNCB0", "CNCB1", "CNCB2"] {
                combo.add(entry);
                editable.add(entry);
            }
            combo.select(Some(1));
            editable.set_text("CNCB7");
            let number_label = controls::label(window, "&Number:");
            let number = controls::edit(window, ID_NUMBER, true);
            let up_down = UpDown::create(window, number, 0, 255);
            number.set_text("2");
            let progress_label = controls::label(window, "Progress:");
            let progress = Progress::create(window);
            progress.set_percent(60);
            let percent = controls::value(window, "60%", true);

            let tabs = Tabs::create(window, ID_TABS);
            tabs.add("Service");
            tabs.add("Control");
            let tab_text = controls::value(window, "The Service page.", false);

            let log_group = controls::group_box(window, "Log — rustmodem.log");
            let log = RichEdit::create(window, ID_LOG);
            let status = StatusBar::create(window, ID_STATUS);

            let mut ui = Ui {
                dpi,
                font: Font::message(dpi),
                log_font: Font::log(dpi),
                tool_images,
                state_images,
                toolbar,
                filter_label,
                filter,
                list,
                buttons_group,
                button,
                default_button,
                check,
                radio_one,
                radio_two,
                fields_group,
                edit_label,
                edit,
                combo_label,
                combo,
                editable_label,
                editable,
                number_label,
                number,
                up_down,
                progress_label,
                progress,
                percent,
                tabs,
                tab_text,
                log_group,
                log,
                status,
                ticks: 0,
                sort: (0, Sort::Ascending),
            };
            ui.apply_fonts(window);
            // Laid out before anything is put in the panes, so that the log
            // wraps to the width it will have.
            ui.layout(window);
            ui.log.append(
                &[
                    ("08:00:00 INFO  [SVC] Service started.".to_string(), None),
                    (
                        "08:00:01 INFO  [Line01] Line 1 state is now idle.".to_string(),
                        Some(metrics::chosen(Colour::rgb(0, 102, 0))),
                    ),
                    (
                        "08:00:02 ERROR [Line06] Serial port CNCB5 could not be opened."
                            .to_string(),
                        Some(metrics::chosen(Colour::rgb(192, 0, 0))),
                    ),
                    (
                        "08:00:03 INFO  [CTL] Control client 1 connected.".to_string(),
                        Some(metrics::chosen(Colour::rgb(0, 70, 160))),
                    ),
                ],
                100,
            );
            ui.status.set_text(0, "Ready");
            ui.status.set_text(1, "Service: connected");
            ui.status.set_text(2, "08:00");
            ui.status.set_text(3, "");
            if let Some(menu) = Menu::of_window(window) {
                menu.set_checked(ID_CHECKED, true);
                menu.set_radio(ID_FIRST, ID_THIRD, ID_FIRST);
                menu.set_enabled(ID_THIRD, false);
            }
            window.set_timer(TIMER, 1000);
            *self.ui.borrow_mut() = Some(ui);
        }
    }

    impl Ui {
        fn apply_fonts(&mut self, window: Hwnd) {
            window.set_font_all(&self.font);
            self.log.hwnd().set_font(&self.log_font);
        }

        fn layout(&self, window: Hwnd) {
            let dpi = self.dpi;
            self.status.place();
            let mut area = window.client_rect();
            area.height -= self.status.height();
            let (tool_width, tool_height) = self.toolbar.size();
            let bands = rows(
                area,
                &[
                    Track::Pixels(tool_height + scale(4, dpi)),
                    Track::Fill(1),
                    Track::Fixed(150),
                    Track::Fixed(110),
                ],
                0,
                dpi,
            );
            let mut placement = Placement::new();

            // The toolbar, and after its buttons the label and the list.
            let bar = bands[0].inset_by(0, scale(2, dpi), 0, scale(2, dpi));
            placement.place(
                self.toolbar.hwnd(),
                Rect::new(bar.x, bar.y, tool_width, tool_height),
            );
            let after = bar.x + tool_width + scale(6, dpi);
            let label_width = scale(36, dpi);
            let row_height = self.font.height() + scale(8, dpi);
            let middle = bar.y + (bar.height - row_height) / 2;
            placement.place(
                self.filter_label,
                Rect::new(after, middle + scale(4, dpi), label_width, row_height),
            );
            placement.place(
                self.filter.hwnd(),
                // The height of a combo box is that of its dropped list.
                Rect::new(
                    after + label_width,
                    middle,
                    scale(120, dpi),
                    scale(200, dpi),
                ),
            );

            placement.place(self.list.hwnd(), bands[1].inset(scale(7, dpi)));

            let margin = scale(7, dpi);
            let panels = columns(
                bands[2].inset_by(margin, 0, margin, 0),
                &[Track::Fixed(150), Track::Fixed(260), Track::Fill(1)],
                7,
                dpi,
            );
            // Inside a group box: below its title, and in from its frame.
            let inside = |group: Rect| {
                group.inset_by(
                    scale(10, dpi),
                    self.font.height() + scale(8, dpi),
                    scale(10, dpi),
                    scale(8, dpi),
                )
            };
            let line = Track::Pixels(row_height);

            placement.place(self.buttons_group, panels[0]);
            let cells = rows(inside(panels[0]), &[line; 5], 3, dpi);
            for (control, cell) in [
                self.button,
                self.default_button,
                self.check,
                self.radio_one,
                self.radio_two,
            ]
            .into_iter()
            .zip(cells)
            {
                placement.place(control, cell);
            }

            placement.place(self.fields_group, panels[1]);
            let cells = rows(inside(panels[1]), &[line; 5], 3, dpi);
            let pair = |cell: Rect| columns(cell, &[Track::Fixed(70), Track::Fill(1)], 4, dpi);
            let text = |cell: Rect| cell.inset_by(0, scale(4, dpi), 0, 0);
            let fields = [
                (self.edit_label, self.edit),
                (self.combo_label, self.combo.hwnd()),
                (self.editable_label, self.editable.hwnd()),
                (self.number_label, self.number),
            ];
            for ((label, field), cell) in fields.into_iter().zip(&cells) {
                let parts = pair(*cell);
                placement.place(label, text(parts[0]));
                let mut rect = parts[1];
                if field == self.combo.hwnd() || field == self.editable.hwnd() {
                    rect.height = scale(200, dpi);
                } else if field == self.number {
                    rect.width = scale(60, dpi);
                }
                placement.place(field, rect);
            }
            let parts = columns(
                cells[4],
                &[Track::Fixed(70), Track::Fill(1), Track::Fixed(36)],
                4,
                dpi,
            );
            placement.place(self.progress_label, text(parts[0]));
            placement.place(
                self.progress.hwnd(),
                parts[1].inset_by(0, scale(4, dpi), 0, scale(4, dpi)),
            );
            placement.place(self.percent, text(parts[2]));

            let tab_rect = panels[2].inset_by(0, scale(6, dpi), 0, 0);
            placement.place(self.tabs.hwnd(), tab_rect);
            placement.place(
                self.tab_text,
                self.tabs.page_rect(tab_rect).inset(scale(8, dpi)),
            );

            let log = bands[3].inset_by(margin, 0, margin, margin);
            placement.place(self.log_group, log);
            placement.place(self.log.hwnd(), inside(log));
            placement.apply();

            // The up-down control places itself inside its edit control.
            self.up_down.attach(self.number, 0, 255);
            let width = window.client_rect().width;
            // The last part is the room the size grip takes.
            self.status.set_parts(&[
                width - scale(250, dpi),
                width - scale(80, dpi),
                width - scale(20, dpi),
                -1,
            ]);
        }
    }

    impl Handler for Sampler {
        fn event(&self, window: Hwnd, event: Event<'_>) -> Reply {
            match event {
                Event::Create => {
                    self.create(window);
                    Reply::Handled
                }
                Event::Destroy => {
                    App::quit(0);
                    Reply::Handled
                }
                Event::Size { minimized, .. } => {
                    if !minimized && let Some(ui) = self.ui.borrow().as_ref() {
                        ui.layout(window);
                    }
                    Reply::Handled
                }
                Event::DpiChanged { dpi } => {
                    if let Some(ui) = self.ui.borrow_mut().as_mut() {
                        ui.dpi = dpi;
                        ui.font = Font::message(dpi);
                        ui.log_font = Font::log(dpi);
                        ui.apply_fonts(window);
                        ui.tool_images =
                            images(&[Glyph::Answer, Glyph::HangUp, Glyph::Settings], dpi);
                        ui.state_images = images(&Glyph::STATES, dpi);
                        ui.toolbar.set_images(&ui.tool_images);
                        ui.list.set_images(&ui.state_images);
                        ui.layout(window);
                    }
                    Reply::Handled
                }
                Event::MinSize(size) => {
                    let dpi = window.dpi();
                    *size = (scale(620, dpi), scale(520, dpi));
                    Reply::Handled
                }
                Event::Timer { id: TIMER } => {
                    if let Some(ui) = self.ui.borrow_mut().as_mut() {
                        ui.ticks += 1;
                        let percent = (60 + ui.ticks * 5) % 101;
                        ui.progress.set_percent(percent);
                        ui.percent.set_text_if_changed(&format!("{percent}%"));
                    }
                    Reply::Handled
                }
                Event::Command { id: ID_EXIT, .. } => {
                    window.close();
                    Reply::Handled
                }
                // Nothing of the window's state is borrowed while a dialog
                // is up: the window goes on getting events behind it.
                Event::Command { id: ID_DIALOG, .. } => {
                    let result = dialog_template().show_modal(Some(window), Rc::new(Fields));
                    self.say(&format!("The dialog ended with {result}."));
                    Reply::Handled
                }
                Event::Command {
                    id: ID_MODELESS, ..
                } => {
                    let dialog = dialog_template().create_modeless(Some(window), Rc::new(Fields));
                    if let Some(dialog) = &dialog {
                        dialog.hwnd().set_visible(true);
                    }
                    *self.modeless.borrow_mut() = dialog;
                    Reply::Handled
                }
                Event::Command { id: ID_SHEET, .. } => {
                    let accepted = property_sheet(Some(window), "Settings", sheet_pages());
                    self.say(&format!("The property sheet was accepted: {accepted}."));
                    Reply::Handled
                }
                Event::Command { id: ID_TASK, .. } => {
                    let pressed = task_dialog(Some(window), &sample_task_dialog());
                    self.say(&format!("The task dialog ended with {pressed}."));
                    Reply::Handled
                }
                Event::Command { id: ID_OPEN, .. } => {
                    let filters = [
                        FileFilter::new("Configuration files", "*.toml"),
                        FileFilter::new("All files", "*.*"),
                    ];
                    let chosen = open_file(Some(window), "Open configuration", &filters);
                    self.say(&format!("Open: {chosen:?}"));
                    Reply::Handled
                }
                Event::Command { id: ID_SAVE, .. } => {
                    let filters = [FileFilter::new("CSV files", "*.csv")];
                    let chosen = save_file(
                        Some(window),
                        "Export statistics",
                        "statistics.csv",
                        "csv",
                        &filters,
                    );
                    self.say(&format!("Save: {chosen:?}"));
                    Reply::Handled
                }
                Event::Command { id: ID_FOLDER, .. } => {
                    let chosen = pick_folder(Some(window), "Log directory");
                    self.say(&format!("Folder: {chosen:?}"));
                    Reply::Handled
                }
                Event::Command {
                    id: ID_FIRST | ID_SECOND,
                    ..
                } => Reply::Handled,
                Event::Command {
                    id: ID_BUTTON | ID_DEFAULT | ID_TOOL_ANSWER | ID_TOOL_SETTINGS | ID_REFRESH,
                    code: code::CLICKED | code::ACCELERATOR,
                } => {
                    if let Some(ui) = self.ui.borrow().as_ref() {
                        let count = ui.log.line_count();
                        ui.log.append(
                            &[(
                                format!("08:00:{count:02} INFO  [SVC] A command was chosen."),
                                None,
                            )],
                            100,
                        );
                    }
                    Reply::Handled
                }
                Event::TabChanged { id: ID_TABS } => {
                    if let Some(ui) = self.ui.borrow().as_ref() {
                        let page = match ui.tabs.selected() {
                            Some(1) => "The Control page.",
                            _ => "The Service page.",
                        };
                        ui.tab_text.set_text(page);
                    }
                    Reply::Handled
                }
                Event::MenuSelect(highlight) => {
                    if let Some(ui) = self.ui.borrow().as_ref() {
                        match highlight {
                            MenuHighlight::Item(ID_EXIT) => {
                                ui.status.set_simple(Some("Closes the sampler."));
                            }
                            MenuHighlight::Item(ID_THIRD) => ui.status.set_simple(Some(
                                "Not available: this is what a reason looks like.",
                            )),
                            MenuHighlight::Item(_) => {
                                ui.status.set_simple(Some("A menu command."));
                            }
                            MenuHighlight::Other => ui.status.set_simple(Some("")),
                            MenuHighlight::Closed => ui.status.set_simple(None),
                        }
                    }
                    Reply::Handled
                }
                Event::ToolTip { id, text } => {
                    *text = match id {
                        ID_TOOL_ANSWER => "Answer the call ringing on the selected line",
                        ID_TOOL_HANG_UP => "Hang up the call on the selected line",
                        _ => "Change the service's settings",
                    }
                    .to_string();
                    Reply::Handled
                }
                Event::ListCell {
                    row, column, cell, ..
                } => {
                    let Some(ui) = self.ui.try_borrow().ok() else {
                        return Reply::Default;
                    };
                    let descending = ui.as_ref().is_some_and(|ui| ui.sort.1 == Sort::Descending);
                    let index = if descending {
                        ROWS.len() - 1 - row
                    } else {
                        row
                    };
                    if let Some(shown) = ROWS.get(index) {
                        cell.text = [shown.0, shown.1, shown.2, shown.3][column.min(3)].to_string();
                        cell.image = (column == 0).then_some(match shown.2 {
                            "Online" => 5,
                            "Handshaking" => 4,
                            "Ringing" => 3,
                            "Idle" => 2,
                            _ => 6,
                        });
                    }
                    Reply::Handled
                }
                Event::ListCellColour {
                    row,
                    column: 2,
                    colour,
                    ..
                } => {
                    let Some(ui) = self.ui.try_borrow().ok() else {
                        return Reply::Default;
                    };
                    let descending = ui.as_ref().is_some_and(|ui| ui.sort.1 == Sort::Descending);
                    let index = if descending {
                        ROWS.len() - 1 - row
                    } else {
                        row
                    };
                    *colour = match ROWS.get(index).map(|shown| shown.2) {
                        Some("Online") => Some(metrics::chosen(Colour::rgb(0, 128, 0))),
                        Some("Port error") => Some(metrics::chosen(Colour::rgb(192, 0, 0))),
                        _ => None,
                    };
                    Reply::Handled
                }
                Event::ListColumnClick { column, .. } => {
                    if let Some(ui) = self.ui.borrow_mut().as_mut() {
                        let direction = match ui.sort {
                            (sorted, Sort::Ascending) if sorted == column => Sort::Descending,
                            _ => Sort::Ascending,
                        };
                        ui.sort = (column, direction);
                        ui.list.set_sort_arrow(Some(ui.sort));
                        ui.list.redraw_rows(0, ROWS.len());
                    }
                    Reply::Handled
                }
                Event::ContextMenu { source, position } => {
                    let is_list = self
                        .ui
                        .borrow()
                        .as_ref()
                        .is_some_and(|ui| ui.list.hwnd() == source);
                    if !is_list {
                        return Reply::Default;
                    }
                    let popup = Menu::popup(&[
                        MenuItem::command(ID_TOOL_ANSWER, "&Answer now"),
                        MenuItem::command(ID_TOOL_HANG_UP, "&Hang up"),
                    ]);
                    popup.set_enabled(ID_TOOL_HANG_UP, false);
                    popup.show_popup(window, position);
                    Reply::Handled
                }
                _ => Reply::Default,
            }
        }
    }

    impl Sampler {
        /// Adds a line to the log pane.
        fn say(&self, text: &str) {
            if let Some(ui) = self.ui.borrow().as_ref() {
                ui.log
                    .append(&[(format!("08:00:59 INFO  [SVC] {text}"), None)], 100);
            }
        }
    }

    const FIELD_NAME: u16 = 300;
    const FIELD_PORT: u16 = 301;
    const FIELD_RINGS: u16 = 302;
    const FIELD_ARROWS: u16 = 303;
    const FIELD_BACKEND: u16 = 304;
    const FIELD_ENABLED: u16 = 305;
    const FIELD_OK: u16 = 306;
    const FIELD_ERROR: u16 = 307;
    const FIELD_PICTURE: u16 = 308;

    /// A dialog with one of each control a dialog of the manager has.
    fn dialog_template() -> DialogTemplate {
        DialogTemplate::new("Edit line", 230, 150)
            .label(7, 9, 70, 8, "&Name:")
            .edit(FIELD_NAME, 80, 7, 143, 14)
            .label(7, 27, 70, 8, "Serial &backend:")
            .combo(FIELD_BACKEND, 80, 25, 80, 60)
            .label(7, 45, 70, 8, "&Port:")
            .combo_editable(FIELD_PORT, 80, 43, 143, 80)
            .label(7, 63, 70, 8, "&Auto-answer rings:")
            .number(FIELD_RINGS, 80, 61, 40, 14)
            .up_down(FIELD_ARROWS)
            .check_box(FIELD_ENABLED, 80, 80, 143, 10, "&Enabled")
            .group(7, 94, 216, 28, "Unknown commands")
            .radio_button(FIELD_OK, 14, 106, 60, 10, "Answer &OK", true)
            .radio_button(FIELD_ERROR, 80, 106, 70, 10, "Answer E&RROR", false)
            .ok_cancel()
    }

    /// Fills in the dialog, and takes a picture of it when asked to.
    struct Fields;

    impl Handler for Fields {
        fn event(&self, dialog: Hwnd, event: Event<'_>) -> Reply {
            match event {
                Event::InitDialog => {
                    dialog.item(FIELD_NAME).set_text("Front desk");
                    let backend = Combo::of(dialog.item(FIELD_BACKEND));
                    backend.add("com0com");
                    backend.select(Some(0));
                    let port = Combo::of(dialog.item(FIELD_PORT));
                    for entry in ["CNCB0 (CNCA0, COM11)", "CNCB1 (CNCA1, COM12)"] {
                        port.add(entry);
                    }
                    port.set_text("CNCB0");
                    let rings = dialog.item(FIELD_RINGS);
                    rings.set_text("2");
                    UpDown::of(dialog.item(FIELD_ARROWS)).attach(rings, 0, 255);
                    controls::set_checked(dialog.item(FIELD_ENABLED), true);
                    controls::set_checked(dialog.item(FIELD_OK), true);
                    if capture_path("--capture-dialog").is_some() {
                        dialog.set_timer(TIMER, 300);
                    }
                    Reply::Default
                }
                Event::Timer { id: TIMER } => {
                    dialog.kill_timer(TIMER);
                    if let Some(path) = capture_path("--capture-dialog") {
                        write_capture(dialog, &path);
                        dialog.end_dialog(id::CANCEL);
                    }
                    Reply::Handled
                }
                _ => Reply::Default,
            }
        }
    }

    /// A page of the property sheet.
    struct Page {
        first: bool,
    }

    impl Handler for Page {
        fn event(&self, page: Hwnd, event: Event<'_>) -> Reply {
            match event {
                Event::InitDialog => {
                    if self.first {
                        let level = Combo::of(page.item(FIELD_BACKEND));
                        for entry in ["error", "warning", "info", "debug"] {
                            level.add(entry);
                        }
                        level.select(Some(2));
                        if capture_path("--capture-sheet").is_some() {
                            page.set_timer(TIMER, 300);
                        }
                    } else {
                        page.item(FIELD_NAME).set_text("rustmodem-control");
                    }
                    Reply::Default
                }
                Event::Command {
                    code: code::EDIT_CHANGE | code::COMBO_SELECTION,
                    ..
                } => {
                    // Something changed: Apply is enabled.
                    page.page_changed();
                    Reply::Handled
                }
                Event::Timer { id: TIMER } => {
                    page.kill_timer(TIMER);
                    if let (Some(path), Some(sheet)) =
                        (capture_path("--capture-sheet"), page.parent())
                    {
                        write_capture(sheet, &path);
                        sheet.command(id::CANCEL);
                    }
                    Reply::Handled
                }
                _ => Reply::Default,
            }
        }
    }

    fn sheet_pages() -> Vec<(String, DialogTemplate, Rc<dyn Handler>)> {
        let service = DialogTemplate::new("", 230, 110)
            .label(7, 9, 70, 8, "&Log level:")
            .combo(FIELD_BACKEND, 80, 7, 80, 70)
            .label(7, 27, 70, 8, "Log &directory:")
            .edit(FIELD_NAME, 80, 25, 100, 14)
            .button(FIELD_PICTURE, 184, 25, 40, 14, "B&rowse…");
        let control = DialogTemplate::new("", 230, 110)
            .label(7, 9, 70, 8, "&Pipe name:")
            .edit(FIELD_NAME, 80, 7, 143, 14);
        vec![
            (
                "Service".to_string(),
                service,
                Rc::new(Page { first: true }),
            ),
            (
                "Control".to_string(),
                control,
                Rc::new(Page { first: false }),
            ),
        ]
    }

    /// A message as the manager shows one: the id in the title, the text
    /// as the main instruction, the cause and the recovery to expand.
    fn sample_task_dialog() -> TaskDialog {
        TaskDialog {
            title: "RMD2008".to_string(),
            instruction: "Control pipe rustmodem-control could not be opened: Access is denied."
                .to_string(),
            content: String::new(),
            expanded: "Cause: The pipe is open to SYSTEM and Administrators only.\n\n\
                       Recovery: Start the manager again as an administrator."
                .to_string(),
            icon: TaskIcon::Error,
            buttons: TaskButtons::CANCEL,
            links: vec![(
                900,
                "Restart as administrator\nThe manager closes and opens again elevated."
                    .to_string(),
            )],
            default: Some(900),
        }
    }

    /// The path after a flag on the command line, if the flag is there.
    fn capture_path(flag: &str) -> Option<String> {
        let mut arguments = std::env::args().skip(1);
        while let Some(argument) = arguments.next() {
            if argument == flag {
                return arguments.next();
            }
        }
        None
    }

    fn write_capture(window: Hwnd, path: &str) {
        match capture(window) {
            Some(image) => {
                let png = glyph::png::encode(&image.rgba, image.width, image.height);
                std::fs::write(path, png).expect("the picture can be written");
                println!("{path}");
            }
            None => eprintln!("The window could not be captured."),
        }
    }

    pub fn run() {
        let app = App::new();
        let sampler = Rc::new(Sampler {
            ui: RefCell::new(None),
            modeless: RefCell::new(None),
        });
        let dpi = metrics::system_dpi();
        let Some(window) = Window::create(
            WindowOptions {
                title: "rustmodem control sampler",
                size: Some((scale(760, dpi), scale(620, dpi))),
                menu: Some(menu()),
                owner: None,
                dialog_keys: true,
            },
            sampler.clone(),
        ) else {
            eprintln!("The window could not be made.");
            return;
        };
        let accelerators = Accelerators::new(&[(Key::Function(5), ID_REFRESH)]);
        window.set_visible(true);

        // Let the window paint, and its themed parts settle.
        let settle = || {
            for _ in 0..20 {
                app.pump(window, Some(&accelerators));
                std::thread::sleep(std::time::Duration::from_millis(30));
            }
        };
        if let Some(path) = capture_path("--capture") {
            settle();
            write_capture(window, &path);
            window.destroy();
            return;
        }
        if capture_path("--capture-dialog").is_some() {
            settle();
            dialog_template().show_modal(Some(window), Rc::new(Fields));
            window.destroy();
            return;
        }
        if capture_path("--capture-sheet").is_some() {
            settle();
            property_sheet(Some(window), "Settings", sheet_pages());
            window.destroy();
            return;
        }
        app.run(window, Some(&accelerators));
    }
}
