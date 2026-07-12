# Local LLM inference: llama.cpp's HTTP server with a declared GGUF model
# fetched from Hugging Face before first start. Generalized from the
# llama.cpp setup in github:harivansh-afk/nix (hosts running an NVIDIA DGX
# Spark); the model choice, sampler settings, and hardware knobs are options
# here rather than hardcoded.
#
# What it adds over the upstream `services.llama-cpp` module it wraps:
#
#   * declarative model provenance: `model.repo` + `model.file` name a GGUF
#     on Hugging Face, and a oneshot pre-start unit downloads it into
#     `<stateDir>/models/<alias>` (hf-transfer accelerated, skipped once the
#     file exists) so the server never races an incomplete download;
#   * optional CUDA: `cuda.enable` rebuilds the package with `cudaSupport`
#     against a selectable `cudaPackages` set;
#   * the systemd tweaks a large mlock'd model needs (LimitMEMLOCK,
#     OOMScoreAdjust, /proc visibility) and an optional block-device
#     read-ahead bump for faster cold model loads.
#
# Example (the hari-compute-1 / DGX Spark shape this was extracted from):
#
#   services.inference = {
#     enable = true;
#     cuda = {
#       enable = true;
#       cudaPackages = pkgs.cudaPackages_13_1;
#     };
#     model = {
#       repo = "unsloth/Qwen3.6-35B-A3B-GGUF";
#       file = "Qwen3.6-35B-A3B-UD-Q4_K_XL.gguf";
#       alias = "qwen3.6-35b-a3b";
#     };
#     diskReadAhead.device = "nvme0n1";
#     settings = {
#       "ctx-size" = 65536;
#       parallel = 1;
#       "n-gpu-layers" = 99;
#       "no-mmap" = true;
#       mlock = true;
#       jinja = true;
#       "flash-attn" = "on";
#       temp = "0.7";
#       "top-p" = "0.8";
#       "top-k" = 20;
#     };
#   };
{
  config,
  ix,
  lib,
  pkgs,
  ...
}: let
  cfg = config.services.inference;

  effectivePackage =
    if cfg.cuda.enable
    then
      cfg.package.override {
        cudaSupport = true;
        cudaPackages = cfg.cuda.cudaPackages;
      }
    else cfg.package;

  huggingfaceCli = pkgs.python3.withPackages (pythonPackages: [
    pythonPackages.huggingface-hub
    pythonPackages.hf-transfer
  ]);

  modelDir = "${cfg.stateDir}/models/${cfg.model.alias}";
  modelPath = "${modelDir}/${cfg.model.file}";

  downloadModel = ix.writeBashApplication pkgs {
    name = "inference-model-download";
    runtimeInputs = [huggingfaceCli];
    text = ''
      if [ ! -s ${lib.escapeShellArg modelPath} ]; then
        hf download ${lib.escapeShellArg cfg.model.repo} \
          --include ${lib.escapeShellArg cfg.model.file} \
          --local-dir ${lib.escapeShellArg modelDir}
      fi
    '';
  };

  settingsValueType = lib.types.oneOf [
    lib.types.bool
    lib.types.int
    lib.types.float
    lib.types.str
  ];
in {
  options.services.inference = {
    enable = lib.mkEnableOption "the llama.cpp inference server with declarative model download";

    package = lib.mkPackageOption pkgs "llama-cpp" {};

    cuda = {
      enable = lib.mkEnableOption "building llama.cpp with CUDA support";

      cudaPackages = lib.mkOption {
        type = lib.types.raw;
        default = pkgs.cudaPackages;
        defaultText = lib.literalExpression "pkgs.cudaPackages";
        example = lib.literalExpression "pkgs.cudaPackages_13_1";
        description = ''
          CUDA package set to build against. Match it to the host's driver
          generation (e.g. `pkgs.cudaPackages_13_1` on a DGX Spark).
        '';
      };
    };

    model = {
      repo = lib.mkOption {
        type = lib.types.str;
        example = "unsloth/Qwen3.6-35B-A3B-GGUF";
        description = "Hugging Face repository the GGUF model is downloaded from.";
      };

      file = lib.mkOption {
        type = lib.types.str;
        example = "Qwen3.6-35B-A3B-UD-Q4_K_XL.gguf";
        description = "GGUF file name inside the repository (picks the quantization).";
      };

      alias = lib.mkOption {
        type = lib.types.str;
        example = "qwen3.6-35b-a3b";
        description = ''
          Model alias: the name the server advertises and the directory the
          model is stored under.
        '';
      };
    };

    host = lib.mkOption {
      type = lib.types.str;
      default = "127.0.0.1";
      description = "Address the server listens on.";
    };

    port = lib.mkOption {
      type = lib.types.port;
      default = 18_080;
      description = "Port the server listens on.";
    };

    stateDir = lib.mkOption {
      type = lib.types.str;
      default = "/var/lib/llama-cpp";
      description = ''
        Directory holding downloaded models (under `models/<alias>`) and the
        Hugging Face cache (under `huggingface`).
      '';
    };

    diskReadAhead = {
      device = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        example = "nvme0n1";
        description = ''
          Block device (under /sys/block) whose read-ahead is raised for
          faster cold model loads; null leaves the kernel default.
        '';
      };

      kb = lib.mkOption {
        type = lib.types.ints.positive;
        default = 8192;
        description = "Read-ahead in KiB applied to `diskReadAhead.device`.";
      };
    };

    settings = lib.mkOption {
      type = lib.types.attrsOf settingsValueType;
      default = {};
      example = {
        "ctx-size" = 65_536;
        "n-gpu-layers" = 99;
        mlock = true;
        "flash-attn" = "on";
      };
      description = ''
        Extra `llama-server` command-line settings merged over the computed
        host/port/model/alias entries; see `services.llama-cpp.settings`.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    services.llama-cpp = {
      enable = true;
      package = effectivePackage;
      settings =
        {
          inherit (cfg) host port;
          model = modelPath;
          inherit (cfg.model) alias;
        }
        // cfg.settings;
    };

    systemd = {
      tmpfiles.rules =
        [
          "d ${cfg.stateDir} 0755 root root -"
          "d ${cfg.stateDir}/models 0755 root root -"
          "d ${modelDir} 0755 root root -"
          "d ${cfg.stateDir}/huggingface 0755 root root -"
        ]
        ++ lib.optional (cfg.diskReadAhead.device != null)
        "w /sys/block/${cfg.diskReadAhead.device}/queue/read_ahead_kb - - - - ${toString cfg.diskReadAhead.kb}";

      services = {
        inference-model-download = {
          description = "Download the declared GGUF model before llama.cpp starts";
          before = ["llama-cpp.service"];
          environment = {
            HF_HOME = "${cfg.stateDir}/huggingface";
            HF_HUB_ENABLE_HF_TRANSFER = "1";
          };
          serviceConfig = {
            Type = "oneshot";
            ExecStart = lib.getExe downloadModel;
          };
        };

        llama-cpp = {
          after = ["inference-model-download.service"];
          requires = ["inference-model-download.service"];
          serviceConfig = {
            # A big mlock'd model is the first thing the kernel should
            # reclaim pressure from, and mlock itself needs an unlimited
            # lock budget.
            OOMScoreAdjust = 1000;
            LimitMEMLOCK = "infinity";
            # The upstream module pins ProcSubset/ProtectProc for hardening,
            # but llama-server's NUMA/topology probing needs full /proc
            # visibility; overriding a same-priority upstream definition
            # requires force.
            # astlog-ignore: no-mkforce
            ProcSubset = lib.mkForce "all";
            # astlog-ignore: no-mkforce
            ProtectProc = lib.mkForce "default";
          };
        };
      };
    };
  };
}
