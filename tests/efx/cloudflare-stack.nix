# Terranix-shaped Cloudflare stack, ported from ix's nix/terraform/cloudflare
# with sanitized values. This is the parity fixture: it keeps the terraform
# resource vocabulary (zones, DNS records derived from the inventory, the
# getix redirect ruleset, email routing, R2 buckets, Workers routes) and the
# "${type.name.field}" interpolation style, so `efx.fromTerranix` proves the
# real stack ports without restating every record.
#
# efx-specific deltas from the terraform original:
# - set-typed records (MX exchanges, CAA entries) carry `strategy = "ensure"`,
#   the cloudflare.dns_record executor's explicit "member of a set, never
#   replace a differing sibling" mode;
# - the `terraform.backend`/`provider` blocks are gone: state is the efx
#   journal, auth is the executor's environment.
{
  lib,
  inventory,
}: let
  cfAccount = "0123456789abcdef0123456789abcdef";
  zoneId = "\${cloudflare_zone.ix_dev.id}";
  getixZoneId = "\${cloudflare_zone.getix_dev.id}";
  leaderIpv4 = inventory.nodes.hel-leader-1.network.publicIpv4;
  edgeIpv4 = inventory.nodes.vin-compute-1.network.publicIpv4;

  aRecord = proxied: fqdn: ipv4: {
    name = lib.replaceStrings ["." "*"] ["_" "wildcard"] fqdn;
    value = {
      zone_id = zoneId;
      name = fqdn;
      type = "A";
      content = ipv4;
      ttl = 1;
      inherit proxied;
    };
  };
in {
  resource = {
    cloudflare_zone = {
      ix_dev = {
        account.id = cfAccount;
        name = "ix.dev";
        type = "full";
      };
      getix_dev = {
        account.id = cfAccount;
        name = "getix.dev";
        type = "full";
      };
    };

    cloudflare_dns_record = lib.listToAttrs [
      (aRecord true "ix.dev" leaderIpv4 // {name = "ix_dev_apex";})
      (aRecord false "api.ix.dev" leaderIpv4 // {name = "api_ix_dev";})
      (aRecord false "*.ix.dev" edgeIpv4 // {name = "wildcard_ix_dev";})
      {
        name = "ix_dev_spf";
        value = {
          zone_id = zoneId;
          name = "ix.dev";
          type = "TXT";
          # The Cloudflare API stores TXT content quoted.
          content = "\"v=spf1 include:_spf.google.com ip4:${leaderIpv4} -all\"";
          ttl = 1;
        };
      }
      {
        name = "ix_dev_mx_0";
        value = {
          zone_id = zoneId;
          name = "ix.dev";
          type = "MX";
          content = "aspmx.l.google.com";
          priority = 1;
          ttl = 1;
          strategy = "ensure";
        };
      }
      {
        name = "ix_dev_mx_1";
        value = {
          zone_id = zoneId;
          name = "ix.dev";
          type = "MX";
          content = "alt1.aspmx.l.google.com";
          priority = 5;
          ttl = 1;
          strategy = "ensure";
        };
      }
      {
        name = "ix_dev_caa_0";
        value = {
          zone_id = zoneId;
          name = "ix.dev";
          type = "CAA";
          ttl = 1;
          data = {
            flags = 0;
            tag = "issue";
            value = "letsencrypt.org";
          };
          strategy = "ensure";
        };
      }
      {
        name = "status_ix_dev";
        value = {
          zone_id = zoneId;
          name = "status.ix.dev";
          type = "CNAME";
          content = "statuspage.betteruptime.com";
          ttl = 1;
          proxied = false;
        };
      }
    ];

    cloudflare_ruleset.getix_redirect = {
      zone_id = getixZoneId;
      name = "default";
      kind = "zone";
      phase = "http_request_dynamic_redirect";
      rules = [
        {
          action = "redirect";
          action_parameters.from_value = {
            status_code = 301;
            preserve_query_string = true;
            target_url.value = "https://ix.dev";
          };
          expression = ''(http.host eq "getix.dev") or (http.host eq "www.getix.dev")'';
          description = "getix.dev is the install/marketing alias for ix.dev";
          enabled = true;
        }
      ];
    };

    cloudflare_email_routing_settings.getix = {
      zone_id = getixZoneId;
    };
    cloudflare_email_routing_address.andrew_gmail = {
      account_id = cfAccount;
      email = "andrew@example.com";
    };
    cloudflare_email_routing_rule.andrew = {
      zone_id = getixZoneId;
      name = "andrew@getix.dev forward";
      enabled = true;
      matchers = [
        {
          type = "literal";
          field = "to";
          value = "andrew@getix.dev";
        }
      ];
      actions = [
        {
          type = "forward";
          value = ["andrew@example.com"];
        }
      ];
    };

    cloudflare_r2_bucket = {
      sdk_artifacts = {
        account_id = cfAccount;
        name = "ix-sdk-artifacts";
      };
      cli = {
        account_id = cfAccount;
        name = "ix-cli";
      };
    };
    cloudflare_r2_managed_domain.sdk_artifacts = {
      account_id = cfAccount;
      bucket_name = "\${cloudflare_r2_bucket.sdk_artifacts.name}";
      enabled = true;
    };

    cloudflare_workers_route = {
      ix_web_apex = {
        zone_id = zoneId;
        pattern = "ix.dev/*";
        script = "ix-web";
      };
      # Bypass route (no script): /api requests skip the worker.
      ix_api_bypass = {
        zone_id = zoneId;
        pattern = "ix.dev/api/*";
      };
    };
  };
}
