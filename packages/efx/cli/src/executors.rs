//! The built-in executors: enough surface for a real static-site demo.

use std::process::Command;

use efx_engine::{ExecuteError, ExecuteRequest, Executor, Outputs, Registry};
use efx_ir::Literal;

/// All built-in executors under their canonical ids.
pub fn builtin_registry() -> Registry {
    let mut registry = Registry::new();
    registry.register("file.write", Box::new(FileWrite));
    registry.register("cmd.run", Box::new(CmdRun));
    registry.register("html.render", Box::new(HtmlRender));
    registry
}

fn required(request: &ExecuteRequest, key: &str) -> Result<String, ExecuteError> {
    request
        .inputs
        .get(key)
        .map(Literal::display_string)
        .ok_or_else(|| ExecuteError::new(format!("`{}` requires input `{key}`", request.kind)))
}

/// `file.write`: writes `content` to `path`, creating parent directories.
/// Outputs: `path`, `bytes`.
struct FileWrite;

impl Executor for FileWrite {
    fn execute(&self, request: &ExecuteRequest) -> Result<Outputs, ExecuteError> {
        let path = required(request, "path")?;
        let content = required(request, "content")?;
        if let Some(parent) = std::path::Path::new(&path).parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)
                .map_err(|err| ExecuteError::new(format!("create {}: {err}", parent.display())))?;
        }
        std::fs::write(&path, &content)
            .map_err(|err| ExecuteError::new(format!("write {path}: {err}")))?;
        let bytes = i64::try_from(content.len())
            .map_err(|_| ExecuteError::new("content larger than i64::MAX bytes"))?;
        Ok(Outputs::from([
            ("path".to_owned(), Literal::Str(path)),
            ("bytes".to_owned(), Literal::Int(bytes)),
        ]))
    }
}

/// `cmd.run`: runs `command` through `sh -c`. Outputs: `stdout` (trimmed),
/// `status`. A non-zero exit is a failure.
struct CmdRun;

impl Executor for CmdRun {
    fn execute(&self, request: &ExecuteRequest) -> Result<Outputs, ExecuteError> {
        let command = required(request, "command")?;
        let output = Command::new("sh")
            .arg("-c")
            .arg(&command)
            .output()
            .map_err(|err| ExecuteError::new(format!("spawn `{command}`: {err}")))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ExecuteError::new(format!(
                "`{command}` exited with {}: {}",
                output.status,
                stderr.trim()
            )));
        }
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        Ok(Outputs::from([
            ("stdout".to_owned(), Literal::Str(stdout)),
            (
                "status".to_owned(),
                Literal::Int(output.status.code().unwrap_or(0).into()),
            ),
        ]))
    }
}

/// `html.render`: substitutes every non-`template` input for `{key}` in
/// `template`. Outputs: `html`.
struct HtmlRender;

impl Executor for HtmlRender {
    fn execute(&self, request: &ExecuteRequest) -> Result<Outputs, ExecuteError> {
        let mut html = required(request, "template")?;
        for (key, value) in &request.inputs {
            if key != "template" {
                html = html.replace(&format!("{{{key}}}"), &value.display_string());
            }
        }
        Ok(Outputs::from([("html".to_owned(), Literal::Str(html))]))
    }
}
