// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The wrappers on real windows: events reach the handler, the controls
//! hold what they are given, dialogs run under the dialog manager, and a
//! window can be captured.
//!
//! Each test makes its windows on its own thread and never shows a window
//! that waits for a person: what has to be pressed is pressed by a timer.

#![cfg(windows)]

use std::cell::{Cell as StdCell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use win32::controls::{self, Combo, Progress, Tabs, UpDown};
use win32::{
    Align, App, Colour, Column, DialogTemplate, Event, Handler, Hwnd, ImageList, ListView, Menu,
    MenuItem, Rect, Reply, RichEdit, Sort, StatusBar, ToolButton, Toolbar, Window, WindowOptions,
    capture, channel, code, id, property_sheet,
};

/// Records the events a window gets, and answers the ones a test cares
/// about.
#[derive(Default)]
struct Recorder {
    seen: RefCell<Vec<String>>,
    /// What the list view is told its cells hold.
    cells: RefCell<Vec<[&'static str; 3]>>,
}

impl Recorder {
    fn saw(&self, what: &str) -> bool {
        self.seen.borrow().iter().any(|seen| seen == what)
    }

    fn count(&self, what: &str) -> usize {
        self.seen
            .borrow()
            .iter()
            .filter(|seen| *seen == what)
            .count()
    }
}

impl Handler for Recorder {
    fn event(&self, _window: Hwnd, event: Event<'_>) -> Reply {
        let name = match event {
            Event::Create => "create".to_string(),
            Event::Close => "close".to_string(),
            Event::Destroy => "destroy".to_string(),
            Event::Size { width, height, .. } => format!("size {width}x{height}"),
            Event::Timer { id } => format!("timer {id}"),
            Event::Wake => "wake".to_string(),
            Event::Command { id, code } => format!("command {id} {code}"),
            Event::ListSelection { id } => format!("selection {id}"),
            Event::ListCell {
                row, column, cell, ..
            } => {
                if let Some(text) = self.cells.borrow().get(row).and_then(|r| r.get(column)) {
                    cell.text = (*text).to_string();
                }
                cell.image = (column == 0).then_some(0);
                return Reply::Handled;
            }
            Event::ListCellColour { row, colour, .. } => {
                if row == 1 {
                    *colour = Some(Colour::rgb(0, 128, 0));
                }
                return Reply::Handled;
            }
            Event::MinSize(size) => {
                *size = (200, 150);
                return Reply::Handled;
            }
            _ => return Reply::Default,
        };
        self.seen.borrow_mut().push(name);
        Reply::Default
    }
}

fn window(title: &str, handler: Rc<dyn Handler>, menu: Option<Menu>) -> Hwnd {
    Window::create(
        WindowOptions {
            title,
            size: Some((640, 480)),
            menu,
            owner: None,
            dialog_keys: true,
        },
        handler,
    )
    .expect("the window can be made")
}

/// Handles messages until `done` is true, or fails the test.
fn pump_until(app: &App, main: Hwnd, what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        app.pump(main, None);
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn a_window_gets_its_events_and_frees_its_handler() {
    let app = App::new();
    let recorder = Rc::new(Recorder::default());
    let main = window("events", recorder.clone(), None);
    assert!(recorder.saw("create"), "created before create returns");
    assert!(main.exists());
    assert_eq!(main.text(), "events");
    assert_eq!(main.class_name(), "QuartzSystems.rustmodem.Window");
    // The window holds the handler as well as the test.
    assert_eq!(Rc::strong_count(&recorder), 2);

    main.set_rect(Rect::new(50, 50, 500, 400));
    let client = main.client_rect();
    assert!(recorder.saw(&format!("size {}x{}", client.width, client.height)));
    // The smallest size the handler gave is kept to.
    main.set_rect(Rect::new(50, 50, 10, 10));
    assert_eq!(
        (main.screen_rect().width, main.screen_rect().height),
        (200, 150)
    );

    main.set_timer(7, 5);
    pump_until(&app, main, "the timer", || recorder.count("timer 7") >= 2);
    main.kill_timer(7);

    // Another thread wakes the window; what it sent is in the queue.
    let (sender, receiver) = channel::<u32>(main);
    let sending = std::thread::spawn(move || {
        for number in 0..100 {
            assert!(sender.send(number));
        }
        sender
    });
    let mut received = Vec::new();
    pump_until(&app, main, "a hundred numbers", || {
        if recorder.saw("wake") {
            received.extend(receiver.drain());
        }
        received.len() == 100
    });
    assert_eq!(received, (0..100).collect::<Vec<u32>>());
    let sender = sending.join().unwrap();

    main.command(42);
    pump_until(&app, main, "the command", || recorder.saw("command 42 0"));

    // Closing destroys the window, which frees the handler it held.
    main.close();
    pump_until(&app, main, "the window to go", || !main.exists());
    assert!(recorder.saw("close") && recorder.saw("destroy"));
    assert_eq!(Rc::strong_count(&recorder), 1);
    // A window that has gone cannot be woken; the value is queued all the
    // same and nothing is lost or leaked.
    assert!(!sender.send(1));
    assert_eq!(receiver.drain(), [1]);
}

#[test]
fn the_controls_hold_what_they_are_given() {
    let _app = App::new();
    let menu = Menu::bar(&[MenuItem::submenu(
        "&File",
        vec![MenuItem::command(10, "&One"), MenuItem::command(11, "&Two")],
    )]);
    let main = window("controls", Rc::new(Recorder::default()), Some(menu));
    let bar = Menu::of_window(main).expect("the window has its menu bar");
    assert_eq!(bar.commands().len(), 2);
    bar.set_enabled(11, false);
    assert!(bar.enabled(10) && !bar.enabled(11));

    let group = controls::group_box(main, "Group");
    assert_eq!(group.text(), "Group");
    assert_eq!(group.class_name(), "Button");
    assert_eq!(group.parent(), Some(main));

    let label = controls::value(main, "a & b", true);
    assert_eq!(label.text(), "a & b");
    label.set_text_if_changed("changed");
    assert_eq!(label.text(), "changed");

    let edit = controls::edit(main, 20, false);
    assert_eq!(main.item(20), edit);
    edit.set_text("Line 1 — né");
    assert_eq!(edit.text(), "Line 1 — né");
    edit.set_enabled(false);
    assert!(!edit.enabled());
    edit.set_visible(false);
    assert!(!edit.visible());

    let check = controls::check_box(main, 21, "&Check");
    assert!(!controls::checked(check));
    controls::set_checked(check, true);
    assert!(controls::checked(check));
    let first = controls::radio_button(main, 22, "One", true);
    let second = controls::radio_button(main, 23, "Two", false);
    controls::set_checked(second, true);
    assert!(!controls::checked(first) && controls::checked(second));
    let button = controls::button(main, 24, "&Go", true);
    assert_eq!(button.text(), "&Go");

    let list = Combo::drop_list(main, 25);
    assert_eq!((list.count(), list.selected()), (0, None));
    for entry in ["CNCB0", "CNCB1", "CNCB10"] {
        list.add(entry);
    }
    assert_eq!(list.count(), 3);
    assert_eq!(list.entry(2), "CNCB10");
    assert_eq!(list.entry(9), "");
    list.select(Some(1));
    assert_eq!((list.selected(), list.text().as_str()), (Some(1), "CNCB1"));
    list.select(None);
    assert_eq!(list.selected(), None);
    list.clear();
    assert_eq!(list.count(), 0);
    let typed = Combo::editable(main, 26);
    typed.add("CNCB0");
    typed.set_text("COM7");
    assert_eq!((typed.text().as_str(), typed.selected()), ("COM7", None));
    assert_eq!(Combo::of(main.item(26)), typed);

    let progress = Progress::create(main);
    progress.set_percent(40);
    assert_eq!(progress.percent(), 40);
    progress.set_percent(400);
    assert_eq!(progress.percent(), 100);

    let number = controls::edit(main, 27, true);
    number.set_rect(Rect::new(10, 10, 80, 24));
    let arrows = UpDown::create(main, number, 0, 255);
    assert_eq!(arrows.hwnd().class_name(), "msctls_updown32");
    // The arrows put themselves inside the right end of the edit control.
    let (edit_rect, arrow_rect) = (number.screen_rect(), arrows.hwnd().screen_rect());
    assert!(arrow_rect.x > edit_rect.x && arrow_rect.width > 0);

    let tabs = Tabs::create(main, 28);
    tabs.add("Service");
    tabs.add("Control");
    assert_eq!(tabs.selected(), Some(0));
    tabs.select(1);
    assert_eq!(tabs.selected(), Some(1));
    let page = tabs.page_rect(Rect::new(0, 0, 300, 200));
    assert!(page.y > 0 && page.height < 200 && page.width <= 300);

    let mut images = ImageList::new(16);
    images.add(&[255u8; 16 * 16 * 4]);
    let toolbar = Toolbar::create(
        main,
        29,
        &images,
        &[
            ToolButton::new(40, "Answer now", 0),
            ToolButton::Separator,
            ToolButton::new(41, "Hang up", 0),
        ],
    );
    assert_eq!(toolbar.count(), 3);
    assert!(toolbar.enabled(40) && toolbar.enabled(41));
    toolbar.set_enabled(41, false);
    assert!(!toolbar.enabled(41));
    let (width, height) = toolbar.size();
    assert!(width > 100 && height >= 16, "{width}x{height}");

    let status = StatusBar::create(main, 30);
    status.place();
    assert!(status.height() >= 16);
    status.set_parts(&[200, 300, -1]);
    status.set_text(0, "Ready");
    status.set_simple(Some("Describes a command."));
    status.set_simple(None);
    // The bar is along the bottom of the window.
    let (bar_rect, main_rect) = (status.hwnd().screen_rect(), main.screen_rect());
    assert!(bar_rect.y + bar_rect.height <= main_rect.y + main_rect.height);
    assert!(bar_rect.y > main_rect.y + main_rect.height / 2);

    main.destroy();
    assert!(!edit.exists());
}

#[test]
fn a_list_view_asks_its_owner_for_its_cells() {
    let app = App::new();
    let recorder = Rc::new(Recorder::default());
    *recorder.cells.borrow_mut() = vec![
        ["1", "CNCB0", "Idle (AT)"],
        ["2", "CNCB1", "Online"],
        ["3", "CNCB2", "Ringing"],
    ];
    let main = window("list", recorder.clone(), None);
    let list = ListView::create(main, 50);
    list.hwnd().set_rect(Rect::new(0, 0, 400, 300));
    let mut images = ImageList::new(16);
    images.add(&[128u8; 16 * 16 * 4]);
    list.set_images(&images);
    list.set_columns(&[
        Column::new("#", 30, Align::Right),
        Column::new("COM port", 70, Align::Left),
        Column::new("State", 100, Align::Left),
    ]);
    assert_eq!(list.column_count(), 3);
    assert_eq!(list.column_width(1), 70);
    list.set_column_width(1, 90);
    assert_eq!(list.column_width(1), 90);
    // Replacing the columns leaves no column behind.
    list.set_columns(&[
        Column::new("#", 30, Align::Right),
        Column::new("COM port", 70, Align::Left),
        Column::new("State", 100, Align::Left),
    ]);
    assert_eq!(list.column_count(), 3);

    assert_eq!(list.row_count(), 0);
    list.set_row_count(3);
    assert_eq!(list.row_count(), 3);
    // What the list shows is what the handler says, cell by cell.
    assert_eq!(list.cell_text(0, 0), "1");
    assert_eq!(list.cell_text(1, 1), "CNCB1");
    assert_eq!(list.cell_text(2, 2), "Ringing");
    recorder.cells.borrow_mut()[1][2] = "Idle (AT)";
    list.redraw_row(1);
    assert_eq!(list.cell_text(1, 2), "Idle (AT)");

    assert_eq!(list.selected(), None);
    list.select(Some(2));
    assert_eq!(list.selected(), Some(2));
    assert!(recorder.saw("selection 50"));
    list.select(None);
    assert_eq!(list.selected(), None);
    list.select(Some(0));

    list.set_sort_arrow(Some((1, Sort::Descending)));
    list.set_sort_arrow(Some((0, Sort::Ascending)));
    list.set_sort_arrow(None);

    // Many rows: scrolling and what is in view.
    list.set_row_count(500);
    assert_eq!(list.top_row(), 0);
    list.ensure_visible(400);
    assert!(list.top_row() > 300, "top row {}", list.top_row());
    list.set_top_row(100);
    assert_eq!(list.top_row(), 100);
    // The number of rows changes and the list stays where it is.
    list.set_row_count(450);
    assert_eq!(list.top_row(), 100);
    list.redraw_visible();
    assert!(list.row_screen_rect(100).is_some());

    // Drawing asks for the colours; nothing goes wrong when it does.
    main.set_visible(true);
    list.hwnd().redraw();
    app.pump(main, None);
    main.destroy();
}

#[test]
fn a_log_pane_keeps_its_last_lines_and_follows_its_end() {
    let app = App::new();
    let main = window("log", Rc::new(Recorder::default()), None);
    let log = RichEdit::create(main, 60);
    log.hwnd().set_rect(Rect::new(0, 0, 500, 120));
    main.set_visible(true);
    assert!(log.at_end(), "an empty pane is at its end");

    let lines = |range: std::ops::Range<u32>| -> Vec<(String, Option<Colour>)> {
        range
            .map(|n| {
                let colour = (n % 2 == 0).then_some(Colour::rgb(192, 0, 0));
                (format!("line {n:04}"), colour)
            })
            .collect()
    };
    log.append(&lines(0..3), 100);
    assert_eq!(log.text(), "line 0000\r\nline 0001\r\nline 0002\r\n");
    assert_eq!(log.line_count(), 4, "three lines and the empty one after");

    // Past the limit the oldest lines go.
    log.append(&lines(3..250), 100);
    app.pump(main, None);
    let text = log.text();
    let kept: Vec<&str> = text.lines().collect();
    assert_eq!(kept.len(), 100);
    assert_eq!((kept[0], kept[99]), ("line 0150", "line 0249"));
    assert!(log.at_end(), "the pane followed what was added");

    // Appending nothing does nothing.
    log.append(&[], 100);
    assert_eq!(log.text(), text);

    // Scrolled up, the pane stays where the reader left it.
    log.hwnd().focus();
    scroll_to_top(log);
    app.pump(main, None);
    assert!(!log.at_end());
    log.append(&lines(250..255), 1000);
    app.pump(main, None);
    assert!(!log.at_end(), "the pane did not jump to the end");
    log.scroll_to_end();
    assert!(log.at_end());

    log.clear();
    assert_eq!(log.text(), "");
    main.destroy();
}

/// Scrolls a pane to its start, as Ctrl+Home does.
fn scroll_to_top(log: RichEdit) {
    // WM_VSCROLL with SB_TOP.
    const WM_VSCROLL: u32 = 0x0115;
    const SB_TOP: usize = 6;
    post(log.hwnd(), WM_VSCROLL, SB_TOP);
}

/// Posts a message with one parameter to a window.
fn post(window: Hwnd, message: u32, wparam: usize) {
    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::SendMessageW;
    // SAFETY: the message takes no pointers, and the handle is a window
    // of this thread.
    unsafe {
        SendMessageW(
            HWND(window.raw() as *mut _),
            message,
            Some(WPARAM(wparam)),
            Some(LPARAM(0)),
        );
    }
}

/// The handler of a dialog that fills itself in, reads itself back, and
/// presses one of its own buttons from a timer.
struct SelfDriving {
    press: u16,
    seen: RefCell<Vec<String>>,
    applied: StdCell<u32>,
}

impl SelfDriving {
    fn new(press: u16) -> Rc<SelfDriving> {
        Rc::new(SelfDriving {
            press,
            seen: RefCell::new(Vec::new()),
            applied: StdCell::new(0),
        })
    }
}

impl Handler for SelfDriving {
    fn event(&self, dialog: Hwnd, event: Event<'_>) -> Reply {
        match event {
            Event::InitDialog => {
                dialog.item(100).set_text("Front desk");
                let rings = dialog.item(101);
                rings.set_text("2");
                UpDown::of(dialog.item(102)).attach(rings, 0, 255);
                let port = Combo::of(dialog.item(103));
                port.add("CNCB0");
                port.add("CNCB1");
                port.select(Some(1));
                controls::set_checked(dialog.item(104), true);
                dialog.set_timer(1, 10);
                self.seen.borrow_mut().push("init".to_string());
                Reply::Default
            }
            Event::Timer { id: 1 } => {
                dialog.kill_timer(1);
                self.seen.borrow_mut().push(format!(
                    "name={} rings={} port={} enabled={}",
                    dialog.item(100).text(),
                    dialog.item(101).text(),
                    Combo::of(dialog.item(103)).text(),
                    controls::checked(dialog.item(104)),
                ));
                // The size is the template's, for the font and the DPI:
                // dialog units map to more than a pixel each.
                let mapped = dialog.dialog_units(Rect::new(0, 0, 200, 100));
                assert!(mapped.width > 200 && mapped.height > 100, "{mapped:?}");
                dialog.command(self.press);
                Reply::Handled
            }
            Event::Command {
                id: 300,
                code: code::CLICKED,
            } => {
                self.seen.borrow_mut().push("custom".to_string());
                dialog.end_dialog(300);
                Reply::Handled
            }
            Event::Apply => {
                self.applied.set(self.applied.get() + 1);
                Reply::Default
            }
            _ => Reply::Default,
        }
    }
}

fn line_dialog() -> DialogTemplate {
    DialogTemplate::new("Edit line", 200, 110)
        .label(7, 9, 40, 8, "&Name:")
        .edit(100, 50, 7, 143, 14)
        .label(7, 27, 40, 8, "&Rings:")
        .number(101, 50, 25, 40, 14)
        .up_down(102)
        .label(7, 45, 40, 8, "&Port:")
        .combo(103, 50, 43, 80, 100)
        .check_box(104, 7, 63, 100, 10, "&Enabled")
        .button(300, 7, 89, 50, 14, "&Custom")
        .ok_cancel()
}

#[test]
fn a_modal_dialog_runs_under_the_dialog_manager_and_ends_with_its_button() {
    let _app = App::new();
    let main = window("owner", Rc::new(Recorder::default()), None);
    let template = line_dialog();

    for (press, expected) in [(id::OK, id::OK), (id::CANCEL, id::CANCEL), (300, 300)] {
        let handler = SelfDriving::new(press);
        let result = template.show_modal(Some(main), handler.clone());
        assert_eq!(result, expected, "pressing {press}");
        let seen = handler.seen.borrow();
        assert_eq!(seen[0], "init");
        assert_eq!(seen[1], "name=Front desk rings=2 port=CNCB1 enabled=true");
        assert_eq!(seen.len(), if press == 300 { 3 } else { 2 });
        // The dialog has gone and has let go of its handler.
        drop(seen);
        assert_eq!(Rc::strong_count(&handler), 1);
    }
    // With no owner too.
    assert_eq!(template.show_modal(None, SelfDriving::new(id::OK)), id::OK);
    main.destroy();
}

#[test]
fn a_modeless_dialog_stays_open_beside_its_owner_until_it_is_closed() {
    let app = App::new();
    let main = window("owner", Rc::new(Recorder::default()), None);
    let handler = SelfDriving::new(id::CANCEL);
    let dialog = line_dialog()
        .create_modeless(Some(main), handler.clone())
        .expect("the dialog can be made");
    let window = dialog.hwnd();
    assert!(window.exists());
    assert_eq!(window.text(), "Edit line");
    assert_eq!(window.item(100).text(), "Front desk");
    window.set_visible(true);
    // Its timer presses Cancel, which closes a modeless dialog.
    pump_until(&app, main, "the dialog to close", || !window.exists());
    assert_eq!(handler.seen.borrow().len(), 2);
    assert_eq!(Rc::strong_count(&handler), 1);
    // Dropping the wrapper of a dialog that has gone is harmless, and
    // dropping one that has not closes it.
    drop(dialog);
    let second = line_dialog()
        .create_modeless(Some(main), SelfDriving::new(0))
        .unwrap();
    let window = second.hwnd();
    drop(second);
    assert!(!window.exists());
    main.destroy();
}

/// A page of a property sheet that says it changed and then presses the
/// sheet's OK.
struct Page {
    press: u16,
    change: bool,
    applied: StdCell<u32>,
    shown: StdCell<bool>,
}

impl Handler for Page {
    fn event(&self, page: Hwnd, event: Event<'_>) -> Reply {
        match event {
            Event::InitDialog => {
                self.shown.set(true);
                page.item(100).set_text("debug");
                page.set_timer(1, 10);
                Reply::Default
            }
            Event::Timer { id: 1 } => {
                page.kill_timer(1);
                if self.change {
                    page.page_changed();
                }
                if let Some(sheet) = page.parent() {
                    sheet.command(self.press);
                }
                Reply::Handled
            }
            Event::Apply => {
                self.applied.set(self.applied.get() + 1);
                Reply::Default
            }
            _ => Reply::Default,
        }
    }
}

#[test]
fn a_property_sheet_applies_the_pages_that_were_shown() {
    let _app = App::new();
    let main = window("owner", Rc::new(Recorder::default()), None);
    let page = |press: u16, change: bool| {
        Rc::new(Page {
            press,
            change,
            applied: StdCell::new(0),
            shown: StdCell::new(false),
        })
    };
    let template = || {
        DialogTemplate::new("", 220, 120)
            .label(7, 9, 60, 8, "&Log level:")
            .edit(100, 70, 7, 100, 14)
    };

    // OK applies the first page, which was shown; the second page was
    // never looked at, and is neither created nor asked to apply.
    let (first, second) = (page(id::OK, true), page(id::OK, true));
    let accepted = property_sheet(
        Some(main),
        "Settings",
        vec![
            ("Service".to_string(), template(), first.clone()),
            ("Control".to_string(), template(), second.clone()),
        ],
    );
    assert!(accepted);
    assert!(first.shown.get() && !second.shown.get());
    assert_eq!((first.applied.get(), second.applied.get()), (1, 0));
    assert_eq!(Rc::strong_count(&first), 1);
    assert_eq!(Rc::strong_count(&second), 1);

    // Cancel applies nothing.
    let only = page(id::CANCEL, true);
    let accepted = property_sheet(
        Some(main),
        "Settings",
        vec![("Service".to_string(), template(), only.clone())],
    );
    assert!(!accepted);
    assert_eq!(only.applied.get(), 0);
    main.destroy();
}

#[test]
fn a_window_is_captured_as_it_is_drawn() {
    let app = App::new();
    let recorder = Rc::new(Recorder::default());
    *recorder.cells.borrow_mut() = vec![["1", "CNCB0", "Idle"], ["2", "CNCB1", "Online"]];
    let main = window("capture", recorder, None);
    let list = ListView::create(main, 50);
    list.hwnd().set_rect(Rect::new(10, 10, 400, 200));
    list.set_columns(&[
        Column::new("#", 40, Align::Right),
        Column::new("COM port", 100, Align::Left),
        Column::new("State", 100, Align::Left),
    ]);
    list.set_row_count(2);
    main.set_visible(true);
    // Let the window paint.
    for _ in 0..10 {
        app.pump(main, None);
        std::thread::sleep(Duration::from_millis(20));
    }
    let image = capture(main).expect("the window can be captured");
    // The picture is the frame that is drawn: no wider than the window.
    let outer = main.screen_rect();
    assert!(image.width > 300 && image.width <= outer.width as u32);
    assert!(image.height > 200 && image.height <= outer.height as u32);
    assert_eq!(image.rgba.len(), (image.width * image.height * 4) as usize);
    // Something is drawn in it: a title bar, a list, text.
    let mut colours = std::collections::BTreeSet::new();
    for pixel in image.rgba.as_chunks::<4>().0.iter().step_by(7) {
        colours.insert(*pixel);
    }
    assert!(colours.len() > 8, "{} colours", colours.len());
    // And not a black border around it.
    let corner = image.pixel(image.width / 2, image.height - 2);
    assert_ne!(corner, [0, 0, 0, 255], "the bottom edge is the window's");
    main.destroy();
}
