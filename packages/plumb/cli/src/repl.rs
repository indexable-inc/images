//! The interactive shell: reedline for line editing, a stage summary after
//! every run, `:` commands for inspecting past runs.

use std::borrow::Cow;
use std::process::ExitCode;

use plumb_core::{Error, Report, Shell};
use reedline::{FileBackedHistory, Prompt, PromptEditMode, PromptHistorySearch, Reedline, Signal};

struct PlumbPrompt {
    shell: Shell,
}

impl Prompt for PlumbPrompt {
    fn render_prompt_left(&self) -> Cow<'_, str> {
        let cwd = self.shell.cwd();
        let short = cwd
            .file_name()
            .map_or_else(|| cwd.display().to_string(), |name| name.to_string_lossy().into_owned());
        Cow::Owned(format!("plumb {short}"))
    }

    fn render_prompt_right(&self) -> Cow<'_, str> {
        let pending = self.shell.background_pending();
        if pending == 0 {
            Cow::Borrowed("")
        } else {
            Cow::Owned(format!("bg:{pending}"))
        }
    }

    fn render_prompt_indicator(&self, _edit_mode: PromptEditMode) -> Cow<'_, str> {
        Cow::Borrowed("> ")
    }

    fn render_prompt_multiline_indicator(&self) -> Cow<'_, str> {
        Cow::Borrowed("... ")
    }

    fn render_prompt_history_search_indicator(
        &self,
        _history_search: PromptHistorySearch,
    ) -> Cow<'_, str> {
        Cow::Borrowed("search: ")
    }
}

/// Run the REPL until `exit` or ctrl-d.
pub fn run(shell: &Shell) -> ExitCode {
    let mut editor = Reedline::create().with_history(Box::new(FileBackedHistory::new(1000).unwrap_or_default()));
    let prompt = PlumbPrompt {
        shell: shell.clone(),
    };
    println!(
        "plumb: runs are values. `:runs` lists them, `:json N` dumps one, \
         `${{o[N]}}`/`${{e[N]}}`/`${{o[N][K]}}` reuse their output. ctrl-d exits."
    );
    loop {
        for report in shell.finished_background() {
            println!("[bg run {} finished: exit {}]", report.id, report.status);
        }
        match editor.read_line(&prompt) {
            Ok(Signal::Success(line)) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                if let Some(command) = trimmed.strip_prefix(':') {
                    inspect(shell, command);
                    continue;
                }
                match shell.eval(trimmed) {
                    Ok(report) => summarize(&report),
                    Err(Error::ExitRequested { code }) => {
                        return crate::exit_code(code);
                    }
                    Err(error) => eprintln!("plumb: {error}"),
                }
            }
            Ok(Signal::CtrlD) => return ExitCode::SUCCESS,
            Ok(_) => {}
            Err(error) => {
                eprintln!("plumb: {error}");
                return ExitCode::FAILURE;
            }
        }
    }
}

/// `:` inspection commands.
fn inspect(shell: &Shell, command: &str) {
    let mut words = command.split_whitespace();
    match words.next() {
        Some("runs") => {
            for report in shell.reports() {
                let source: String = report.source.chars().take(48).collect();
                println!(
                    "[{}] exit {} {}ms  {}",
                    report.id, report.status, report.duration_ms, source
                );
            }
        }
        Some("json") => match target_report(shell, words.next()) {
            Some(report) => match serde_json::to_string_pretty(&*report) {
                Ok(rendered) => println!("{rendered}"),
                Err(error) => eprintln!("plumb: {error}"),
            },
            None => eprintln!("plumb: no such run"),
        },
        Some(stream @ ("out" | "err")) => {
            let Some(report) = target_report(shell, words.next()) else {
                eprintln!("plumb: no such run");
                return;
            };
            let stage = words.next().and_then(|raw| raw.parse::<usize>().ok());
            let stages: Vec<&plumb_core::Stage> = report.stages().collect();
            let chosen = stage.map_or_else(
                || stages.last(),
                |index| stages.iter().find(|s| s.index == index),
            );
            match chosen {
                Some(chosen) => {
                    let capture = if stream == "out" {
                        &chosen.stdout
                    } else {
                        &chosen.stderr
                    };
                    print!("{}", capture.render());
                }
                None => eprintln!("plumb: no such stage"),
            }
        }
        _ => {
            println!(
                ":runs           list retained runs\n\
                 :json [N]       full report of run N (default: last)\n\
                 :out [N] [K]    stdout of run N stage K (default: last run, final stage)\n\
                 :err [N] [K]    stderr of run N stage K"
            );
        }
    }
}

fn target_report(shell: &Shell, id: Option<&str>) -> Option<std::sync::Arc<Report>> {
    id.map_or_else(
        || shell.reports().pop(),
        |raw| raw.parse().ok().and_then(|id| shell.report(id)),
    )
}

/// One line per stage, then the variable names the run bound.
fn summarize(report: &Report) {
    for id in &report.background_started {
        println!("[bg run {id} started]");
    }
    let stages: Vec<&plumb_core::Stage> = report.stages().collect();
    if stages.is_empty() {
        return;
    }
    for stage in &stages {
        println!(
            "[{}] {}  exit {}  {}ms (cpu {}+{}ms)  out {}  err {}",
            stage.index,
            stage.argv.join(" "),
            stage.status,
            stage.duration_ms,
            stage.user_ms,
            stage.sys_ms,
            human_bytes(stage.stdout.total_bytes()),
            human_bytes(stage.stderr.total_bytes()),
        );
    }
    let id = report.id;
    println!("run {id}: exit {}  ${{o[{id}]}} ${{e[{id}]}} ${{s[{id}]}}", report.status);
}

fn human_bytes(count: u64) -> String {
    #[expect(clippy::cast_precision_loss, reason = "a rounded size label needs no exact mantissa")]
    let bytes = count as f64;
    if count >= 1024 * 1024 {
        format!("{:.1}MB", bytes / (1024.0 * 1024.0))
    } else if count >= 1024 {
        format!("{:.1}KB", bytes / 1024.0)
    } else {
        format!("{count}B")
    }
}
