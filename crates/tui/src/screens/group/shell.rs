//! The Shell tab of a group: the project shell tabs, opened in the group workspace
//! (`POST /groups/:id/shell`).

use crate::screens::project::shell::ShellTab;

pub fn tab(id: &str) -> ShellTab {
    ShellTab::group(id)
}
