# Terranix-shaped Better Stack status-page stack, ported from ix's
# nix/terraform/status-page. Covers the page, a section, a monitor, a
# heartbeat, and the section resource wiring — every cross-resource
# "${type.name.field}" read becomes an efx reference.
#
# Deliberately absent, and asserted on separately in ../efx-plan.nix:
# - the escalation policy/severities: `betteruptime_policy.steps[].urgency_id`
#   is a reference *inside* a structured list, which the efx IR cannot
#   express; `fromTerranix` throws on it instead of emitting a plan that
#   silently drops the dependency (the monitors here carry no `policy_id`
#   for the same reason);
# - the `local_file` heartbeats export: its terraform `jsonencode(...)`
#   interpolation has no translation, so ../efx/default.nix declares it
#   natively as an html.render + file.write pair instead.
{
  resource = {
    betteruptime_status_page.ix = {
      company_name = "ix";
      company_url = "https://ix.dev";
      history = 7;
      timezone = "Eastern Time (US & Canada)";
      whitelabeled = false;
      subdomain = "ix";
      custom_domain = "status.ix.dev";
      design = "v2";
    };

    betteruptime_status_page_section.global = {
      status_page_id = "\${betteruptime_status_page.ix.id}";
      name = "Global";
      position = 0;
    };

    betteruptime_monitor.website_cli = {
      pronounceable_name = "Website & CLI";
      url = "https://ix.dev/";
      monitor_type = "expected_status_code";
      expected_status_codes = [200];
      check_frequency = 180;
      confirmation_period = 0;
    };

    betteruptime_heartbeat.orchestrator_liveness = {
      name = "orchestrator-liveness";
      period = 60;
      grace = 30;
    };

    betteruptime_status_page_resource.website_cli = {
      status_page_id = "\${betteruptime_status_page.ix.id}";
      status_page_section_id = "\${betteruptime_status_page_section.global.id}";
      resource_id = "\${betteruptime_monitor.website_cli.id}";
      resource_type = "Monitor";
      public_name = "Website & CLI";
      widget_type = "intraday_history";
      position = 0;
    };
  };
}
