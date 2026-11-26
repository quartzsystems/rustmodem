// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The controls of the main window: making them, placing them, and
//! putting into them what the model holds.
//!
//! Top to bottom the window is the toolbar, with the filter after its
//! buttons; the line grid in its group box; three group boxes side by
//! side, the two statistics panels and the event log; and the status bar.
//! Nothing here decides anything: every text, count and colour class
//! comes from the model, and the view only shows it.

use std::cell::{Cell, RefCell};

use glyph::Glyph;
use model::{
    CALL_LABELS, CARRIER_LABELS, Column, Command, Filter, LOG_LIMIT, LogClass, MenuEntry, Model,
    RATE_ROWS, RowChange, StateColour, TimeZone,
};
use win32::controls::{self, Combo, Progress};
use win32::layout::{Placement, Track, columns, rows};
use win32::metrics::{self, scale, unscale};
use win32::{
    Align, Colour, Font, Hwnd, ImageList, ListView, Menu, MenuItem, Rect, RichEdit, Sort,
    StatusBar, ToolButton, Toolbar,
};

use crate::clock::Clock;
use crate::settings::Saved;

/// The ids of the controls. Command ids start at 1000.
pub mod ids {
    /// The toolbar.
    pub const TOOLBAR: u16 = 100;
    /// The filter, after the toolbar's buttons.
    pub const FILTER: u16 = 101;
    /// The line grid.
    pub const LIST: u16 = 102;
    /// The event log.
    pub const LOG: u16 = 103;
    /// The status bar.
    pub const STATUS: u16 = 104;
}

/// The colours the manager chooses: those of the states of a line, which
/// are the colours of the state images, and of the levels of the event
/// log. A high-contrast theme takes them away again.
///
/// The state of a line that is online.
const GREEN: Colour = Colour::rgb(0, 128, 0);

/// The state of a line that is ringing or handshaking: a yellow dark
/// enough to be read on white.
const YELLOW: Colour = Colour::rgb(176, 124, 0);

/// The state of a line that is idle.
const BLUE: Colour = Colour::rgb(0, 84, 200);

/// A port error, and an error in the event log.
const RED: Colour = Colour::rgb(192, 0, 0);

/// A warning in the event log.
const AMBER: Colour = Colour::rgb(176, 96, 0);

/// The widths of the status bar's parts after the message, at 96 DPI:
/// the connection, SIP, how many lines answer, RTP, the processor, and
/// the clock with the room the size grip takes.
const STATUS_PARTS: [i32; 6] = [150, 64, 124, 64, 64, 66];

/// The widths of the two statistics panels, at 96 DPI.
const CALLS_PANEL: i32 = 264;
const CARRIER_PANEL: i32 = 290;

/// The narrowest the Binding column is made, at 96 DPI.
const LEAST_BINDING: i32 = 60;

/// The smallest the window may be, at 96 DPI.
const MIN_SIZE: (i32, i32) = (880, 560);

/// The size of the window when nothing is remembered, at 96 DPI.
pub const DEFAULT_SIZE: (i32, i32) = (1180, 740);

/// One connect rate: what was negotiated, the bar, and the share.
struct Rate {
    label: Hwnd,
    bar: Progress,
    percent: Hwnd,
}

/// The controls of the main window.
pub struct View {
    dpi: u32,
    font: Font,
    log_font: Font,
    tool_images: ImageList,
    state_images: ImageList,
    toolbar: Toolbar,
    filter_label: Hwnd,
    filter: Combo,
    lines_group: Hwnd,
    list: ListView,
    calls_group: Hwnd,
    calls: Vec<(Hwnd, Hwnd)>,
    carrier_group: Hwnd,
    rates: Vec<Rate>,
    carrier: Vec<(Hwnd, Hwnd)>,
    log_group: Hwnd,
    log: RichEdit,
    status: StatusBar,
    toolbar_shown: Cell<bool>,
    status_shown: Cell<bool>,
    /// The generation of the model's log the pane shows, and how many of
    /// its lines.
    log_shown: Cell<(u64, u64)>,
    /// How many connect rates are shown; `None` before the first time.
    rates_shown: Cell<Option<usize>>,
    /// Whether each command was last shown as one that can be chosen.
    available: RefCell<Vec<Option<bool>>>,
    /// The texts of the status bar's parts as last set.
    status_texts: RefCell<Vec<String>>,
}

fn images(glyphs: &[Glyph], dpi: u32) -> ImageList {
    let size = scale(16, dpi);
    let mut list = ImageList::new(size);
    for glyph in glyphs {
        list.add(&glyph::rgba(*glyph, size as u32));
    }
    list
}

fn menu_item(entry: &MenuEntry) -> MenuItem {
    let command = |command: &Command| MenuItem::command(command.id(), command.text());
    match entry {
        MenuEntry::Item(item) => command(item),
        MenuEntry::Separator => MenuItem::Separator,
        MenuEntry::Submenu(text, commands) => {
            MenuItem::submenu(text, commands.iter().map(command).collect())
        }
    }
}

/// The menu bar, as the model lists it.
pub fn menu_bar() -> Menu {
    let menus: Vec<MenuItem> = model::menu_bar()
        .iter()
        .map(|(title, entries)| MenuItem::submenu(title, entries.iter().map(menu_item).collect()))
        .collect();
    Menu::bar(&menus)
}

/// The Lines menu as the grid's context menu, with each command as the
/// model says it is now.
pub fn lines_popup(model: &Model) -> Menu {
    let entries = model::lines_menu();
    let items: Vec<MenuItem> = entries.iter().map(menu_item).collect();
    let menu = Menu::popup(&items);
    for entry in &entries {
        if let MenuEntry::Item(command) = entry {
            menu.set_enabled(command.id(), model.availability(*command).is_enabled());
        }
    }
    menu
}

/// The colour of a line of the event log, by its level: an error in red,
/// a warning in amber, what happened in the control's own colour, and
/// detail in the system's colour for what matters less.
fn log_colour(class: LogClass) -> Option<Colour> {
    match class {
        LogClass::Error => Some(metrics::chosen(RED)),
        LogClass::Warning => Some(metrics::chosen(AMBER)),
        LogClass::Info => None,
        LogClass::Debug => Some(metrics::gray_text()),
    }
}

/// The colour of the text of the State column.
pub fn state_colour(colour: StateColour) -> Option<Colour> {
    match colour {
        StateColour::Default => None,
        StateColour::Green => Some(metrics::chosen(GREEN)),
        StateColour::Yellow => Some(metrics::chosen(YELLOW)),
        StateColour::Blue => Some(metrics::chosen(BLUE)),
        StateColour::Red => Some(metrics::chosen(RED)),
    }
}

/// The images of the toolbar's buttons, in the order of the buttons.
fn tool_glyphs() -> Vec<Glyph> {
    model::toolbar()
        .into_iter()
        .flatten()
        .map(tool_glyph)
        .collect()
}

/// The image of a toolbar button.
fn tool_glyph(command: Command) -> Glyph {
    match command {
        Command::Answer => Glyph::Answer,
        Command::HangUp => Glyph::HangUp,
        Command::Reset => Glyph::Reset,
        Command::NewLine => Glyph::NewLine,
        Command::Reregister => Glyph::Reregister,
        Command::ReloadConfig => Glyph::Reload,
        _ => Glyph::Settings,
    }
}

impl View {
    /// Makes the controls in `window`, as `saved` remembers them.
    pub fn create(window: Hwnd, saved: &Saved) -> View {
        let dpi = window.dpi();
        let tool_images = images(&tool_glyphs(), dpi);
        let state_images = images(&Glyph::STATES, dpi);

        // The buttons take the images in order.
        let mut image = 0;
        let buttons: Vec<ToolButton> = model::toolbar()
            .into_iter()
            .map(|entry| match entry {
                Some(command) => {
                    let button = ToolButton::new(
                        command.id(),
                        command.tool_label().unwrap_or_default(),
                        image,
                    );
                    image += 1;
                    button
                }
                None => ToolButton::Separator,
            })
            .collect();
        let toolbar = Toolbar::create(window, ids::TOOLBAR, &tool_images, &buttons);
        // The mnemonic is not S, which opens the SIP menu.
        let filter_label = controls::label(window, "Sho&w:");
        let filter = Combo::drop_list(window, ids::FILTER);
        for entry in Filter::ALL {
            filter.add(entry.title());
        }
        filter.select(Some(saved.filter.index()));

        let lines_group = controls::group_box(window, "Modem lines");
        let list = ListView::create(window, ids::LIST);
        let list_columns: Vec<win32::Column> = Column::ALL
            .iter()
            .zip(&saved.widths)
            .map(|(column, width)| {
                let align = if column.right_aligned() {
                    Align::Right
                } else {
                    Align::Left
                };
                win32::Column::new(column.title(), scale(*width, dpi), align)
            })
            .collect();
        list.set_columns(&list_columns);
        list.set_images(&state_images);

        let pair = |label: &str| {
            (
                controls::value(window, label, false),
                controls::value(window, model::EMPTY, true),
            )
        };
        let calls_group = controls::group_box(window, "Inbound call statistics");
        let calls = CALL_LABELS.iter().map(|label| pair(label)).collect();
        let carrier_group = controls::group_box(window, "Negotiation and carrier");
        let rates = (0..RATE_ROWS)
            .map(|_| Rate {
                label: controls::value(window, "", false),
                bar: Progress::create(window),
                percent: controls::value(window, "", true),
            })
            .collect();
        let carrier = CARRIER_LABELS.iter().map(|label| pair(label)).collect();

        let log_group = controls::group_box(window, "Event log");
        let log = RichEdit::create(window, ids::LOG);
        let status = StatusBar::create(window, ids::STATUS);

        let view = View {
            dpi,
            font: Font::message(dpi),
            log_font: Font::log(dpi),
            tool_images,
            state_images,
            toolbar,
            filter_label,
            filter,
            lines_group,
            list,
            calls_group,
            calls,
            carrier_group,
            rates,
            carrier,
            log_group,
            log,
            status,
            toolbar_shown: Cell::new(true),
            status_shown: Cell::new(true),
            log_shown: Cell::new((0, 0)),
            rates_shown: Cell::new(None),
            available: RefCell::new(vec![None; Command::ALL.len()]),
            status_texts: RefCell::new(Vec::new()),
        };
        view.apply_fonts(window);
        view.set_toolbar_shown(saved.toolbar);
        view.set_status_shown(saved.status_bar);
        view.layout(window);
        view
    }

    fn apply_fonts(&self, window: Hwnd) {
        window.set_font_all(&self.font);
        self.log.hwnd().set_font(&self.log_font);
    }

    /// The window moved to a monitor of another scaling: the fonts, the
    /// images and every size are made again for it.
    pub fn dpi_changed(&mut self, window: Hwnd, dpi: u32) {
        let old = self.dpi;
        self.dpi = dpi;
        self.font = Font::message(dpi);
        self.log_font = Font::log(dpi);
        self.apply_fonts(window);
        self.tool_images = images(&tool_glyphs(), dpi);
        self.state_images = images(&Glyph::STATES, dpi);
        self.toolbar.set_images(&self.tool_images);
        self.list.set_images(&self.state_images);
        for column in 0..Column::ALL.len() {
            let width = unscale(self.list.column_width(column), old);
            self.list.set_column_width(column, scale(width, dpi));
        }
        self.layout(window);
    }

    /// The smallest the window may be, in pixels.
    pub fn min_size(dpi: u32) -> (i32, i32) {
        (scale(MIN_SIZE.0, dpi), scale(MIN_SIZE.1, dpi))
    }

    /// The grid.
    pub fn list(&self) -> ListView {
        self.list
    }

    /// The status bar.
    pub fn status(&self) -> StatusBar {
        self.status
    }

    /// The filter the combo box of the toolbar shows.
    pub fn chosen_filter(&self) -> Option<Filter> {
        self.filter.selected().and_then(Filter::at)
    }

    /// Whether the toolbar is shown.
    pub fn toolbar_shown(&self) -> bool {
        self.toolbar_shown.get()
    }

    /// Whether the status bar is shown.
    pub fn status_shown(&self) -> bool {
        self.status_shown.get()
    }

    /// Shows or hides the toolbar and the filter beside it. The window is
    /// to be laid out again afterwards.
    pub fn set_toolbar_shown(&self, shown: bool) {
        self.toolbar_shown.set(shown);
        self.toolbar.hwnd().set_visible(shown);
        self.filter_label.set_visible(shown);
        self.filter.hwnd().set_visible(shown);
    }

    /// Shows or hides the status bar. The window is to be laid out again
    /// afterwards.
    pub fn set_status_shown(&self, shown: bool) {
        self.status_shown.set(shown);
        self.status.hwnd().set_visible(shown);
    }

    /// The widths of the grid's columns as they are now, in pixels at 96
    /// DPI: what is remembered.
    pub fn column_widths(&self) -> Vec<i32> {
        (0..Column::ALL.len())
            .map(|column| unscale(self.list.column_width(column), self.dpi))
            .collect()
    }

    // ----------------------------------------------------------------------
    // Layout
    // ----------------------------------------------------------------------

    /// The height of one row of a statistics panel.
    fn row_height(&self) -> i32 {
        self.font.height() + scale(3, self.dpi)
    }

    /// What a group box keeps for its title and its frame.
    fn inside(&self, group: Rect) -> Rect {
        let side = scale(9, self.dpi);
        group.inset_by(
            side,
            self.font.height() + scale(7, self.dpi),
            side,
            scale(9, self.dpi),
        )
    }

    /// The height of the bottom row: as many rows as the taller panel has,
    /// in a group box.
    fn panels_height(&self) -> i32 {
        let panel_rows = CALL_LABELS.len().max(RATE_ROWS + CARRIER_LABELS.len()) as i32;
        self.font.height() + scale(7 + 9, self.dpi) + panel_rows * self.row_height()
    }

    /// Places every control for the size the window has now.
    pub fn layout(&self, window: Hwnd) {
        let dpi = self.dpi;
        let margin = scale(7, dpi);
        let mut area = window.client_rect();
        if self.status_shown.get() {
            self.status.place();
            area.height -= self.status.height();
            self.place_status_parts(area.width);
        }
        let (tool_width, tool_height) = self.toolbar.size();
        let field_height = self.font.height() + scale(8, dpi);
        let tool_band = if self.toolbar_shown.get() {
            tool_height.max(field_height) + scale(6, dpi)
        } else {
            // Room above the grid's group box all the same.
            scale(4, dpi)
        };
        let bands = rows(
            area,
            &[
                Track::Pixels(tool_band),
                Track::Fill(1),
                Track::Pixels(self.panels_height() + margin),
            ],
            0,
            dpi,
        );
        let mut placement = Placement::new();

        // The toolbar, and after its buttons the label and the filter.
        if self.toolbar_shown.get() {
            let bar = bands[0];
            placement.place(
                self.toolbar.hwnd(),
                Rect::new(
                    bar.x,
                    bar.y + (bar.height - tool_height) / 2,
                    tool_width,
                    tool_height,
                ),
            );
            let after = bar.x + tool_width + scale(8, dpi);
            let label_width = scale(40, dpi);
            let top = bar.y + (bar.height - field_height) / 2;
            placement.place(
                self.filter_label,
                Rect::new(after, top + scale(4, dpi), label_width, self.font.height()),
            );
            placement.place(
                self.filter.hwnd(),
                // The height of a combo box is that of its dropped list.
                Rect::new(after + label_width, top, scale(170, dpi), scale(200, dpi)),
            );
        }

        // The grid fills its group box.
        let lines = bands[1].inset_by(margin, 0, margin, margin);
        placement.place(self.lines_group, lines);
        let grid = self.inside(lines);
        placement.place(self.list.hwnd(), grid);

        // The three panels. The event log takes the width that is left.
        let panels = columns(
            bands[2].inset_by(margin, 0, margin, margin),
            &[
                Track::Fixed(CALLS_PANEL),
                Track::Fixed(CARRIER_PANEL),
                Track::Fill(1),
            ],
            7,
            dpi,
        );
        let line = Track::Pixels(self.row_height());

        placement.place(self.calls_group, panels[0]);
        let cells = rows(
            self.inside(panels[0]),
            &vec![line; self.calls.len()],
            0,
            dpi,
        );
        for ((label, value), cell) in self.calls.iter().zip(cells) {
            let parts = columns(cell, &[Track::Fixed(128), Track::Fill(1)], 4, dpi);
            placement.place(*label, parts[0]);
            placement.place(*value, parts[1]);
        }

        placement.place(self.carrier_group, panels[1]);
        let cells = rows(
            self.inside(panels[1]),
            &vec![line; self.rates.len() + self.carrier.len()],
            0,
            dpi,
        );
        for (rate, cell) in self.rates.iter().zip(&cells) {
            let parts = columns(
                *cell,
                &[Track::Fixed(92), Track::Fill(1), Track::Fixed(38)],
                6,
                dpi,
            );
            placement.place(rate.label, parts[0]);
            placement.place(
                rate.bar.hwnd(),
                parts[1].inset_by(0, scale(2, dpi), 0, scale(3, dpi)),
            );
            placement.place(rate.percent, parts[2]);
        }
        for ((label, value), cell) in self.carrier.iter().zip(&cells[self.rates.len()..]) {
            let parts = columns(*cell, &[Track::Fixed(150), Track::Fill(1)], 4, dpi);
            placement.place(*label, parts[0]);
            placement.place(*value, parts[1]);
        }

        placement.place(self.log_group, panels[2]);
        placement.place(self.log.hwnd(), self.inside(panels[2]));
        placement.apply();

        self.fit_last_column(grid.width);
    }

    /// The Binding column takes the width the others leave. In a window
    /// too narrow to leave any it keeps a little, and the grid scrolls.
    fn fit_last_column(&self, grid_width: i32) {
        let last = Column::ALL.len() - 1;
        let others: i32 = (0..last).map(|column| self.list.column_width(column)).sum();
        // The frame of the control, and the scroll bar it may show.
        let room = grid_width - others - metrics::scroll_bar_width() - scale(4, self.dpi);
        let least = scale(LEAST_BINDING, self.dpi);
        let width = room.max(least);
        if self.list.column_width(last) != width {
            self.list.set_column_width(last, width);
        }
    }

    fn place_status_parts(&self, width: i32) {
        let total: i32 = STATUS_PARTS.iter().map(|part| scale(*part, self.dpi)).sum();
        let mut edge = (width - total).max(scale(60, self.dpi));
        let mut edges = Vec::with_capacity(STATUS_PARTS.len() + 1);
        for part in &STATUS_PARTS[..STATUS_PARTS.len() - 1] {
            edges.push(edge);
            edge += scale(*part, self.dpi);
        }
        edges.push(edge);
        // The last part runs to the end of the bar.
        edges.push(-1);
        self.status.set_parts(&edges);
        // New parts have lost their texts.
        let texts = std::mem::take(&mut *self.status_texts.borrow_mut());
        for (part, text) in texts.iter().enumerate() {
            self.set_status_text(part, text);
        }
    }

    // ----------------------------------------------------------------------
    // Showing what the model holds
    // ----------------------------------------------------------------------

    /// Brings the grid up to date after a change to its rows. The
    /// selection stays on its line and the list stays where it was
    /// scrolled to.
    pub fn show_rows(&self, model: &Model, change: RowChange) {
        match change {
            RowChange::None => {}
            RowChange::Changed(row) => self.list.redraw_row(row),
            RowChange::Added(_) | RowChange::Removed(_) | RowChange::Reset => {
                self.list.set_row_count(model.lines.row_count());
                // The control keeps its selection by row; the model keeps
                // it by line.
                if self.list.selected() != model.lines.selected_row() {
                    self.list.select(model.lines.selected_row());
                }
                self.list.redraw_visible();
            }
        }
    }

    /// Has the rows whose durations tick drawn again.
    pub fn show_ticking(&self, model: &Model) {
        for row in model.lines.ticking_rows() {
            self.list.redraw_row(row);
        }
    }

    /// The sort arrow in the grid's header.
    pub fn show_sort(&self, model: &Model) {
        let sort = model.lines.sort();
        let direction = if sort.descending {
            Sort::Descending
        } else {
            Sort::Ascending
        };
        self.list
            .set_sort_arrow(Some((sort.column.index(), direction)));
    }

    /// The two statistics panels.
    pub fn show_stats(&self, model: &Model, clock: &Clock) {
        let stats = model.stats_view(clock.now_ms(), clock);
        for ((_, value), (_, text)) in self.calls.iter().zip(&stats.calls) {
            value.set_text_if_changed(text);
        }
        for ((_, value), (_, text)) in self.carrier.iter().zip(&stats.carrier) {
            value.set_text_if_changed(text);
        }

        let shown = stats.rates.len().min(self.rates.len());
        if self.rates_shown.get() != Some(shown) {
            self.rates_shown.set(Some(shown));
            for (index, rate) in self.rates.iter().enumerate() {
                // With no rate to show the first row says so.
                let text = index < shown || index == 0;
                rate.label.set_visible(text);
                rate.percent.set_visible(text);
                rate.bar.hwnd().set_visible(index < shown);
            }
        }
        if shown == 0 {
            if let Some(first) = self.rates.first() {
                first.label.set_text_if_changed("Connect rates");
                first.percent.set_text_if_changed(model::EMPTY);
            }
            return;
        }
        for (rate, row) in self.rates.iter().zip(&stats.rates) {
            rate.label.set_text_if_changed(&row.label);
            if rate.bar.percent() != row.percent {
                rate.bar.set_percent(row.percent);
            }
            rate.percent
                .set_text_if_changed(&format!("{}%", row.percent));
        }
    }

    /// Adds to the event log the lines the model has and the pane does
    /// not. The pane follows its end unless the user has scrolled up.
    pub fn show_log(&self, model: &Model, zone: &dyn TimeZone) {
        let (generation, mut seen) = self.log_shown.get();
        if generation != model.log.generation() {
            // The buffer was started over: so is the pane.
            self.log.clear();
            seen = 0;
        }
        let lines: Vec<(String, Option<Colour>)> = model
            .log
            .since(seen)
            .map(|line| (line.text(zone), log_colour(line.class)))
            .collect();
        self.log.append(&lines, LOG_LIMIT);
        self.log_shown
            .set((model.log.generation(), model.log.added()));
    }

    /// The title of the window and of the group boxes that have one that
    /// changes.
    pub fn show_titles(&self, window: Hwnd, model: &Model) {
        window.set_text_if_changed(&model.title());
        self.lines_group.set_text_if_changed(&model.lines.title());
        self.log_group.set_text_if_changed(&model.log_title());
    }

    fn set_status_text(&self, part: usize, text: &str) {
        let mut texts = self.status_texts.borrow_mut();
        if texts.len() <= part {
            texts.resize(part + 1, String::new());
        }
        if texts[part] != text {
            texts[part] = text.to_string();
            self.status.set_text(part, text);
        }
    }

    /// The status bar: the message, then the parts the model lists.
    pub fn show_status(&self, model: &Model, clock: &Clock, notice: Option<&str>) {
        let message = match notice {
            Some(notice) => notice.to_string(),
            None => model.status_message(),
        };
        self.set_status_text(0, &message);
        for (index, text) in model.status_parts(clock.now_ms(), clock).iter().enumerate() {
            self.set_status_text(index + 1, text);
        }
    }

    /// The menus, the toolbar, the filter and the grid, as the model says
    /// they are now: what can be chosen, and what is checked.
    pub fn show_commands(&self, window: Hwnd, model: &Model) {
        let menu = Menu::of_window(window);
        {
            let mut available = self.available.borrow_mut();
            for (index, command) in Command::ALL.iter().enumerate() {
                let enabled = model.availability(*command).is_enabled();
                if available[index] == Some(enabled) {
                    continue;
                }
                available[index] = Some(enabled);
                if let Some(menu) = &menu {
                    menu.set_enabled(command.id(), enabled);
                }
                if command.tool_label().is_some() {
                    self.toolbar.set_enabled(command.id(), enabled);
                }
            }
        }
        if let Some(menu) = &menu {
            menu.set_radio(
                Command::ShowAll.id(),
                Command::ShowNotReady.id(),
                model.filter_command().id(),
            );
            menu.set_checked(Command::ViewToolbar.id(), self.toolbar_shown.get());
            menu.set_checked(Command::ViewStatusBar.id(), self.status_shown.get());
        }
        let filter = model.lines.filter().index();
        if self.filter.selected() != Some(filter) {
            self.filter.select(Some(filter));
        }
        // Without the service the lines are what they were, not what
        // they are: the grid shows them and takes no part.
        let grid = self.list.hwnd();
        if grid.enabled() != model.connected() {
            grid.set_enabled(model.connected());
        }
    }
}
