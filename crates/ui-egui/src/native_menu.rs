//! The macOS menu bar: PdfCraft, File, Edit, View, Go, Pages, Window and Help, as native menus.
//!
//! The commands are the registry's (`pdfcraft_engine::commands`), the same ones the in-window
//! Menu shows, with live labels and enablement. Items show the registry's shortcuts as key
//! equivalents. AppKit takes such a key before the window sees it, so a key equivalent is handed
//! back to egui as the very key press it was: the keyboard keeps going through egui exactly as
//! before (text fields, form fields and the ⌘W rule in `registry_shortcuts` included), and a
//! menu never takes a key away from the document. Only a click runs the command directly.
//!
//! Built on the first frame (on the main thread, after AppKit has finished launching, so it
//! replaces winit's default menu) and rebuilt when the interface language changes. Clicks and
//! key equivalents arrive on a channel; `raw_input_hook` turns keys and Cut / Copy / Paste /
//! Select All into the input events the keyboard would have produced, and `logic` runs
//! everything else.

use std::sync::mpsc::{Receiver, Sender, channel};

use muda::accelerator::{Accelerator, Code, Modifiers};
use muda::{IsMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use pdfcraft_engine::commands;

use crate::canvas::{Fit, PageLayout};
use crate::{PdfCraftApp, RightPanel, i18n};

/// Labels only the menu bar uses, in Italian (the catalogs have the rest; other languages fall
/// back to English for these few).
const EXTRA_IT: &[(&str, &str)] = &[
    ("Services", "Servizi"),
    ("Hide PdfCraft", "Nascondi PdfCraft"),
    ("Hide Others", "Nascondi altre"),
    ("Show All", "Mostra tutte"),
    ("Quit PdfCraft", "Esci da PdfCraft"),
    ("Window", "Finestra"),
    ("Minimize", "Riduci a icona"),
    ("Zoom", "Ingrandisci"),
    ("Enter Full Screen", "Attiva modalità a tutto schermo"),
    ("Bring All to Front", "Porta tutto in primo piano"),
    ("Go", "Vai"),
    ("First page", "Prima pagina"),
    ("Last page", "Ultima pagina"),
    ("Paste", "Incolla"),
];

/// A translated label.
fn tr(s: &str) -> String {
    if i18n::current().code() == "it"
        && let Some((_, it)) = EXTRA_IT.iter().find(|(en, _)| *en == s)
    {
        return (*it).to_owned();
    }
    i18n::t(s).to_owned()
}

/// What a native item does when chosen.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Action {
    /// Run a registered command, as the in-window menu does.
    Command(&'static str),
    /// Open a recent file by path.
    Recent(String),
    /// Feed egui an input event, as the keyboard would.
    Input(InputKind),
    /// A document-view action the in-window View menu performs directly.
    View(ViewAction),
    Panel(usize),
    /// Window ▸ Minimize, without AppKit's ⌘M (which stays the app's to use).
    Minimize,
    /// A registry shortcut pressed while its item was the key equivalent: fed to egui as that key.
    Key(commands::Shortcut),
    /// PdfCraft ▸ Quit: closes the window the way its close button does, so unsaved changes are
    /// asked about (#1136). AppKit's own Quit (`terminate:`) would end the process at once.
    Quit,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum InputKind {
    Cut,
    Copy,
    Paste,
    SelectAll,
    /// ⌘Z / ⇧⌘Z in a focused text field (the field keeps its own undo history).
    TextUndo,
    TextRedo,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum ViewAction {
    ActualSize,
    FitPage,
    FitWidth,
    FitHeight,
    ZoomIn,
    ZoomOut,
    RotateCw,
    RotateCcw,
    PrevPage,
    NextPage,
    FirstPage,
    LastPage,
    Back,
    Forward,
}

/// The side panels, in the in-window View ▸ Side panels order.
const PANELS: [(RightPanel, &str); 11] = [
    (RightPanel::Comments, "Comments"),
    (RightPanel::Bookmarks, "Bookmarks"),
    (RightPanel::Pages, "Pages"),
    (RightPanel::Fields, "Fields"),
    (RightPanel::Layers, "Layers"),
    (RightPanel::Attachments, "Attachments"),
    (RightPanel::Signatures, "Signatures"),
    (RightPanel::Accessibility, "Accessibility Checker"),
    (RightPanel::Tags, "Tags"),
    (RightPanel::Search, "Search"),
    (RightPanel::Compare, "Compare"),
];

/// Registry commands the native bar shows somewhere other than their registry menu (or not at
/// all, because macOS puts them elsewhere).
const MOVED: [&str; 4] = ["app.preferences", "help.about", "view.focus_page_input", "file.open_recent"];

/// When a native item is enabled.
#[derive(Clone, Copy, Debug, PartialEq)]
enum When {
    Always,
    Command(&'static str),
    Document,
    Back,
    Forward,
}

struct Tracked {
    item: MenuItem,
    when: When,
    /// Registry command whose live label ("Undo Typing") the item follows.
    label_of: Option<&'static str>,
    enabled: bool,
    label: String,
}

pub(crate) struct NativeMenu {
    /// Kept alive: AppKit holds the menu only while muda's handle exists.
    _menu: Menu,
    /// Every submenu, kept alive for the same reason.
    _submenus: Vec<Submenu>,
    events: Receiver<(MenuEvent, bool)>,
    actions: Vec<Bound>,
    tracked: Vec<Tracked>,
    recent: Submenu,
    recent_paths: Option<Vec<String>>,
    recent_seq: u64,
    /// The interface language (its code) the labels are in.
    lang: &'static str,
}

/// What an item does: when clicked, and when its key equivalent was pressed (if that differs).
type Bound = (muda::MenuId, Action, Option<Action>);

struct Builder {
    actions: Vec<Bound>,
    tracked: Vec<Tracked>,
    seq: u64,
}

impl Builder {
    fn item(&mut self, label: &str, action: Action, when: When) -> MenuItem {
        self.keyed_item(label, action, when, None, None)
    }

    /// An item with a key equivalent; `keyed` is what that key does when it differs from a click.
    fn keyed_item(&mut self, label: &str, action: Action, when: When, accel: Option<Accelerator>, keyed: Option<Action>) -> MenuItem {
        self.seq = self.seq.saturating_add(1);
        let keyed = accel.and(keyed);
        let item = MenuItem::with_id(format!("pdfcraft.{}", self.seq), label, matches!(when, When::Always), accel);
        self.actions.push((item.id().clone(), action, keyed));
        let label_of = match when {
            When::Command(id) => Some(id),
            _ => None,
        };
        self.tracked.push(Tracked { item: item.clone(), when, label_of, enabled: matches!(when, When::Always), label: label.to_owned() });
        item
    }

    fn command(&mut self, spec: &'static commands::CommandSpec) -> MenuItem {
        let label = i18n::menu_label(spec.id, spec.label);
        let accel = spec.shortcut.and_then(accelerator);
        self.keyed_item(&label, Action::Command(spec.id), When::Command(spec.id), accel, spec.shortcut.map(Action::Key))
    }

    /// A registry menu's commands in `sections`, skipping `skip`.
    fn registry(&mut self, menu: &Submenu, name: &str, skip: &[&str]) {
        let specs = commands::menu(name).filter(|s| !MOVED.contains(&s.id) && !skip.contains(&s.id));
        for (i, section) in sections(specs).into_iter().enumerate() {
            if i > 0 {
                append(menu, &PredefinedMenuItem::separator());
            }
            for spec in section {
                let item = self.command(spec);
                append(menu, &item);
            }
        }
    }
}

/// Commands split where the id's group (`file.`, `export.`…) changes. Runs of one-command groups
/// share a section, so a menu doesn't wall single items off between separators.
pub(crate) fn sections<'a>(specs: impl IntoIterator<Item = &'a commands::CommandSpec>) -> Vec<Vec<&'a commands::CommandSpec>> {
    let mut groups: Vec<Vec<&commands::CommandSpec>> = Vec::new();
    for spec in specs {
        let g = spec.id.split('.').next().unwrap_or(spec.id);
        match groups.last_mut() {
            Some(last) if last.first().is_some_and(|p| p.id.split('.').next().unwrap_or(p.id) == g) => last.push(spec),
            _ => groups.push(vec![spec]),
        }
    }
    let mut out: Vec<Vec<&commands::CommandSpec>> = Vec::new();
    let mut singles = false;
    for group in groups {
        let single = group.len() == 1;
        match out.last_mut() {
            Some(last) if single && singles => last.extend(group),
            _ => out.push(group),
        }
        singles = single;
    }
    out
}

/// The key equivalent for a registry shortcut: ⌘ (with ⇧ and ⌃ as given) and a letter, digit,
/// comma or Tab. Other keys (+, −, Delete…) stay egui's alone and show no equivalent.
fn accelerator(s: commands::Shortcut) -> Option<Accelerator> {
    let code = match s.key {
        "," => Code::Comma,
        "Tab" => Code::Tab,
        k => {
            let mut chars = k.chars();
            let (Some(c), None) = (chars.next(), chars.next()) else { return None };
            letter_or_digit(c)?
        }
    };
    let mut mods = Modifiers::empty();
    if s.command {
        mods |= Modifiers::META;
    }
    if s.shift {
        mods |= Modifiers::SHIFT;
    }
    if s.mac_ctrl {
        mods |= Modifiers::CONTROL;
    }
    // A bare key would fire while typing in a field, which egui's handling never does.
    (s.command || s.mac_ctrl).then(|| Accelerator::new(mods, code))
}

fn letter_or_digit(c: char) -> Option<Code> {
    const LETTERS: [Code; 26] = [
        Code::KeyA,
        Code::KeyB,
        Code::KeyC,
        Code::KeyD,
        Code::KeyE,
        Code::KeyF,
        Code::KeyG,
        Code::KeyH,
        Code::KeyI,
        Code::KeyJ,
        Code::KeyK,
        Code::KeyL,
        Code::KeyM,
        Code::KeyN,
        Code::KeyO,
        Code::KeyP,
        Code::KeyQ,
        Code::KeyR,
        Code::KeyS,
        Code::KeyT,
        Code::KeyU,
        Code::KeyV,
        Code::KeyW,
        Code::KeyX,
        Code::KeyY,
        Code::KeyZ,
    ];
    const DIGITS: [Code; 10] =
        [Code::Digit0, Code::Digit1, Code::Digit2, Code::Digit3, Code::Digit4, Code::Digit5, Code::Digit6, Code::Digit7, Code::Digit8, Code::Digit9];
    let c = c.to_ascii_uppercase();
    if c.is_ascii_uppercase() {
        LETTERS.get(usize::from(c as u8 - b'A')).copied()
    } else if c.is_ascii_digit() {
        DIGITS.get(usize::from(c as u8 - b'0')).copied()
    } else {
        None
    }
}

fn cmd(key: Code) -> Option<Accelerator> {
    Some(Accelerator::new(Modifiers::META, key))
}

/// Appending only fails for an item already in the menu, which this module never does; a
/// failure just leaves the item out.
fn append(menu: &Submenu, item: &dyn IsMenuItem) {
    if let Err(e) = menu.append(item) {
        log::warn!("native menu: {e}");
    }
}

impl NativeMenu {
    /// Build the menu bar and install it. `None` off the main thread (tests, headless hosts).
    /// `events` is the previous bar's channel: muda takes the click handler only once per
    /// process, so a rebuilt bar keeps receiving on the first one.
    pub(crate) fn install(ctx: &egui::Context, events: Option<Receiver<(MenuEvent, bool)>>) -> Option<Self> {
        objc2::MainThreadMarker::new()?;
        let events = events.unwrap_or_else(|| {
            let (tx, rx) = channel::<(MenuEvent, bool)>();
            set_handler(tx, ctx.clone());
            rx
        });
        let mut b = Builder { actions: Vec::new(), tracked: Vec::new(), seq: 0 };
        let menu = Menu::new();
        let mut subs = Vec::new();

        // PdfCraft
        let app_menu = Submenu::new("PdfCraft", true);
        append(&app_menu, &b.item(&i18n::menu_label("help.about", "About PdfCraft"), Action::Command("help.about"), When::Always));
        append(&app_menu, &PredefinedMenuItem::separator());
        if let Some(spec) = commands::command("app.preferences") {
            append(&app_menu, &b.command(spec));
            append(&app_menu, &PredefinedMenuItem::separator());
        }
        append(&app_menu, &PredefinedMenuItem::services(Some(tr("Services").as_str())));
        append(&app_menu, &PredefinedMenuItem::separator());
        append(&app_menu, &PredefinedMenuItem::hide(Some(tr("Hide PdfCraft").as_str())));
        append(&app_menu, &PredefinedMenuItem::hide_others(Some(tr("Hide Others").as_str())));
        append(&app_menu, &PredefinedMenuItem::show_all(Some(tr("Show All").as_str())));
        append(&app_menu, &PredefinedMenuItem::separator());
        append(&app_menu, &b.keyed_item(&tr("Quit PdfCraft"), Action::Quit, When::Always, cmd(Code::KeyQ), None));

        // File, with Open Recent as a live submenu after Open…
        let file = Submenu::new(tr("File"), true);
        let recent = Submenu::new(i18n::menu_label("file.open_recent", "Open Recent"), false);
        for (i, section) in sections(commands::menu("File").filter(|s| !MOVED.contains(&s.id))).into_iter().enumerate() {
            if i > 0 {
                append(&file, &PredefinedMenuItem::separator());
            }
            for spec in section {
                append(&file, &b.command(spec));
                if spec.id == "file.open" {
                    append(&file, &recent);
                }
            }
        }

        // Edit: the registry's Undo / Redo, then the clipboard, then the rest.
        let edit = Submenu::new(tr("Edit"), true);
        b.registry(&edit, "Edit", &[]);
        append(&edit, &PredefinedMenuItem::separator());
        // The same input events a click makes are what egui-winit makes of the keys.
        for (label, kind, key) in [
            ("Cut", InputKind::Cut, Code::KeyX),
            ("Copy", InputKind::Copy, Code::KeyC),
            ("Paste", InputKind::Paste, Code::KeyV),
            ("Select All", InputKind::SelectAll, Code::KeyA),
        ] {
            append(&edit, &b.keyed_item(&tr(label), Action::Input(kind), When::Always, cmd(key), None));
        }

        // View: zoom, page display, the registry's View commands, theme and side panels.
        let view = Submenu::new(tr("View"), true);
        for (label, a) in [
            ("Actual size", ViewAction::ActualSize),
            ("Zoom to page level", ViewAction::FitPage),
            ("Fit to width", ViewAction::FitWidth),
            ("Fit to height", ViewAction::FitHeight),
            ("Zoom in", ViewAction::ZoomIn),
            ("Zoom out", ViewAction::ZoomOut),
        ] {
            append(&view, &b.item(&tr(label), Action::View(a), When::Document));
        }
        append(&view, &PredefinedMenuItem::separator());
        append(&view, &b.item(&tr("Rotate view clockwise"), Action::View(ViewAction::RotateCw), When::Document));
        append(&view, &b.item(&tr("Rotate view counterclockwise"), Action::View(ViewAction::RotateCcw), When::Document));
        append(&view, &PredefinedMenuItem::separator());
        for l in PageLayout::ORDER {
            append(&view, &b.item(&tr(l.label()), Action::Command(l.command()), When::Document));
        }
        append(&view, &b.item(&tr("Continuous scrolling"), Action::Command("view.continuous"), When::Document));
        append(&view, &PredefinedMenuItem::separator());
        b.registry(&view, "View", &[]);
        append(&view, &PredefinedMenuItem::separator());
        let theme = Submenu::new(tr("Display theme"), true);
        for id in ["view.theme.system", "view.theme.light", "view.theme.dark"] {
            if let Some(spec) = commands::command(id) {
                append(&theme, &b.item(&i18n::menu_label(id, spec.label), Action::Command(id), When::Always));
            }
        }
        append(&view, &theme);
        let panels = Submenu::new(tr("Side panels"), true);
        for (i, (_, label)) in PANELS.iter().enumerate() {
            append(&panels, &b.item(&tr(label), Action::Panel(i), When::Always));
        }
        append(&view, &panels);

        // Go
        let go = Submenu::new(tr("Go"), true);
        for (label, a) in [
            ("Previous page", ViewAction::PrevPage),
            ("Next page", ViewAction::NextPage),
            ("First page", ViewAction::FirstPage),
            ("Last page", ViewAction::LastPage),
        ] {
            append(&go, &b.item(&tr(label), Action::View(a), When::Document));
        }
        if let Some(spec) = commands::command("view.focus_page_input") {
            append(&go, &b.command(spec));
        }
        append(&go, &PredefinedMenuItem::separator());
        append(&go, &b.item(&tr("Previous view"), Action::View(ViewAction::Back), When::Back));
        append(&go, &b.item(&tr("Next view"), Action::View(ViewAction::Forward), When::Forward));

        // Pages
        let pages = Submenu::new(tr("Pages"), true);
        b.registry(&pages, "Pages", &[]);

        // Window
        let window = Submenu::new(tr("Window"), true);
        append(&window, &b.item(&tr("Minimize"), Action::Minimize, When::Always));
        append(&window, &PredefinedMenuItem::maximize(Some(tr("Zoom").as_str())));
        append(&window, &PredefinedMenuItem::separator());
        append(&window, &PredefinedMenuItem::fullscreen(Some(tr("Enter Full Screen").as_str())));
        append(&window, &PredefinedMenuItem::separator());
        append(&window, &PredefinedMenuItem::bring_all_to_front(Some(tr("Bring All to Front").as_str())));

        // Help
        let help = Submenu::new(tr("Help"), true);
        b.registry(&help, "Help", &[]);

        for m in [&app_menu, &file, &edit, &view, &go, &pages, &window, &help] {
            if let Err(e) = menu.append(m) {
                log::warn!("native menu: {e}");
            }
        }
        menu.init_for_nsapp();
        window.set_as_windows_menu_for_nsapp();
        help.set_as_help_menu_for_nsapp();
        subs.extend([app_menu, file, edit, view, go, pages, window, help, theme, panels]);
        Some(Self {
            _menu: menu,
            _submenus: subs,
            events,
            actions: b.actions,
            tracked: b.tracked,
            recent,
            recent_paths: None,
            recent_seq: 0,
            lang: i18n::current().code(),
        })
    }

    /// The actions chosen since the last call.
    /// A pressed key equivalent comes back as that key where it does something else than a click.
    pub(crate) fn take_actions(&mut self) -> Vec<Action> {
        let mut out = Vec::new();
        while let Ok((e, from_key)) = self.events.try_recv() {
            if let Some((_, click, keyed)) = self.actions.iter().find(|(id, _, _)| id == e.id()) {
                out.push(keyed.as_ref().filter(|_| from_key).unwrap_or(click).clone());
            }
        }
        out
    }

    fn next_id(&mut self, kind: &str) -> String {
        self.recent_seq = self.recent_seq.saturating_add(1);
        format!("pdfcraft.{kind}.{}", self.recent_seq)
    }

    /// Whether the interface language changed since the bar was built.
    pub(crate) fn stale(&self) -> bool {
        self.lang != i18n::current().code()
    }

    /// Bring labels, enablement and Open Recent in line with the app.
    pub(crate) fn sync(&mut self, app: &PdfCraftApp) {
        let active = app.active_ids().map(|(_, id)| id);
        let view = app.active.and_then(|i| app.views.get(i));
        for t in &mut self.tracked {
            let enabled = match t.when {
                When::Always => true,
                When::Command(id) => commands::command(id).is_some_and(|spec| app.command_enabled(spec)),
                When::Document => view.is_some(),
                When::Back => view.is_some_and(|v| !v.back.is_empty()),
                When::Forward => view.is_some_and(|v| !v.forward.is_empty()),
            };
            if enabled != t.enabled {
                t.item.set_enabled(enabled);
                t.enabled = enabled;
            }
            if let Some(spec) = t.label_of.and_then(commands::command) {
                let label = i18n::menu_label(spec.id, &commands::current_label(spec, &app.session, active));
                if label != t.label {
                    t.item.set_text(&label);
                    t.label = label;
                }
            }
        }
        let paths: Vec<String> = app.recent.iter().map(|r| r.path.clone()).collect();
        if self.recent_paths.as_ref() != Some(&paths) {
            // Bounded: each pass removes one item of a finite submenu.
            while self.recent.remove_at(0).is_some() {}
            self.actions.retain(|(_, a, _)| !matches!(a, Action::Recent(_) | Action::Command("file.clear_recent")));
            for r in &app.recent {
                let label = if r.name.is_empty() { r.path.clone() } else { r.name.clone() };
                let item = MenuItem::with_id(self.next_id("recent"), label, true, None);
                self.actions.push((item.id().clone(), Action::Recent(r.path.clone()), None));
                append(&self.recent, &item);
            }
            if !app.recent.is_empty() {
                append(&self.recent, &PredefinedMenuItem::separator());
                let clear = MenuItem::with_id(self.next_id("recent"), tr("Clear Recent Files"), true, None);
                self.actions.push((clear.id().clone(), Action::Command("file.clear_recent"), None));
                append(&self.recent, &clear);
            }
            self.recent.set_enabled(!app.recent.is_empty());
            self.recent_paths = Some(paths);
        }
    }
}

/// Clicks and key equivalents go to `tx`, and wake the event loop so the next frame sees them.
fn set_handler(tx: Sender<(MenuEvent, bool)>, ctx: egui::Context) {
    MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
        let _ = tx.send((e, from_key()));
        ctx.request_repaint();
    }));
}

/// Whether the event AppKit is handling (the one that chose the item) is a key press.
fn from_key() -> bool {
    use objc2_app_kit::{NSApplication, NSEventType};
    let Some(mtm) = objc2::MainThreadMarker::new() else { return false };
    NSApplication::sharedApplication(mtm).currentEvent().is_some_and(|e| e.r#type() == NSEventType::KeyDown)
}

/// The key press and release a registry shortcut is, as `registry_shortcuts` matches it.
pub(crate) fn shortcut_events(s: commands::Shortcut) -> Vec<egui::Event> {
    let Some(key) = egui::Key::from_name(s.key) else { return Vec::new() };
    let mut modifiers = egui::Modifiers::NONE;
    if s.command {
        modifiers |= egui::Modifiers::COMMAND;
    }
    if s.shift {
        modifiers |= egui::Modifiers::SHIFT;
    }
    if s.mac_ctrl {
        modifiers |= egui::Modifiers::CTRL;
    }
    [true, false].map(|pressed| egui::Event::Key { key, physical_key: Some(key), pressed, repeat: false, modifiers }).to_vec()
}

/// The input events the keyboard would have produced for `kind` (as egui-winit makes them).
pub(crate) fn input_events(kind: InputKind) -> Vec<egui::Event> {
    let key = |key: egui::Key, modifiers: egui::Modifiers| {
        [true, false].map(|pressed| egui::Event::Key { key, physical_key: Some(key), pressed, repeat: false, modifiers })
    };
    match kind {
        InputKind::Cut => vec![egui::Event::Cut],
        InputKind::Copy => vec![egui::Event::Copy],
        InputKind::Paste => {
            let text = arboard::Clipboard::new().and_then(|mut c| c.get_text()).unwrap_or_default().replace("\r\n", "\n");
            if text.is_empty() { Vec::new() } else { vec![egui::Event::Paste(text)] }
        }
        InputKind::SelectAll => key(egui::Key::A, egui::Modifiers::COMMAND).to_vec(),
        InputKind::TextUndo => key(egui::Key::Z, egui::Modifiers::COMMAND).to_vec(),
        InputKind::TextRedo => key(egui::Key::Z, egui::Modifiers::COMMAND | egui::Modifiers::SHIFT).to_vec(),
    }
}

impl PdfCraftApp {
    /// Run one native menu action (everything but input events, which `raw_input_hook` feeds).
    pub(crate) fn run_native_action(&mut self, action: Action) {
        match action {
            Action::Command(id) => {
                self.execute(id);
            }
            Action::Recent(path) => self.open_recent(&path),
            Action::Panel(i) => {
                if let Some((p, _)) = PANELS.get(i) {
                    self.choose_right_panel(Some(*p));
                }
            }
            Action::View(a) => {
                let Some(v) = self.active.and_then(|i| self.views.get_mut(i)) else { return };
                match a {
                    ViewAction::ActualSize => v.set_zoom(1.0),
                    ViewAction::FitPage => v.fit = Fit::Page,
                    ViewAction::FitWidth => v.fit = Fit::Width,
                    ViewAction::FitHeight => {
                        v.fit = Fit::Height;
                        v.goto = Some((v.current, 0.0));
                    }
                    ViewAction::ZoomIn => v.zoom_step(true),
                    ViewAction::ZoomOut => v.zoom_step(false),
                    ViewAction::RotateCw => v.rotate_view(true),
                    ViewAction::RotateCcw => v.rotate_view(false),
                    ViewAction::PrevPage => v.step_page(false),
                    ViewAction::NextPage => v.step_page(true),
                    ViewAction::FirstPage => v.go_to_page(0),
                    ViewAction::LastPage => v.go_to_page(usize::MAX),
                    ViewAction::Back => {
                        v.view_history(false);
                    }
                    ViewAction::Forward => {
                        v.view_history(true);
                    }
                }
            }
            Action::Minimize => {
                if let Some(ctx) = &self.ctx {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                }
            }
            Action::Quit => {
                if let Some(ctx) = &self.ctx {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
            Action::Input(_) | Action::Key(_) => {}
        }
    }
}

impl PdfCraftApp {
    /// Before input: chosen items that are keyboard input join this frame's events; the rest
    /// wait for `logic`.
    pub(crate) fn poll_native_menu(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        let Some(menu) = self.native_menu.as_mut() else { return };
        for action in menu.take_actions() {
            // Undo / Redo belong to a text field while one has focus, as ⌘Z does.
            let action = match action {
                Action::Command("edit.undo") if ctx.text_edit_focused() => Action::Input(InputKind::TextUndo),
                Action::Command("edit.redo") if ctx.text_edit_focused() => Action::Input(InputKind::TextRedo),
                a => a,
            };
            match action {
                Action::Input(kind) => raw.events.extend(input_events(kind)),
                Action::Key(s) => raw.events.extend(shortcut_events(s)),
                a => self.native_actions.push(a),
            }
        }
    }

    /// Each frame: build (or, after a language change, rebuild) the bar, run what was chosen,
    /// and bring labels and enablement up to date.
    pub(crate) fn native_menu_frame(&mut self, ctx: &egui::Context) {
        if !self.native_menu_bar {
            return;
        }
        if self.native_menu.as_ref().is_none_or(NativeMenu::stale) {
            let events = self.native_menu.take().map(|m| m.events);
            self.native_menu = NativeMenu::install(ctx, events);
        }
        for action in std::mem::take(&mut self.native_actions) {
            self.run_native_action(action);
        }
        if let Some(mut menu) = self.native_menu.take() {
            menu.sync(self);
            self.native_menu = Some(menu);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(sections: &[Vec<&commands::CommandSpec>]) -> Vec<Vec<&'static str>> {
        sections.iter().map(|s| s.iter().map(|c| c.id).collect()).collect()
    }

    #[test]
    fn one_command_groups_share_a_section() {
        let view = sections(commands::menu("View"));
        // No section is a lone command between two others that are lone commands too.
        for w in ids(&view).windows(2) {
            assert!(!(w[0].len() == 1 && w[1].len() == 1), "{w:?}");
        }
        // Nothing is dropped or reordered.
        let flat: Vec<&str> = ids(&view).concat();
        let all: Vec<&str> = commands::menu("View").map(|c| c.id).collect();
        assert_eq!(flat, all);
    }

    #[test]
    fn shortcuts_become_key_equivalents_with_their_modifiers() {
        let s = |key, command, shift, mac_ctrl| commands::Shortcut { command, shift, mac_ctrl, key };
        assert_eq!(accelerator(s("O", true, false, false)), Some(Accelerator::new(Modifiers::META, Code::KeyO)));
        assert_eq!(accelerator(s("S", true, true, false)), Some(Accelerator::new(Modifiers::META | Modifiers::SHIFT, Code::KeyS)));
        assert_eq!(accelerator(s(",", true, false, false)), Some(Accelerator::new(Modifiers::META, Code::Comma)));
        assert_eq!(accelerator(s("Tab", false, false, true)), Some(Accelerator::new(Modifiers::CONTROL, Code::Tab)));
        assert_eq!(accelerator(s("3", true, false, false)), Some(Accelerator::new(Modifiers::META, Code::Digit3)));
        // Keys egui keeps to itself, and bare keys that would fire while typing.
        assert_eq!(accelerator(s("+", true, false, false)), None);
        assert_eq!(accelerator(s("Delete", true, false, false)), None);
        assert_eq!(accelerator(s("H", false, false, false)), None);
    }

    #[test]
    fn a_key_equivalent_is_the_key_registry_shortcuts_matches() {
        let save = commands::command("file.save").and_then(|c| c.shortcut).expect("Save has a shortcut");
        let events = shortcut_events(save);
        assert_eq!(events.len(), 2);
        let ctx = egui::Context::default();
        let raw = egui::RawInput { events, ..Default::default() };
        let mut hit = false;
        let mut out = ctx.run_ui(raw, |ui| {
            hit = ui.input_mut(|i| i.consume_shortcut(&egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::S)));
        });
        out.textures_delta.clear();
        assert!(hit);
    }
}
