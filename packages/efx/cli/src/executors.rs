//! The built-in executors: the static-site demo surface plus the cloud
//! kinds that replace the terranix/opentofu stacks.

use std::process::Command;

use efx_engine::{ExecuteError, ExecuteRequest, Executor, Outputs, Registry};
use efx_ir::Literal;

use crate::cloudflare;

/// All built-in executors under their canonical ids.
pub fn builtin_registry() -> Registry {
    let mut registry = Registry::new();
    registry.register("file.write", Box::new(FileWrite));
    registry.register("cmd.run", Box::new(CmdRun));
    registry.register("html.render", Box::new(HtmlRender));
    cloudflare::register(&mut registry);
    register_declared_gaps(&mut registry);
    registry
}

/// Cloud kinds the nix layer already plans but no executor reconciles yet.
/// Registered explicitly so `efx apply` fails them loudly with the reason
/// and the interim path — never silently, and never by pretending the
/// resource converged. `efx plan` over these kinds works fully (that is the
/// terranix-parity contract); apply-side coverage lands kind by kind.
fn register_declared_gaps(registry: &mut Registry) {
    let gaps: &[(&str, &str)] = &[
        (
            "cloudflare.ruleset",
            "zone rulesets reconcile through a phase-entrypoint API (list the \
             phase's ruleset, then replace its rule list) that is not wired up yet",
        ),
        (
            "cloudflare.email_routing_settings",
            "email routing enablement is not wired up yet",
        ),
        (
            "cloudflare.email_routing_address",
            "destination-address verification (Cloudflare mails the target a \
             confirmation) is not wired up yet",
        ),
        (
            "cloudflare.email_routing_rule",
            "routing-rule reconciliation (match by rule name, diff matchers and \
             actions) is not wired up yet",
        ),
        (
            "cloudflare.r2_managed_domain",
            "the managed-domain toggle on R2 buckets is not wired up yet",
        ),
        (
            "ovh.dedicated_server",
            "the OVH API's application-key request signing is not wired up yet",
        ),
        (
            "betteruptime.status_page",
            "Better Stack reconciliation (match by subdomain, PATCH drift) is \
             not wired up yet",
        ),
        (
            "betteruptime.status_page_section",
            "Better Stack reconciliation is not wired up yet",
        ),
        (
            "betteruptime.status_page_resource",
            "Better Stack reconciliation is not wired up yet",
        ),
        (
            "betteruptime.monitor",
            "Better Stack reconciliation (match by url, PATCH drift) is not \
             wired up yet",
        ),
        (
            "betteruptime.heartbeat",
            "Better Stack reconciliation (match by name, output the minted \
             heartbeat url) is not wired up yet",
        ),
        (
            "betteruptime.policy",
            "escalation policies carry references inside structured step lists, \
             which needs a native efx shape first",
        ),
        (
            "betteruptime.severity",
            "Better Stack reconciliation is not wired up yet",
        ),
    ];
    for (kind, gap) in gaps {
        registry.register(
            *kind,
            Box::new(Unimplemented {
                executor: kind,
                gap,
            }),
        );
    }
}

/// An executor id that is declared but deliberately not implemented yet.
/// Applying it fails loudly with the reason and the interim path, instead of
/// pretending the resource reconciled.
struct Unimplemented {
    executor: &'static str,
    gap: &'static str,
}

impl Executor for Unimplemented {
    fn execute(&self, request: &ExecuteRequest) -> Result<Outputs, ExecuteError> {
        Err(ExecuteError::new(format!(
            "executor `{}` is not implemented: {}. Effect `{}` was NOT applied; \
             keep applying this resource through the existing opentofu stack \
             until the executor lands.",
            self.executor, self.gap, request.name
        )))
    }
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
