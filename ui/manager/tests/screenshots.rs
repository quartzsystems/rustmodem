// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The screenshot tests: the main window and every dialog, each compared
//! with a golden picture.
//!
//! This is a program of its own (`harness = false`). It starts
//! `rustmodem-demosvc --script frozen`, whose lines, calls, statistics
//! and log are the same on every run, opens the manager against it at a
//! fixed size with its clock stopped, takes a picture of each view
//! through `win32::capture`, and compares each with the PNG of the same
//! name under `tests/goldens/`.
//!
//! - A pixel differs when any of its channels is off by more than the
//!   tolerance, 40 unless `RM_TOLERANCE` says otherwise.
//! - A view fails when more than 2 per mille of its pixels differ, or it
//!   is not the size of its golden, or it has no golden.
//! - Every picture goes to `target/screenshots/`, and for a view that
//!   fails so do its golden and a picture of where the two differ.
//! - `RM_BLESS=1` writes the pictures as the new goldens.
//!
//! The goldens are blessed on the CI runner image: fonts, text smoothing
//! and the drawing of the controls differ from one build of Windows to
//! the next, so a picture taken elsewhere is not expected to match them.
//! The comparison therefore fails the run where it means something: on
//! CI (the `CI` variable is set), or with `RM_SCREENSHOTS=strict`.
//! Elsewhere the differences are reported and the run passes;
//! `RM_SCREENSHOTS=skip` takes no pictures at all.
//!
//! The title bar is left out of the comparison. Its colours say whether
//! the window is the active one, which a test that runs behind whatever
//! else is on the desktop does not decide; what the title says is
//! asserted as text by the other tests.

#[cfg(not(windows))]
fn main() {}

#[cfg(windows)]
mod common;

#[cfg(windows)]
fn main() {
    std::process::exit(shots::run());
}

#[cfg(windows)]
mod shots {
    use std::cell::{Cell, RefCell};
    use std::path::{Path, PathBuf};
    use std::rc::Rc;
    use std::time::Duration;

    use ctlproto::ErrorInfo;
    use manager::{Hooks, groups_ids, ids, line_ids, routes_ids, settings_ids};
    use model::{Command, ConnectionState};
    use win32::controls::Combo;
    use win32::{App, Hwnd, Image, ListView, TaskDialog, capture, id};

    use crate::common::{Bench, DemoService, close, frame};

    /// The pipes of this test. Their names are in the pictures, so they
    /// are the same on every run; the test is one program and runs once.
    const PIPE: &str = "rustmodem-screenshots";
    const NO_PIPE: &str = "rustmodem-screenshots-none";

    /// How far off a channel may be before its pixel differs.
    const TOLERANCE: u8 = 40;

    /// How many of a thousand pixels may differ before a view fails.
    const LIMIT_PER_MILLE: u64 = 2;

    /// The height of the title bar at 96 DPI, which is not compared.
    const CAPTION: u32 = 32;

    /// How long the pictures of a view must stay the same for the view
    /// to have finished drawing, as a number of looks and the time
    /// between them.
    const STILL_LOOKS: u32 = 3;
    const LOOK_EVERY: Duration = Duration::from_millis(250);

    /// How many looks a view is given to come to rest.
    const MOST_LOOKS: u32 = 60;

    /// How many pixels may change between two looks at a view at rest: a
    /// caret blinks.
    const BLINK: u64 = 64;

    /// How long a task dialog is left to open before it is looked at, in
    /// milliseconds.
    const TASK_DIALOG_OPENS: u32 = 400;

    const CONNECT: Duration = Duration::from_secs(30);

    /// What is done with a dialog of the manager that has come up, in
    /// place of taking its picture: its name and its window.
    type WithDialog = Box<dyn Fn(&str, Hwnd)>;

    /// What the run is to do with a difference.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Mode {
        /// Write the pictures as the goldens.
        Bless,
        /// A view that fails fails the run.
        Strict,
        /// A view that fails is reported.
        Report,
    }

    /// The pictures taken so far, and what takes them.
    struct Camera {
        app: App,
        main: Cell<Option<Hwnd>>,
        taken: RefCell<Vec<(String, Image)>>,
        /// A picture is being taken: the window goes on taking events
        /// meanwhile, and what they start does not take another.
        busy: Cell<bool>,
    }

    impl Camera {
        fn pump(&self, time: Duration) {
            let until = std::time::Instant::now() + time;
            loop {
                if let Some(main) = self.main.get() {
                    self.app.pump(main, None);
                }
                if std::time::Instant::now() >= until {
                    return;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }

        /// A picture of a window once it has finished drawing: the same
        /// several times over.
        fn still(&self, window: Hwnd) -> Image {
            self.pump(LOOK_EVERY);
            let mut last = capture(window).expect("the window can be captured");
            let mut same = 1;
            for _ in 0..MOST_LOOKS {
                if same >= STILL_LOOKS {
                    break;
                }
                self.pump(LOOK_EVERY);
                let next = capture(window).expect("the window can be captured");
                let moved = if (next.width, next.height) == (last.width, last.height) {
                    differing(&next, &last, 0, 0).0
                } else {
                    u64::MAX
                };
                same = if moved <= BLINK { same + 1 } else { 1 };
                last = next;
            }
            last
        }

        /// Takes the picture of a view.
        fn take(&self, name: &str, window: Hwnd) {
            let image = self.still(window);
            println!("  {name}: {} x {}", image.width, image.height);
            self.taken.borrow_mut().push((name.to_string(), image));
        }
    }

    /// How many pixels of two pictures of the same size differ, and a
    /// picture of where: the pixels that differ in red over the first
    /// picture, paled. The rows above `skip_rows` are not compared.
    fn differing(shown: &Image, golden: &Image, tolerance: u8, skip_rows: u32) -> (u64, Image) {
        let mut count = 0;
        let mut marked = Vec::with_capacity(shown.rgba.len());
        let pixels = shown
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .zip(golden.rgba.as_chunks::<4>().0);
        for (index, (ours, theirs)) in pixels.enumerate() {
            let row = index as u32 / shown.width.max(1);
            let off = ours
                .iter()
                .zip(theirs)
                .take(3)
                .any(|(a, b)| a.abs_diff(*b) > tolerance);
            if off && row >= skip_rows {
                count += 1;
                marked.extend_from_slice(&[255, 0, 0, 255]);
            } else {
                let pale = |channel: u8| 170 + channel / 3;
                marked.extend_from_slice(&[pale(ours[0]), pale(ours[1]), pale(ours[2]), 255]);
            }
        }
        (
            count,
            Image {
                width: shown.width,
                height: shown.height,
                rgba: marked,
            },
        )
    }

    fn write_png(path: &Path, image: &Image) {
        let png = glyph::png::encode(&image.rgba, image.width, image.height);
        std::fs::write(path, png).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    }

    /// Reads a golden. `None` if there is none; a file that is there and
    /// is not a picture is an error that says so.
    fn read_png(path: &Path) -> Option<Result<Image, String>> {
        let file = std::fs::File::open(path).ok()?;
        let read = || -> Result<Image, String> {
            let mut decoder = png::Decoder::new(std::io::BufReader::new(file));
            // Eight bits a channel, whatever the file has.
            decoder.set_transformations(png::Transformations::normalize_to_color8());
            let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
            let size = reader
                .output_buffer_size()
                .ok_or_else(|| "the picture is too large".to_string())?;
            let mut pixels = vec![0; size];
            let info = reader.next_frame(&mut pixels).map_err(|e| e.to_string())?;
            pixels.truncate(info.buffer_size());
            let rgba: Vec<u8> = match info.color_type {
                png::ColorType::Rgba => pixels,
                png::ColorType::Rgb => pixels
                    .as_chunks::<3>()
                    .0
                    .iter()
                    .flat_map(|pixel| [pixel[0], pixel[1], pixel[2], 255])
                    .collect(),
                other => return Err(format!("the picture is {other:?}, not RGB")),
            };
            Ok(Image {
                width: info.width,
                height: info.height,
                rgba,
            })
        };
        Some(read())
    }

    /// The directory the goldens are in.
    fn goldens() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("goldens")
    }

    /// `target/screenshots`, in the target directory this program was
    /// built into.
    fn output() -> PathBuf {
        let program = std::env::current_exe().expect("the test program has a path");
        let target = program
            .ancestors()
            .find(|directory| directory.file_name().is_some_and(|name| name == "target"))
            .map(Path::to_path_buf)
            .unwrap_or_else(|| {
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("..")
                    .join("..")
                    .join("target")
            });
        target.join("screenshots")
    }

    fn mode() -> Option<Mode> {
        let set = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
        if set("RM_BLESS").is_some_and(|value| value != "0") {
            return Some(Mode::Bless);
        }
        match set("RM_SCREENSHOTS").as_deref() {
            Some("skip") => None,
            Some("strict") => Some(Mode::Strict),
            Some("report") => Some(Mode::Report),
            _ if set("CI").is_some() => Some(Mode::Strict),
            _ => Some(Mode::Report),
        }
    }

    fn tolerance() -> u8 {
        std::env::var("RM_TOLERANCE")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(TOLERANCE)
    }

    /// The name a task dialog's picture goes by.
    fn task_name(dialog: &TaskDialog) -> &'static str {
        match dialog.title.as_str() {
            "Remove line" => "remove-line",
            "RMD4011" => "restart-offer",
            "RMD2008" => "access-denied",
            _ => "message",
        }
    }

    /// The button a task dialog is closed with once its picture is
    /// taken: always the one that changes nothing.
    fn task_answer(dialog: &TaskDialog) -> u16 {
        match dialog.title.as_str() {
            "Remove line" => id::NO,
            // Later.
            "RMD4011" => 902,
            "RMD2008" => id::CANCEL,
            _ => id::CLOSE,
        }
    }

    /// The window without a service: what it shows, and why.
    fn disconnected(camera: &Rc<Camera>) {
        let bench = Bench::open(NO_PIPE);
        camera.main.set(Some(bench.opened.window));
        assert!(
            bench.run_until(CONNECT, |bench| {
                bench.opened.manager.with_model(|model| {
                    matches!(model.connection(), ConnectionState::Disconnected(Some(_)))
                })
            }),
            "the manager did not say why it is not connected"
        );
        camera.take("main-disconnected", bench.opened.window);
        camera.main.set(None);
    }

    /// The window against the frozen demo service, and every dialog.
    fn connected(camera: &Rc<Camera>) {
        let _service = DemoService::start(PIPE, "frozen", 8);
        // What a dialog of the manager does once it is up: its picture
        // is taken, and it is closed.
        let with_dialog: Rc<RefCell<Option<WithDialog>>> = Rc::new(RefCell::new(None));
        let current = Rc::new(Cell::new(None::<Hwnd>));
        let hooks = Hooks {
            dialog: Some(Rc::new({
                let camera = Rc::clone(camera);
                let with_dialog = Rc::clone(&with_dialog);
                let current = Rc::clone(&current);
                move |name: &str, window: Hwnd| {
                    current.set(Some(frame(name, window)));
                    if let Some(act) = with_dialog.borrow().as_ref() {
                        act(name, window);
                        return;
                    }
                    let sheet = window.parent();
                    // Whether this view has had its picture taken: a
                    // list says that it is up again once its editor has
                    // closed.
                    let again = camera.taken.borrow().iter().any(|(taken, _)| taken == name);
                    match (name, sheet) {
                        // Each page of the settings says so when it is
                        // up; the next is chosen then, and after the
                        // last the sheet is closed.
                        ("settings", Some(sheet)) => {
                            camera.take(name, sheet);
                            sheet.select_page(1);
                        }
                        ("settings-control", Some(sheet)) => {
                            camera.take(name, sheet);
                            sheet.select_page(2);
                        }
                        ("settings-sip", Some(sheet)) => {
                            camera.take(name, sheet);
                            sheet.select_page(3);
                        }
                        ("settings-media", Some(sheet)) => {
                            camera.take(name, sheet);
                            sheet.command(id::CANCEL);
                        }
                        // The lists of the SIP menu: the list, then the
                        // editor of its first entry, and when the list is
                        // up again it is closed.
                        ("hunt-groups", _) if !again => {
                            ListView::of(window.item(groups_ids::LIST)).select(Some(0));
                            camera.take(name, window);
                            window.command(groups_ids::EDIT);
                        }
                        ("routes", _) if !again => {
                            ListView::of(window.item(routes_ids::LIST)).select(Some(0));
                            camera.take(name, window);
                            window.command(routes_ids::EDIT);
                        }
                        _ => {
                            if !again {
                                camera.take(name, window);
                            }
                            close(name, window);
                        }
                    }
                }
            })),
            watch_task: Some(Rc::new({
                let camera = Rc::clone(camera);
                let current = Rc::clone(&current);
                move |dialog: &TaskDialog, window: Hwnd, elapsed: u32| {
                    if elapsed < TASK_DIALOG_OPENS || camera.busy.replace(true) {
                        return None;
                    }
                    camera.take(task_name(dialog), window);
                    camera.busy.set(false);
                    // A dialog of the manager that was refused something
                    // is still up behind the message: it goes too.
                    if task_name(dialog) == "message"
                        && let Some(behind) = current.get().filter(|behind| behind.exists())
                    {
                        behind.command(id::CANCEL);
                    }
                    Some(task_answer(dialog))
                }
            })),
            file: Some(Rc::new(|_: &str| None)),
            serial_ports: Some(Rc::new(|| {
                vec![
                    "COM11".to_string(),
                    "COM12".to_string(),
                    "COM13".to_string(),
                ]
            })),
            ..Hooks::default()
        };
        let bench = Bench::open_with(PIPE, hooks);
        let window = bench.opened.window;
        camera.main.set(Some(window));
        assert!(
            bench.run_until(CONNECT, |bench| {
                bench.opened.manager.with_model(|model| {
                    model.connected()
                        && model.lines.len() == 8
                        && model.stats().is_some()
                        && !model.log.is_empty()
                }) && bench.opened.manager.requests_waiting() == 0
            }),
            "the manager did not connect to the demo service"
        );
        camera.take("main", window);

        // With a line selected the commands for it can be chosen.
        let list = ListView::of(window.item(ids::LIST));
        list.select(Some(0));
        bench.pump();
        camera.take("main-line-selected", window);

        // Line 5 is idle and has no call: the one to edit.
        list.select(Some(4));
        bench.pump();
        for command in [
            Command::Connect,
            Command::NewLine,
            Command::EditLine,
            Command::CallHistory,
            Command::Settings,
            Command::PortsAndDrivers,
            Command::About,
            Command::TrunkPeers,
            Command::HuntGroups,
            Command::Routes,
            Command::CallCapture,
            Command::RateCap,
            Command::LoopbackTest,
            Command::AtConsole,
            Command::SipTrace,
        ] {
            let before = camera.taken.borrow().len();
            bench.choose(command);
            // The AT console is not modal: it is up when the command
            // returns, and its picture is taken a moment later.
            bench.run_until(Duration::from_secs(20), |_| {
                camera.taken.borrow().len() > before
            });
            assert!(
                camera.taken.borrow().len() > before,
                "{command:?} showed nothing"
            );
        }

        // The diagnostics of a line that is in a call: line 1.
        list.select(Some(0));
        bench.pump();
        let before = camera.taken.borrow().len();
        bench.choose(Command::LineDiagnostics);
        bench.run_until(Duration::from_secs(20), |_| {
            camera.taken.borrow().len() > before
        });
        assert!(
            camera.taken.borrow().len() > before,
            "the diagnostics showed nothing"
        );
        list.select(Some(4));
        bench.pump();

        // A line's own modem settings, from the line's dialog.
        *with_dialog.borrow_mut() = Some(Box::new({
            let camera = Rc::clone(camera);
            let asked = Cell::new(false);
            move |name, dialog| {
                if name == "line-modem" {
                    camera.take(name, dialog);
                    close(name, dialog);
                } else if asked.replace(true) {
                    // The settings have been shown and closed: the
                    // line's dialog says that it is up again.
                    close(name, dialog);
                } else {
                    dialog.command(line_ids::MODEM);
                }
            }
        }));
        bench.choose(Command::EditLine);
        *with_dialog.borrow_mut() = None;
        assert!(
            camera
                .taken
                .borrow()
                .iter()
                .any(|(name, _)| name == "line-modem"),
            "the line's modem settings showed nothing"
        );

        // Remove line asks first.
        bench.choose(Command::RemoveLine);

        // A refusal from the service: a new line on a port a line has.
        *with_dialog.borrow_mut() = Some(Box::new(|_, dialog| {
            Combo::of(dialog.item(line_ids::PORT)).set_text("CNCB0");
            dialog.command(id::OK);
        }));
        bench.choose(Command::NewLine);

        // A change of settings, and the offer to restart that follows.
        *with_dialog.borrow_mut() = Some(Box::new(|name, page| {
            let sheet = page.parent().expect("a page is in its sheet");
            if name == "settings" {
                sheet.select_page(1);
            } else {
                page.item(settings_ids::PIPE).set_text("rustmodem-other");
                sheet.command(id::OK);
            }
        }));
        bench.choose(Command::Settings);
        *with_dialog.borrow_mut() = None;

        // The message that the pipe does not admit this user cannot be
        // had from a service that admits everyone: it is shown as the
        // manager builds it.
        let denied = ErrorInfo::from(msg::message(
            "RMD2008",
            &["rustmodem-control", "Access is denied. (os error 5)"],
        ));
        let dialog = manager::denied_dialog(&denied);
        let mut watch = |task: Hwnd, elapsed: u32| -> Option<u16> {
            if elapsed < TASK_DIALOG_OPENS || camera.busy.replace(true) {
                return None;
            }
            camera.take("access-denied", task);
            camera.busy.set(false);
            Some(id::CANCEL)
        };
        win32::task_dialog_watched(Some(window), &dialog, &mut watch);

        camera.main.set(None);
    }

    /// Compares the pictures with their goldens, or writes them as the
    /// goldens. Returns how many views failed.
    fn judge(taken: &[(String, Image)], mode: Mode) -> usize {
        let goldens = goldens();
        let output = output();
        std::fs::create_dir_all(&output).expect("target/screenshots can be made");
        let tolerance = tolerance();
        let mut failed = 0;
        for (name, image) in taken {
            write_png(&output.join(format!("{name}.png")), image);
            if mode == Mode::Bless {
                std::fs::create_dir_all(&goldens).expect("the goldens directory can be made");
                write_png(&goldens.join(format!("{name}.png")), image);
                println!("  {name}: blessed");
                continue;
            }
            let golden = match read_png(&goldens.join(format!("{name}.png"))) {
                None => {
                    println!("  {name}: FAILED, there is no golden");
                    failed += 1;
                    continue;
                }
                Some(Err(problem)) => {
                    println!("  {name}: FAILED, the golden cannot be read: {problem}");
                    failed += 1;
                    continue;
                }
                Some(Ok(golden)) => golden,
            };
            if (golden.width, golden.height) != (image.width, image.height) {
                println!(
                    "  {name}: FAILED, it is {} x {} and its golden is {} x {}",
                    image.width, image.height, golden.width, golden.height
                );
                write_png(&output.join(format!("{name}.golden.png")), &golden);
                failed += 1;
                continue;
            }
            let (count, marked) = differing(image, &golden, tolerance, CAPTION);
            let pixels = u64::from(image.width) * u64::from(image.height);
            let per_mille = count * 1000 / pixels.max(1);
            if count * 1000 > pixels * LIMIT_PER_MILLE {
                println!(
                    "  {name}: FAILED, {count} of {pixels} pixels differ ({per_mille} per mille)"
                );
                write_png(&output.join(format!("{name}.golden.png")), &golden);
                write_png(&output.join(format!("{name}.diff.png")), &marked);
                failed += 1;
            } else {
                println!("  {name}: ok, {count} of {pixels} pixels differ");
            }
        }
        // A golden with no picture is a view that went missing.
        if mode != Mode::Bless
            && let Ok(entries) = std::fs::read_dir(&goldens)
        {
            for entry in entries.flatten() {
                let path = entry.path();
                let stem = path.file_stem().and_then(|stem| stem.to_str());
                if path.extension().is_some_and(|extension| extension == "png")
                    && let Some(stem) = stem
                    && !taken.iter().any(|(name, _)| name == stem)
                {
                    println!("  {stem}: FAILED, there is a golden and no picture");
                    failed += 1;
                }
            }
        }
        failed
    }

    /// Runs the screenshot tests. Returns the exit code of the program.
    pub fn run() -> i32 {
        let Some(mode) = mode() else {
            println!("screenshots: skipped, RM_SCREENSHOTS=skip");
            return 0;
        };
        let dpi = win32::metrics::system_dpi();
        if dpi != 96 {
            println!(
                "screenshots: this desktop is at {dpi} DPI and the goldens are at 96 (100 percent)"
            );
            // Nothing can be compared; where the comparison is the point
            // that is a failure.
            return i32::from(mode == Mode::Strict);
        }
        println!("screenshots: taking the pictures");
        // A row under the pointer lights up, and a button glows: the
        // pointer goes where none of the windows are.
        let pointer = win32::pointer();
        win32::move_pointer(0, 0);
        let camera = Rc::new(Camera {
            app: App::new(),
            main: Cell::new(None),
            taken: RefCell::new(Vec::new()),
            busy: Cell::new(false),
        });
        disconnected(&camera);
        connected(&camera);
        win32::move_pointer(pointer.0, pointer.1);

        println!("screenshots: comparing with {}", goldens().display());
        let taken = camera.taken.borrow();
        let failed = judge(&taken, mode);
        println!("screenshots: the pictures are in {}", output().display());
        match (mode, failed) {
            (Mode::Bless, _) => {
                println!("screenshots: {} goldens written", taken.len());
                0
            }
            (_, 0) => {
                println!("screenshots: {} views match their goldens", taken.len());
                0
            }
            (Mode::Strict, failed) => {
                println!("screenshots: {failed} views FAILED");
                1
            }
            (Mode::Report, failed) => {
                println!(
                    "screenshots: {failed} views differ from their goldens. The goldens are \
                     blessed on the CI runner image, and this is not it: the run passes. Set \
                     RM_SCREENSHOTS=strict to fail on a difference here."
                );
                0
            }
        }
    }
}
