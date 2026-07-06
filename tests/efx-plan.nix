# efx terranix-port parity tests (see tests/efx/). The ported stacks must
# translate to exactly the golden plan IR fixture the efx CLI's own tests
# consume (`efx plan --ir`), so the nix emitter and the Rust parser are
# pinned to one artifact. The rest are the translator's loud-failure
# guarantees: anything `fromTerranix` cannot represent faithfully throws at
# eval time instead of emitting a plan that means something else.
{
  lib,
  ix,
  paths,
}: let
  inherit (ix) efx;
  port = import ./efx {inherit lib ix;};
  golden = lib.importJSON (
    paths.root + "/packages/efx/cli/tests/fixtures/terranix_port.plan.json"
  );
  byName = effects: ix.lists.genAttrs' effects (e: lib.nameValuePair e.name e);
  effectsByName = byName port.effects;
  goldenByName = byName golden.effects;
  throws = value: !(builtins.tryEval (builtins.deepSeq value value)).success;

  ovhEffects = efx.fromTerranix {
    config = import ./efx/ovh-stack.nix {
      inherit lib;
      inherit (port) inventory;
    };
  };

  localFilePort = efx.fromTerranix {
    config.resource.local_file.hello = {
      filename = "out/hello.txt";
      content = "hi";
    };
  };
in [
  {
    assertion = map (e: e.name) port.effects == map (e: e.name) golden.effects;
    message = "efx terranix port should declare exactly the golden fixture's effects, in order (regenerate per tests/efx/default.nix after an intentional change)";
  }
  {
    assertion = port.plan == golden;
    message = "efx terranix port should render exactly packages/efx/cli/tests/fixtures/terranix_port.plan.json (regenerate per tests/efx/default.nix after an intentional change)";
  }
  {
    assertion = lib.all (
      name: effectsByName.${name} == goldenByName.${name}
    ) (builtins.attrNames goldenByName);
    message = "every ported effect should match its golden counterpart field for field";
  }
  {
    assertion =
      effectsByName."cloudflare_dns_record.ix_dev_apex".inputs.zone_id
      == {
        ref = {
          effect = "cloudflare_zone.ix_dev";
          field = "id";
        };
      };
    message = "terraform interpolation strings should translate to first-class efx references";
  }
  {
    assertion =
      effectsByName."cloudflare_zone.ix_dev".inputs."account.id".literal
      == port.stacks.cloudflare.resource.cloudflare_zone.ix_dev.account.id;
    message = "nested resource attrsets should flatten to dotted input keys";
  }
  {
    assertion =
      effectsByName."cloudflare_dns_record.ix_dev_mx_0".inputs.strategy.literal == "ensure";
    message = "set-typed records should carry the explicit ensure strategy through translation";
  }
  {
    assertion =
      effectsByName."betteruptime_monitor.website_cli".inputs.expected_status_codes.literal
      == "[200]";
    message = "list-valued resource attributes should become canonical-JSON string inputs";
  }
  {
    assertion = lib.all (e: lib.hasPrefix "ovh_dedicated_server." e.name) ovhEffects;
    message = "terraform/provider/import blocks should be dropped, leaving only resource effects";
  }
  {
    assertion =
      (builtins.head localFilePort).kind
      == "file.write"
      && (builtins.head localFilePort).inputs.path.literal == "out/hello.txt";
    message = "local_file should port onto the native file.write executor with filename renamed to path";
  }
  {
    # The efx IR cannot express a reference inside a structured value; the
    # translator must refuse (loudly) rather than emit a plan whose
    # dependency silently became a literal string.
    assertion = throws (efx.fromTerranix {
      config.resource.betteruptime_policy.default = {
        name = "ix-default";
        steps = [
          {
            type = "escalation";
            urgency_id = "\${betteruptime_severity.sms_first.id}";
            wait_before = 0;
          }
        ];
      };
    });
    message = "fromTerranix should throw on references nested inside structured values (the escalation-policy shape)";
  }
  {
    assertion = throws (efx.fromTerranix {
      config.resource.cloudflare_dns_record.bad = {
        zone_id = "prefix-\${cloudflare_zone.ix_dev.id}";
        name = "x.ix.dev";
        type = "A";
        content = "192.0.2.1";
      };
    });
    message = "fromTerranix should throw on interpolations embedded mid-string";
  }
  {
    assertion = throws (efx.fromTerranix {
      config.resource.demo_thing.bad.ratio = 1.5;
    });
    message = "fromTerranix should throw on floats (the IR is deliberately float-free)";
  }
  {
    assertion = throws (efx.fromTerranix {config.locals.x = 1;});
    message = "fromTerranix should throw on unsupported top-level terranix keys";
  }
  {
    assertion = throws (efx.plan [
      (efx.effect {
        name = "a";
        kind = "cmd.run";
      })
      (efx.effect {
        name = "a";
        kind = "cmd.run";
      })
    ]);
    message = "efx.plan should throw on duplicate effect names";
  }
  {
    assertion = throws (efx.plan [
      (efx.effect {
        name = "a";
        kind = "file.write";
        inputs.content = efx.ref "missing" "out";
      })
    ]);
    message = "efx.plan should throw on references to undeclared effects";
  }
  {
    assertion = throws (efx.effect {
      name = "a";
      kind = "cmd.run";
      inputs.structured = {nested = true;};
    });
    message = "native effect inputs should reject structured values that were not explicitly JSON-encoded";
  }
]
