//! Central action registry: every keyboard-triggerable command has one
//! entry here, so menu items, keyboard shortcuts and (later) a
//! Search-Everywhere / command palette all stay in sync with a single
//! source of truth.

use egui::{Key, KeyboardShortcut, Modifiers};

/// A stable identifier for an action, used to dispatch after a shortcut is
/// consumed (avoids stringly-typed matching in `app.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ActionId {
    Save,
    SaveAll,
    Send,
    CloseTab,
    NextTab,
    PrevTab,
    OpenWorkspace,
    ToggleCollections,
    /// Open the Settings dialog (`dialogs::settings`).
    OpenSettings,
    /// Open the curl-import dialog (`dialogs::curl_import`).
    ImportCurl,
    /// Open Search Everywhere directly in actions-only mode
    /// (`dialogs::search`); the all-sections mode is opened by a bare
    /// double `Shift` press, detected outside this registry.
    SearchActions,
}

/// One registered action: an id, a human-readable title (for menus / a
/// future command palette) and an optional default keyboard shortcut.
#[derive(Debug, Clone, Copy)]
pub struct Action {
    pub id: ActionId,
    pub title: &'static str,
    pub shortcut: Option<KeyboardShortcut>,
}

/// The full set of registered actions, in a stable order suitable for
/// listing in a command palette.
pub const ACTIONS: &[Action] = &[
    Action {
        id: ActionId::Save,
        title: "Save",
        shortcut: Some(KeyboardShortcut::new(Modifiers::COMMAND, Key::S)),
    },
    Action {
        id: ActionId::SaveAll,
        title: "Save All",
        shortcut: Some(KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::S)),
    },
    Action {
        id: ActionId::Send,
        title: "Send Request",
        shortcut: Some(KeyboardShortcut::new(Modifiers::COMMAND, Key::Enter)),
    },
    Action {
        id: ActionId::CloseTab,
        title: "Close Tab",
        shortcut: Some(KeyboardShortcut::new(Modifiers::COMMAND, Key::W)),
    },
    Action {
        id: ActionId::NextTab,
        title: "Next Tab",
        shortcut: Some(KeyboardShortcut::new(Modifiers::COMMAND, Key::Tab)),
    },
    Action {
        id: ActionId::PrevTab,
        title: "Previous Tab",
        shortcut: Some(KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::Tab)),
    },
    Action {
        id: ActionId::OpenWorkspace,
        title: "Open Workspace...",
        shortcut: Some(KeyboardShortcut::new(Modifiers::COMMAND, Key::O)),
    },
    Action {
        id: ActionId::ToggleCollections,
        title: "Toggle Collections",
        shortcut: Some(KeyboardShortcut::new(Modifiers::COMMAND, Key::Num1)),
    },
    Action {
        id: ActionId::OpenSettings,
        title: "Settings...",
        shortcut: Some(KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::ALT), Key::S)),
    },
    Action {
        id: ActionId::ImportCurl,
        title: "Import curl...",
        shortcut: Some(KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::V)),
    },
    Action {
        id: ActionId::SearchActions,
        title: "Search Actions...",
        shortcut: Some(KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::A)),
    },
];

/// Look up an action's shortcut by id, e.g. for display in a menu item.
pub fn shortcut_for(id: ActionId) -> Option<KeyboardShortcut> {
    ACTIONS.iter().find(|a| a.id == id).and_then(|a| a.shortcut)
}

/// Consume every registered shortcut against the current frame's input,
/// returning the first matching action id (if several were somehow pressed
/// at once, only one is dispatched per frame).
pub fn dispatch(ctx: &egui::Context) -> Option<ActionId> {
    ctx.input_mut(|input| {
        for action in ACTIONS {
            if let Some(shortcut) = action.shortcut {
                if input.consume_shortcut(&shortcut) {
                    return Some(action.id);
                }
            }
        }
        None
    })
}
