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

mod mount;

use crate::cli_util::CommandHelper;
use crate::command_error::CommandError;
use crate::ui::Ui;

/// Filesystem operations.
#[derive(clap::Subcommand, Clone, Debug)]
pub enum FsCommand {
    Mount(mount::FsMountArgs),
}

pub async fn cmd_fs(
    ui: &mut Ui,
    command: &CommandHelper,
    subcommand: &FsCommand,
) -> Result<(), CommandError> {
    match subcommand {
        FsCommand::Mount(args) => mount::cmd_fs_mount(ui, command, args).await,
    }
}
