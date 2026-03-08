use crate::models::PaneInfo;

pub fn pane_target(pane: &PaneInfo) -> String {
    format!("{}:{}.{}", pane.session, pane.window_index, pane.pane_index)
}
