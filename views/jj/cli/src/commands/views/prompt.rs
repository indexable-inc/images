// Copyright 2026 The Jujutsu Authors
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// https://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::io::Write as _;
use std::path::Path;

use tracing::instrument;

use super::ViewConfig;
use super::get_views_config;
use super::record;
use crate::cli_util::CommandHelper;
use crate::command_error::CommandError;
use crate::ui::Ui;

/// Say which view the working directory is inside, for a shell prompt
///
/// Prints one tab-separated line -- the view's name, how many published
/// commits are behind, how many view commits here are unpublished -- and
/// nothing at all outside every configured view. The counts come from the
/// record the last `jj views fetch` or `jj views status` left, so they are as
/// fresh as the last time something asked the published repository; a view
/// never surveyed prints its name alone. Nothing here derives and nothing
/// moves: pass `--ignore-working-copy` and the cost is a config read and a
/// file read, which a per-prompt caller can afford where a real survey's
/// derive it cannot.
#[derive(clap::Args, Clone, Debug)]
pub struct ViewsPromptArgs {}

#[instrument(skip_all)]
pub async fn cmd_views_prompt(
    ui: &mut Ui,
    command: &CommandHelper,
    _args: &ViewsPromptArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui).await?;
    let configured = get_views_config(&workspace_command).await?;
    let Ok(relative) = command
        .cwd()
        .strip_prefix(workspace_command.workspace_root())
    else {
        // `-R` pointed at a repository somewhere other than up from here, so
        // the working directory is inside none of its views.
        return Ok(());
    };
    let Some(view) = owning_view(&configured, relative) else {
        return Ok(());
    };

    let mut out = ui.stdout();
    match record::read(workspace_command.repo_path(), &view.name) {
        Some(record) => writeln!(out, "{}\t{}\t{}", view.name, record.incoming, record.ahead)?,
        None => writeln!(out, "{}", view.name)?,
    }
    Ok(())
}

/// The configured view whose path prefix contains `relative`, if any.
///
/// The longest prefix wins so a view nested inside another view's path
/// belongs to the inner one.
fn owning_view<'a>(configured: &'a [ViewConfig], relative: &Path) -> Option<&'a ViewConfig> {
    configured
        .iter()
        .filter(|view| relative.starts_with(&view.path))
        .max_by_key(|view| view.path.len())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::ViewConfig;
    use super::owning_view;

    fn view(name: &str, path: &str) -> ViewConfig {
        ViewConfig {
            name: name.to_owned(),
            path: path.to_owned(),
            remote: "https://example.com/repo.git".to_owned(),
            branch: "main".to_owned(),
            anchor: None,
            root_anchor: false,
            upstream: None,
        }
    }

    #[test]
    fn the_nearest_enclosing_view_owns_the_directory() {
        let configured = vec![view("outer", "vendor"), view("inner", "vendor/upstream")];

        let owner = owning_view(&configured, Path::new("vendor/upstream/src"));
        assert_eq!(owner.map(|view| view.name.as_str()), Some("inner"));

        let owner = owning_view(&configured, Path::new("vendor/other"));
        assert_eq!(owner.map(|view| view.name.as_str()), Some("outer"));
    }

    #[test]
    fn a_directory_under_no_view_has_no_owner() {
        let configured = vec![view("upstream", "vendor/upstream")];

        assert!(owning_view(&configured, Path::new("src")).is_none());
        // A sibling that merely shares the prefix string is not inside it.
        assert!(owning_view(&configured, Path::new("vendor/upstream-docs")).is_none());
    }
}
