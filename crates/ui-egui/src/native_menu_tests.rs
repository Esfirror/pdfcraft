//! The macOS menu bar's items (src/native_menu.rs, macOS only), checked on every platform: each
//! registry command the bar shows does something when it is enabled, on a document.

use pdfcraft_engine::commands;

use crate::PdfCraftApp;

const PDF: &[u8] = b"%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >> endobj\n3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >> endobj\n4 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >> endobj\ntrailer << /Root 1 0 R >>\n%%EOF";

/// Open system pickers, the print dialog or the network: not for a unit test.
const SYSTEM: &[&str] = &[
    "file.open",
    "file.pin_folder",
    "create.file",
    "create.multiple",
    "create.multiple_separate",
    "create.images",
    "file.save_as",
    "page.insert",
    "page.replace",
    "print.dialog",
    "export.all_images",
    "help.check_updates",
    "file.open_recent",
];

fn menu_ids() -> Vec<&'static str> {
    let mut ids: Vec<&'static str> = ["File", "Edit", "View", "Pages", "Help"].iter().flat_map(|m| commands::menu(m).map(|s| s.id)).collect();
    ids.extend(["view.theme.system", "view.theme.light", "view.theme.dark", "view.continuous", "file.clear_recent"]);
    ids.extend(crate::canvas::PageLayout::ORDER.map(|l| l.command()));
    ids
}

#[test]
fn every_enabled_menu_command_does_something() {
    let mut idle = Vec::new();
    for id in menu_ids() {
        if SYSTEM.contains(&id) {
            continue;
        }
        let mut app = PdfCraftApp::new();
        app.open_bytes("menu.pdf", None, PDF.to_vec()).expect("fixture opens");
        let Some(spec) = commands::command(id) else {
            idle.push(format!("{id}: not registered"));
            continue;
        };
        if app.command_enabled(spec) && !app.execute(id) {
            idle.push(format!("{id}: enabled, but nothing runs it"));
        }
    }
    assert!(idle.is_empty(), "menu items that do nothing:\n{}", idle.join("\n"));
}

#[test]
fn the_menu_bar_names_only_registered_commands() {
    for id in ["app.preferences", "help.about", "view.focus_page_input", "file.open_recent", "file.clear_recent"] {
        assert!(commands::command(id).is_some(), "{id}");
    }
}
