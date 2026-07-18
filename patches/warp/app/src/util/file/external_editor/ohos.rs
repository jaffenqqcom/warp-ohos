use std::collections::HashMap;
use std::sync::OnceLock;

use warpui::AppContext;

use super::Editor;

static INSTALLED_EDITOR_METADATA: OnceLock<HashMap<Editor, EditorMetadata>> = OnceLock::new();

/// Metadata for an installed editor.
///
/// On OHOS no external desktop editors are available, so
/// this struct serves as a minimal placeholder for the
/// always-empty installed-editors collection.
struct EditorMetadata;

fn compute_installed_editors() -> HashMap<Editor, EditorMetadata> {
    HashMap::new()
}

impl Editor {
    fn installed_editors(&self) -> &HashMap<Editor, EditorMetadata> {
        INSTALLED_EDITOR_METADATA.get_or_init(compute_installed_editors)
    }

    /// Returns `false` for all editors — OHOS does not support
    /// desktop applications, so no external editors can be installed.
    pub fn is_installed(&self, _ctx: &mut AppContext) -> bool {
        false
    }
}
